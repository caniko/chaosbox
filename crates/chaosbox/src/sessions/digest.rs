//! Byte-compatible reimplementation of the migration's `sessionHash`
//! contract.
//!
//! The contract is defined by `history-transfer-support.mjs`: each row is
//! canonicalized with object keys sorted at every level, serialized using
//! JavaScript's `JSON.stringify` rules, and fed to SHA-256 followed by a NUL
//! byte. Every receipt already on disk was produced that way, so `verify`
//! means nothing unless Rust reproduces those bytes exactly.

use std::{collections::BTreeMap, fmt::Write};

use rusqlite::{types::ValueRef, Connection, Row, Statement};
use sha2::{Digest, Sha256};

/// Link tables folded into a session digest only when the schema has them.
const LINK_TABLES: [&str; 2] = ["session_pending", "session_inbox"];

/// Largest integer JavaScript still represents exactly, so `JSON.stringify`
/// prints it as an integer rather than as the rounded `f64` it became.
const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// A digest could not be computed over a session.
#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    /// The session has no `session_v2` row to anchor the digest.
    #[error("session {0} has no row in session_v2")]
    MissingSession(String),
    /// A BLOB reached the encoder. The contract covers text and numbers, and
    /// guessing an encoding would produce a digest nobody else can reproduce.
    #[error("{table}.{column} holds a BLOB; the digest contract covers text and numbers only")]
    BlobValue {
        /// Table the offending column belongs to.
        table: String,
        /// Column that held the BLOB.
        column: String,
    },
    /// SQLite refused to read the database.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// What one session digest covered, so callers can check counts for free.
#[derive(Clone, Debug)]
pub struct SessionDigest {
    /// Lowercase hex SHA-256 over the canonical fragments.
    pub digest: String,
    /// `session_message` rows absorbed while hashing.
    pub messages: usize,
}

/// Recompute the destination digest of one session exactly as the migration
/// wrote it, alongside the message count it covered.
///
/// # Errors
///
/// Returns [`DigestError::MissingSession`] when `session_v2` has no row,
/// [`DigestError::BlobValue`] on an out-of-contract column, and
/// [`DigestError::Sqlite`] when the database cannot be read.
pub fn session_digest(connection: &Connection, id: &str) -> Result<SessionDigest, DigestError> {
    let mut hasher = Sha256::new();
    let mut fragment = String::new();

    let present = encode_one(
        connection,
        "SELECT * FROM session_v2 WHERE id = ?",
        id,
        "session_v2",
        &mut fragment,
    )?;
    if !present {
        return Err(DigestError::MissingSession(id.to_string()));
    }
    absorb(&mut hasher, &fragment);

    let messages = hash_rows(
        connection,
        "SELECT * FROM session_message WHERE session_id = ? ORDER BY seq, id",
        id,
        "session_message",
        &mut hasher,
        &mut fragment,
    )?;

    for table in LINK_TABLES {
        if !table_exists(connection, table)? {
            continue;
        }
        let sql = format!("SELECT * FROM {table} WHERE session_id = ? ORDER BY id");
        let mut statement = connection.prepare(&sql)?;
        let columns = sorted_columns(&statement);
        let mut rows = statement.query(rusqlite::params![id])?;
        while let Some(row) = rows.next()? {
            fragment.clear();
            fragment.push('[');
            encode_text(table, &mut fragment);
            fragment.push(',');
            encode_row(&columns, row, table, &mut fragment)?;
            fragment.push(']');
            absorb(&mut hasher, &fragment);
        }
    }

    Ok(SessionDigest {
        digest: hex(&hasher.finalize()),
        messages,
    })
}

/// Hex digest of a session without its message count.
///
/// # Errors
///
/// See [`session_digest`].
pub fn session_hash(connection: &Connection, id: &str) -> Result<String, DigestError> {
    Ok(session_digest(connection, id)?.digest)
}

/// Digest of the recovered rows backing a session's truncation recovery,
/// matching `recoveredHash` from `history-transfer-support.mjs`.
///
/// # Errors
///
/// Returns [`DigestError::Sqlite`] when the database cannot be read.
pub fn recovered_hash(connection: &Connection, id: &str) -> Result<String, DigestError> {
    let mut hasher = Sha256::new();
    let mut fragment = String::new();
    hash_rows(
        connection,
        "SELECT * FROM recovered WHERE session_id = ? ORDER BY id",
        id,
        "recovered",
        &mut hasher,
        &mut fragment,
    )?;
    Ok(hex(&hasher.finalize()))
}

/// Hash every row a query returns, reusing one fragment buffer.
fn hash_rows(
    connection: &Connection,
    sql: &str,
    id: &str,
    table: &str,
    hasher: &mut Sha256,
    fragment: &mut String,
) -> Result<usize, DigestError> {
    let mut statement = connection.prepare(sql)?;
    let columns = sorted_columns(&statement);
    let mut rows = statement.query(rusqlite::params![id])?;
    let mut count = 0_usize;
    while let Some(row) = rows.next()? {
        fragment.clear();
        encode_row(&columns, row, table, fragment)?;
        absorb(hasher, fragment);
        count += 1;
    }
    Ok(count)
}

/// Encode at most one row, reporting whether one was found.
fn encode_one(
    connection: &Connection,
    sql: &str,
    id: &str,
    table: &str,
    fragment: &mut String,
) -> Result<bool, DigestError> {
    let mut statement = connection.prepare(sql)?;
    let columns = sorted_columns(&statement);
    let mut rows = statement.query(rusqlite::params![id])?;
    fragment.clear();
    let Some(row) = rows.next()? else {
        return Ok(false);
    };
    encode_row(&columns, row, table, fragment)?;
    Ok(true)
}

/// Column names paired with their index, already in canonical key order.
fn sorted_columns(statement: &Statement<'_>) -> BTreeMap<String, usize> {
    statement
        .column_names()
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name.to_string(), index))
        .collect()
}

/// Whether a table exists, for the link tables the contract treats as
/// optional.
///
/// Only "no such row" means the table is absent. Any other failure — a
/// corrupt database, a closed connection — has to stay an error, because
/// silently answering "no" would drop an entire table out of a digest and
/// produce a wrong number that still verifies.
fn table_exists(connection: &Connection, table: &str) -> Result<bool, DigestError> {
    match connection.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
        [table],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(source) => Err(DigestError::Sqlite(source)),
    }
}

/// One NUL-terminated canonical fragment into the running hash.
fn absorb(hasher: &mut Sha256, fragment: &str) {
    hasher.update(fragment.as_bytes());
    hasher.update([0_u8]);
}

/// Encode a row as an object with keys sorted, as `canonical()` would.
fn encode_row(
    columns: &BTreeMap<String, usize>,
    row: &Row<'_>,
    table: &str,
    out: &mut String,
) -> Result<(), DigestError> {
    out.push('{');
    for (position, (name, index)) in columns.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        encode_text(name, out);
        out.push(':');
        encode_value(row.get_ref(*index)?, table, name, out)?;
    }
    out.push('}');
    Ok(())
}

/// Encode one SQLite value using JavaScript's `JSON.stringify` rules.
fn encode_value(
    value: ValueRef<'_>,
    table: &str,
    column: &str,
    out: &mut String,
) -> Result<(), DigestError> {
    match value {
        ValueRef::Null => out.push_str("null"),
        ValueRef::Integer(integer) => encode_integer(integer, out),
        ValueRef::Real(real) => out.push_str(&encode_js_real(real)),
        ValueRef::Text(text) => encode_text(&String::from_utf8_lossy(text), out),
        ValueRef::Blob(_) => {
            return Err(DigestError::BlobValue {
                table: table.to_string(),
                column: column.to_string(),
            });
        }
    }
    Ok(())
}

/// Integers stay integers while they fit in a JavaScript number; beyond that
/// the reader already rounded them to an `f64`, so the `f64` form is what
/// `JSON.stringify` saw. Precision loss is the point of this branch.
#[allow(clippy::cast_precision_loss)]
fn encode_integer(value: i64, out: &mut String) {
    if (-JS_MAX_SAFE_INTEGER..=JS_MAX_SAFE_INTEGER).contains(&value) {
        out.push_str(&value.to_string());
    } else {
        out.push_str(&encode_js_real(value as f64));
    }
}

/// Quote and escape a string exactly as `JSON.stringify` does.
fn encode_text(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < '\u{20}' => {
                write!(out, "\\u{:04x}", u32::from(control))
                    .expect("writing to a String cannot fail");
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// `Number::toString` for a finite double: shortest round-tripped digits in
/// whichever notation ECMAScript selects, and `null` for the two values
/// `JSON.stringify` refuses to print.
fn encode_js_real(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_string();
    }
    if value == 0.0 {
        // Also covers negative zero, which `JSON.stringify` prints as `0`.
        return "0".to_string();
    }
    let mut text = encode_positive_real(value.abs());
    if value < 0.0 {
        text.insert(0, '-');
    }
    text
}

/// Notation selection for a positive, finite, non-zero double.
fn encode_positive_real(value: f64) -> String {
    let (digits, position) = shortest_digits(value);
    let length = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if length <= position && position <= 21 {
        let padding = usize::try_from(position - length).unwrap_or(0);
        format!("{digits}{}", "0".repeat(padding))
    } else if (1..=21).contains(&position) {
        let head_end = usize::try_from(position).unwrap_or(0);
        format!("{}.{}", &digits[..head_end], &digits[head_end..])
    } else if position <= 0 && position > -6 {
        let zeros = usize::try_from(-position).unwrap_or(0);
        format!("0.{}{digits}", "0".repeat(zeros))
    } else {
        encode_exponential(&digits, position)
    }
}

/// Exponential form with the explicit exponent sign JavaScript emits.
fn encode_exponential(digits: &str, position: i32) -> String {
    let exponent = position - 1;
    let sign = if exponent < 0 { '-' } else { '+' };
    let mantissa = match digits.split_at(1) {
        (head, "") => head.to_string(),
        (head, tail) => format!("{head}.{tail}"),
    };
    format!("{mantissa}e{sign}{}", exponent.unsigned_abs())
}

/// Shortest round-tripped digits plus the decimal position `n` for which the
/// value equals `digits * 10^(n - k)`, where `k` is the digit count.
///
/// Rust's `Display` never uses exponent notation, so the plain rendering has
/// to be unpicked into the pieces ECMAScript's notation rules expect. Two
/// kinds of zero carry no information: the ones `Display` used to pad the
/// integer part out to the value's magnitude, and — below one — the ones it
/// used to push the first significant digit away from the decimal point.
/// Those belong to `n`, not to `digits`; keeping them would shift every
/// fraction like `0.448854` by a place.
fn shortest_digits(value: f64) -> (String, i32) {
    let rendered = format!("{value}");
    let (integer, fraction) = rendered.split_once('.').unwrap_or((&rendered, ""));
    if integer.trim_start_matches('0').is_empty() {
        // `0 < |value| < 1`: leading fraction zeros move onto the position.
        let significant = fraction.trim_start_matches('0');
        let leading = i32::try_from(fraction.len() - significant.len()).unwrap_or(0);
        let digits = significant.trim_end_matches('0');
        return (nonempty_digits(digits), leading.saturating_neg());
    }
    let position = i32::try_from(integer.len()).unwrap_or(0);
    // Concatenating the two parts drops the `.` itself: `digits` has to be
    // pure digits, because notation selection slices it by decimal position.
    let mut digits = String::with_capacity(rendered.len());
    digits.push_str(integer);
    digits.push_str(fraction);
    (nonempty_digits(digits.trim_end_matches('0')), position)
}

/// Guard against a rendering that carried no significant digit at all, which
/// `sessionDigest` never sees because zero is handled before notation is
/// chosen but which would otherwise slice past an empty string.
fn nonempty_digits(digits: &str) -> String {
    if digits.is_empty() {
        "0".to_string()
    } else {
        digits.to_string()
    }
}

/// Lowercase hex, matching Node's `digest("hex")`.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// Canonical JSON string matching `history-transfer-support.mjs` `canonical`
/// plus `JSON.stringify`: object keys sorted recursively at every level,
/// strings escaped exactly as `JSON.stringify` does.
///
/// Used for `boundaryRecordSha256`: the merge driver writes
/// `digest(boundary)`, where `boundary` is the parsed boundary-record JSON.
/// Verifying that digest binds a receipt to the exact record bytes (modulo
/// key order and whitespace), independent of how the file was pretty-printed.
#[must_use]
pub fn canonical_json_string(value: &serde_json::Value) -> String {
    let mut out = String::new();
    encode_canonical(value, &mut out);
    out
}

/// SHA-256 hex over [`canonical_json_string`], matching Node's
/// `digest(value)`.
#[must_use]
pub fn canonical_json_digest(value: &serde_json::Value) -> String {
    use sha2::Digest as _;
    let mut hasher = Sha256::new();
    hasher.update(canonical_json_string(value).as_bytes());
    hex(&hasher.finalize())
}

/// One canonical JSON value into the buffer.
fn encode_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                encode_integer(integer, out);
            } else if let Some(unsigned) = number.as_u64() {
                encode_canonical_u64(unsigned, out);
            } else if let Some(real) = number.as_f64() {
                out.push_str(&encode_js_real(real));
            } else {
                out.push_str(&number.to_string());
            }
        }
        serde_json::Value::String(text) => encode_text(text, out),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_canonical(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_text(key, out);
                out.push(':');
                encode_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// Unsigned integers: exact while they fit in a JavaScript number, otherwise
/// the rounded `f64` rendering `JSON.stringify` saw.
#[allow(clippy::cast_precision_loss)]
fn encode_canonical_u64(value: u64, out: &mut String) {
    if value <= JS_MAX_SAFE_INTEGER as u64 {
        out.push_str(&value.to_string());
    } else {
        out.push_str(&encode_js_real(value as f64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Byte-exact `JSON.stringify` outputs captured from
    /// `fixtures/sessions/js-encoder-ref.mjs`, including the canonical
    /// fragments the fixture below inserts. Every expectation here is
    /// machine-generated: regenerate that script's output instead of
    /// editing these numbers or strings by hand.
    const REFERENCE: &str = include_str!("../../../../fixtures/sessions/js-encoder-ref.json");

    /// The fixture rows, shaped exactly like the fragments above. Column
    /// order is deliberately not the canonical key order, so these tests
    /// fail if sorting ever stops happening.
    const FIXTURE_SQL: &str = r#"
        CREATE TABLE session_v2 (
            id TEXT PRIMARY KEY,
            seq INTEGER,
            time INTEGER,
            cost REAL,
            meta TEXT,
            nul TEXT
        );
        INSERT INTO session_v2 (id, seq, time, cost, meta, nul)
        VALUES ('ses_x', 3, 1790166653727, 3.6143699999999996,
                '{"b":1,"a":2}', NULL);

        CREATE TABLE session_message (
            session_id TEXT,
            seq INTEGER,
            id TEXT,
            role TEXT,
            cost REAL,
            time INTEGER,
            text TEXT
        );
        INSERT INTO session_message (session_id, seq, id, role, cost, time, text)
        VALUES ('ses_x', 1, 'm1', 'user', 0, 1790166653727,
                'line' || char(10) || 'break');

        CREATE TABLE session_pending (
            session_id TEXT,
            id TEXT,
            seq INTEGER,
            time INTEGER,
            cost REAL,
            meta TEXT,
            nul TEXT
        );
        INSERT INTO session_pending (session_id, id, seq, time, cost, meta, nul)
        VALUES ('ses_x', 'm1', 3, 1790166653727, 3.6143699999999996,
                '{"b":1,"a":2}', NULL);
    "#;

    /// Parse the captured reference table.
    fn reference() -> Value {
        serde_json::from_str(REFERENCE).expect("reference table parses")
    }

    /// A connection holding exactly the fixture rows.
    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(FIXTURE_SQL)
            .expect("fixture loads");
        connection
    }

    /// Every captured real number, in whichever notation ECMAScript picked.
    ///
    /// Inputs arrive as text and go through `str::parse`, not through a JSON
    /// parser: the reference is only meaningful if the `f64` under test is
    /// the exact double the string denotes.
    #[test]
    fn reals_match_json_stringify() {
        let table = reference();
        for case in table["realCases"].as_array().expect("realCases") {
            let digits = case[0].as_str().expect("digits");
            let expected = case[1].as_str().expect("string");
            let value = digits.parse::<f64>().expect("f64");
            assert_eq!(encode_js_real(value), expected, "case {digits}");
        }
    }

    /// Signed zero and the two values `JSON.stringify` renders as `null`.
    #[test]
    fn zero_and_non_finite_match_json_stringify() {
        assert_eq!(encode_js_real(-0.0), "0");
        assert_eq!(encode_js_real(0.0), "0");
        assert_eq!(encode_js_real(f64::NAN), "null");
        assert_eq!(encode_js_real(f64::INFINITY), "null");
        assert_eq!(encode_js_real(f64::NEG_INFINITY), "null");
    }

    /// Integers stay integers while JavaScript can hold them exactly, and
    /// become the rounded `f64` rendering once it cannot.
    #[test]
    fn integers_match_json_stringify() {
        let table = reference();
        for case in table["integerCases"].as_array().expect("integerCases") {
            let digits = case[0].as_str().expect("digits");
            let expected = case[1].as_str().expect("string");
            let value = digits.parse::<i64>().expect("i64");
            let mut encoded = String::new();
            encode_integer(value, &mut encoded);
            assert_eq!(encoded, expected, "integer {digits}");
        }
    }

    /// Escaping, including the control characters and U+2028/U+2029 that
    /// plain reference dumps never exercised.
    #[test]
    fn strings_match_json_stringify() {
        let table = reference();
        for case in table["stringCases"].as_array().expect("stringCases") {
            let input = case[0].as_str().expect("input");
            let expected = case[1].as_str().expect("string");
            let mut encoded = String::new();
            encode_text(input, &mut encoded);
            assert_eq!(encoded, expected, "string {input:?}");
        }
    }

    /// The session row on its own, proving key sorting happens at all.
    #[test]
    fn session_row_matches_canonical_json_stringify() {
        let table = reference();
        let expected = table["fragments"]["session"].as_str().expect("fragment");
        let connection = fixture();

        let mut encoded = String::new();
        let found = encode_one(
            &connection,
            "SELECT * FROM session_v2 WHERE id = ?",
            "ses_x",
            "session_v2",
            &mut encoded,
        )
        .expect("session row encodes");

        assert!(found, "the fixture has a session row");
        assert_eq!(encoded, expected);
    }

    /// Digest composition: fragments in schema order, each NUL-terminated,
    /// so a change to separators or ordering breaks the test rather than
    /// silently invalidating every receipt.
    #[test]
    fn session_digest_composes_fragments_with_nul_separators() {
        let table = reference();
        let fragments = &table["fragments"];
        let connection = fixture();
        let computed = session_digest(&connection, "ses_x").expect("digest");

        let mut hasher = Sha256::new();
        for key in ["session", "message", "tuple"] {
            hasher.update(fragments[key].as_str().expect("fragment").as_bytes());
            hasher.update([0_u8]);
        }

        assert_eq!(computed.digest, hex(&hasher.finalize()));
        assert_eq!(computed.messages, 1);
    }

    /// A missing anchor row is an error, never an empty digest.
    #[test]
    fn a_session_without_a_row_is_an_error() {
        let connection = fixture();
        let error = session_digest(&connection, "ses_missing").expect_err("must fail");
        assert!(
            matches!(&error, DigestError::MissingSession(id) if id == "ses_missing"),
            "unexpected error: {error}"
        );
    }

    /// A BLOB is out of contract: guessing an encoding would produce a
    /// digest nobody else can reproduce, so it has to be refused.
    #[test]
    fn a_blob_value_is_rejected_rather_than_guessed() {
        let connection = Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE session_v2 (id TEXT, payload BLOB);
                 INSERT INTO session_v2 VALUES ('ses_blob', x'00FF');",
            )
            .expect("fixture loads");

        let error = session_digest(&connection, "ses_blob").expect_err("must fail");
        assert!(
            matches!(
                &error,
                DigestError::BlobValue { table, column }
                    if table == "session_v2" && column == "payload"
            ),
            "unexpected error: {error}"
        );
    }

    /// The optional link tables are optional; a database that cannot be read
    /// at all is not. Swallowing every failure as "the table is not there"
    /// would silently drop a whole table out of the digest and still produce
    /// a well-formed 64-hex answer that verifies.
    #[test]
    fn only_a_missing_table_reads_as_absent() {
        let connection = fixture();
        assert!(
            !table_exists(&connection, "session_definitely_absent").expect("absent"),
            "no such table"
        );
        assert!(
            table_exists(&connection, "session_pending").expect("present"),
            "the fixture has session_pending"
        );

        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        std::io::Write::write_all(&mut file, b"this is not a database at all").expect("bytes");
        let broken = Connection::open(file.path()).expect("connection opens lazily");

        let error = table_exists(&broken, "session_pending").expect_err("must fail");
        assert!(
            matches!(error, DigestError::Sqlite(_)),
            "expected a sqlite error, got: {error}"
        );
    }
}
