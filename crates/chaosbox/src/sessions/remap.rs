//! Deterministic provenance-derived identifiers for divergent variants.
//!
//! A variant is a second, genuinely different copy of a session the
//! destination already holds under its original id. Native import is
//! create-only and `session_message.id` is a global primary key, so a variant
//! materializes under fresh identifiers for the session and for every one of
//! its messages. Those identifiers are a pure function of provenance — the
//! same inputs produce the same id on every machine and every run — which is
//! what lets the verifier re-derive them from a receipt instead of trusting
//! the mapping that assigned them.
//!
//! The algorithm is `variant-ids.mjs`, ported exactly: SHA-256 over NUL
//! joined fields, the first 6 bytes as hex, the remaining 26 bytes reduced
//! modulo 62¹⁴ and rendered as 14 base62 digits. `golden-vectors.json` is the
//! acceptance test; every vector below is asserted in `tests/sessions.rs`.

use sha2::{Digest, Sha256};

/// Seed for the session derivation, namespaced so no other input can collide
/// with it.
const SESSION_SEED: &str = "chaosbox.variant-session-id.v1";
/// Seed for the message derivation.
const MESSAGE_SEED: &str = "chaosbox.variant-message-id.v1";
/// Alphabet the 14-character suffix is rendered in.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// Suffix length in characters.
const SUFFIX_LENGTH: usize = 14;
/// 62¹⁴: the suffix space one derivation draws from.
const SUFFIX_MODULUS: u128 = 62_u128.pow(14);

/// Derive a variant session id from its provenance: the source snapshot it
/// was read from, the canonical session it diverges from, and the
/// collision-resolve attempt that was actually used.
#[must_use]
pub fn variant_session_id(source: &str, canonical: &str, attempt: u64) -> String {
    render("ses", SESSION_SEED, &[source, canonical], attempt)
}

/// Derive a variant message id from the derived session it belongs to, the
/// original message id, and the attempt that was actually used.
#[must_use]
pub fn variant_message_id(derived_session: &str, original: &str, attempt: u64) -> String {
    render("msg", MESSAGE_SEED, &[derived_session, original], attempt)
}

/// Whether an id has the exact shape every native session id in this build
/// uses: `ses_`, 12 lowercase hex digits, 14 alphanumerics.
#[must_use]
pub fn is_native_session_shape(id: &str) -> bool {
    is_native_shape(id, "ses_")
}

/// Whether an id has the exact shape every native message id uses.
#[must_use]
pub fn is_native_message_shape(id: &str) -> bool {
    is_native_shape(id, "msg_")
}

/// One re-keyed message from the pinned mapping: the source id, the derived
/// id the destination holds, and the attempt that was actually used.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct MessagePair {
    /// Message id in the source snapshot.
    pub original: String,
    /// Message id in the destination.
    pub derived: String,
    /// Collision-resolve attempt that produced `derived`.
    pub attempt: u64,
}

/// One variant from the pinned mapping. Field order reproduces the file's
/// key order exactly — the mapping is machine-written with uniform order,
/// so a plain struct round-trips `JSON.stringify(mapping.variants)`
/// byte-for-byte without an order-preserving map.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct VariantEntry {
    /// Derived session id the destination holds.
    #[serde(rename = "sessionID")]
    pub session_id: String,
    /// Collision-resolve attempt behind `session_id`.
    #[serde(rename = "idAttempt")]
    pub id_attempt: u64,
    /// Source snapshot the variant was read from.
    pub source: String,
    /// Canonical session the variant diverges from.
    #[serde(rename = "sourceSessionID")]
    pub source_session_id: String,
    /// Always `"divergent"` in the mapping (receipts say
    /// `"divergent-variant"`).
    pub kind: String,
    /// Message count.
    pub messages: u64,
    /// Duplicate of `id_attempt`, kept where the mapping wrote it.
    #[serde(rename = "sessionAttempt")]
    pub session_attempt: u64,
    /// Snapshot holding the canonical session, which may differ from
    /// `source`.
    #[serde(rename = "canonicalSource")]
    pub canonical_source: String,
    /// Source row stamps, carried for the audit trail.
    #[serde(rename = "sourceTimeCreated")]
    pub source_time_created: i64,
    /// Source row stamps, carried for the audit trail.
    #[serde(rename = "sourceTimeUpdated")]
    pub source_time_updated: i64,
    /// Source parent, absent for roots.
    #[serde(rename = "parentID")]
    pub parent_id: Option<String>,
    /// Source title at materialization time.
    pub title: Option<String>,
    /// Source directory at materialization time.
    pub directory: Option<String>,
    /// Directory the destination row carries after remapping.
    #[serde(rename = "remappedDirectory")]
    pub remapped_directory: Option<String>,
    /// Manifest digest of the source snapshot.
    #[serde(rename = "sourceSnapshotSha256")]
    pub source_snapshot_sha256: Option<String>,
    /// Reconciliation table the divergence was found in.
    #[serde(rename = "occurrenceTable")]
    pub occurrence_table: Option<String>,
    /// Every re-keyed message, in `(seq, id)` order.
    #[serde(rename = "messageIDs")]
    pub message_ids: Vec<MessagePair>,
}

/// The `variants` array of the pinned mapping file; every other top-level
/// field is unpinned by design and ignored here.
#[derive(Clone, Debug, serde::Deserialize)]
struct MappingFile {
    variants: Vec<VariantEntry>,
}

/// Parse the `variants` array out of a mapping file, ignoring every other
/// top-level field: only `variants` is pinned, by design.
///
/// # Errors
///
/// Returns the parse failure when the file is not a mapping.
pub fn parse_mapping_variants(text: &str) -> Result<Vec<VariantEntry>, serde_json::Error> {
    Ok(serde_json::from_str::<MappingFile>(text)?.variants)
}

/// Recompute the digest `identity-v3.json` pins over a mapping file:
/// SHA-256 over the compact JSON serialization of `mapping.variants`.
///
/// # Errors
///
/// Returns the parse failure when the file is not a mapping.
pub fn mapping_variants_digest(text: &str) -> Result<String, serde_json::Error> {
    let variants = parse_mapping_variants(text)?;
    let compact = serde_json::to_string(&variants)?;
    let mut hasher = Sha256::new();
    hasher.update(compact.as_bytes());
    Ok(hex(&hasher.finalize()))
}

/// The two-sided re-key proof from contract §5.4: SHA-256 over the
/// concatenation of `original NUL derived newline` for each message pair in
/// `(seq, id)` order. A dropped, duplicated, or reordered message breaks the
/// digest even when the counts still match, and no stored map is needed.
pub fn message_map_digest<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut hasher = Sha256::new();
    for (original, derived) in pairs {
        hasher.update(original.as_bytes());
        hasher.update([0_u8]);
        hasher.update(derived.as_bytes());
        hasher.update(b"\n");
    }
    hex(&hasher.finalize())
}

/// Render one derived identifier: `prefix_`, 12 hex digits, 14 base62
/// digits. Attempt 0 contributes no tag; any other attempt appends
/// `NUL attempt=<n>`, so attempt 0 and attempt 1 can never derive the same
/// id.
fn render(prefix: &str, seed: &str, fields: &[&str], attempt: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    for field in fields {
        hasher.update([0_u8]);
        hasher.update(field.as_bytes());
    }
    if attempt != 0 {
        hasher.update([0_u8]);
        hasher.update(format!("attempt={attempt}").as_bytes());
    }
    let digest = hasher.finalize();

    let mut id = String::with_capacity(prefix.len() + 1 + 12 + SUFFIX_LENGTH);
    id.push_str(prefix);
    id.push('_');
    for byte in &digest[0..6] {
        id.push(lower_hex(*byte >> 4));
        id.push(lower_hex(*byte & 0x0f));
    }
    // The 26 remaining bytes reduced modulo 62¹⁴ by Horner: the running
    // value stays below 2⁹², far inside a u128.
    let mut residue: u128 = 0;
    for byte in &digest[6..32] {
        residue = (residue * 256 + u128::from(*byte)) % SUFFIX_MODULUS;
    }
    let mut suffix = [b'0'; SUFFIX_LENGTH];
    for slot in suffix.iter_mut().rev() {
        *slot = BASE62[usize::try_from(residue % 62).unwrap_or(0)];
        residue /= 62;
    }
    id.push_str(std::str::from_utf8(&suffix).expect("base62 is ASCII"));
    id
}

/// One lowercase hex digit for a nibble.
fn lower_hex(nibble: u8) -> char {
    (if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + nibble - 10
    }) as char
}

/// Shape check shared by both identifier kinds.
fn is_native_shape(id: &str, prefix: &str) -> bool {
    let Some(rest) = id.strip_prefix(prefix) else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() == 26
        && bytes[..12]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        && bytes[12..].iter().all(u8::is_ascii_alphanumeric)
}

/// Lowercase hex, matching Node's `digest("hex")`.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(lower_hex(byte >> 4));
        out.push(lower_hex(byte & 0x0f));
    }
    out
}
