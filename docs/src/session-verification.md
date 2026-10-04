# Session verification contract

What `chaosbox sessions verify` checks, what it refuses to check, and what a
passing run is allowed to claim. The code enforces every rule here; this file
is the statement of them so a gate can be evaluated against the report
without reading the source.

Both subcommands are read-only. Verification opens every database with
`SQLITE_OPEN_READ_ONLY` and never writes to a campaign directory.

---

## 1. The pinned inventory

A pass needs an expectation that does not come from the thing being checked.
Comparing the destination only to the receipts it discovered makes an empty
journal pass: there is nothing to disagree with.

So the expectation comes from `progress.json`, which the migration driver
writes before and during its run:

| field | meaning |
| --- | --- |
| `total` | sessions the driver expected to migrate |
| `verified[].id` | one entry per session it actually migrated |
| `deferred` | sessions it deliberately left behind |
| `errors` | sessions it could not migrate |
| `complete` | whether the driver finished |
| `identityDigest` | pins the campaign identity |
| `driverDigest` | pins the frozen migration script |

`verified` must be an array of objects each naming one distinct session, and
every id must be a real session id. Anything else is `InvalidProgress`, and
the pass reports it instead of inventing an inventory.

`total`, `deferred` and `errors` are **required**. They are the pass's only
statement about work it did not do, and they are precisely what a default
would invent: `total` defaulted to the number of `verified` entries makes an
inventory that reported no total agree with itself, while the other two
defaulted to zero report work as never having been reported at all. A
missing counter is `InvalidProgress`, which leaves `reconciled` false — and
because `clean()` includes reconciliation, the pass fails even under
`--allow-partial`.

`deferred` and `errors` may be written either as a bare count or as the
array of sessions behind them. The elements are counted, not interpreted: a
non-empty array already fails reconciliation, so their shape never decides
whether the campaign agrees with itself.

`complete` may be absent, which reads as "not finished" and is shown as
`progress_complete: false`; when present it must be a boolean, so a
mistyped value is refused instead of decaying into `false`. The two digests
may be absent but must be 64 lowercase hex digits when present.

Those digests are **reported, not verified**. The pass echoes what
`progress.json` pinned; it does not re-derive either one from the artifact
it names. Comparing `driverDigest` against the frozen
`assemble-canonical-history.mjs` is therefore a check G1 performs in its own
evidence — a clean report does not imply it happened.

A `total` that does not match `verified`, or a non-zero `deferred` or
`errors`, is reported and fails reconciliation: they mean the driver left
work undone.

### Beyond the canonical run

After the canonical run, two more pins join the expectation — never by
weakening reconciliation, only by letting each new receipt trace to the pin
that sanctions it:

- **Variants** (`variants_expected`): `journal-v3/identity-v3.json` pins
  `mappingDigest` over the staged `variants-mapping.json`. The pass
  recomputes that digest and reads the sanctioned session ids out of the
  mapping. A variant receipt for a session the mapping does not sanction is
  `unexpected_receipts`, exactly like a canonical one.
- **Delta-new** (`delta_expected`): `journal-v3/identity-delta.json` chains
  onto the campaign's actual `journal-v3/identity-v3.json` — every chain
  field present must name that file (not an arbitrary path) with the SHA-256
  it still hashes to, and at least one must be present — and pins
  `deltaInventory` (path plus SHA-256 over the delta file). The pass verifies
  all hashes, requires the delta file's `status` be `final` (preliminary
  inventories move as the stores grow and sanction nothing), requires its
  `derivedFrom.reconciliation.sha256` equal the variant identity's
  `reconciliationDigest`, and reads `deltaNew.ids` out of the pinned file,
  rejecting duplicates and non-session ids. A delta without its variant
  identity, a chain pointing elsewhere, a tampered file, a preliminary
  status, a drifting reconciliation, or a duplicated id is a hard `Mapping`
  error, never a quiet fallback to no delta. Zero until the delta identity
  lands.

All four set differences are computed against the **union** of the three
pins. A receipt for a session none of them sanctions is still an error; the
union only admits receipts that trace to a pin.

---

## 2. The receipt schema

Receipts are immutable. A base receipt lives in `journal-v2`; a session whose
contents changed gets a newer record in a later journal with `supersedes`
naming the one it replaces. Exactly one receipt per session is unreferenced —
that is the effective receipt verification reads.

**Required on every receipt:**

| field | type | rule |
| --- | --- | --- |
| `sessionID` | string | must equal the session the file name promises |
| `source` | string | bare database name: letters, digits, `_`, `-` only |
| `inputDigest` | string | 64 lowercase hex digits |
| `recoveryDigest` | string | 64 lowercase hex digits |
| `destinationDigest` | string | 64 lowercase hex digits |
| `messages` | integer | `>= 0` |

**Optional, validated when present:** `supersedes` (a
`journal-*/<receipt>.json` address — a value of any other type is an error,
never a receipt read as a base record), `drafts` (non-negative integer),
`transformation` (string), `version` (positive integer), `kind` (one of
`"divergent-variant"`, `"supersession"`, `"delta-new"`),
`sourceSessionID` (a session id), `idTransformation` (object with string
`session` and `message`), `idAttempt` (non-negative integer),
`messageIDMapDigest` (64 lowercase hex digits).

Per-kind requirements, on top of the shared fields:

| kind | rule |
| --- | --- |
| `divergent-variant` | requires `sourceSessionID`, `idTransformation`, `idAttempt`, `messageIDMapDigest` |
| `supersession` | requires `supersedes` — a supersession that replaces nothing is meaningless |
| `delta-new` | forbids `supersedes` and `sourceSessionID` — it attests to its own session |

Receipt file names carry a generation: `<id>.json` is generation 1,
`<id>.<N>.json` is generation N. The suffix must be `[1-9][0-9]*` — no zero,
no leading zeros — so `<id>.0.json` is not a receipt name at all.

Two failure kinds, deliberately distinct: `MissingField` means *not
attested*, `InvalidField` means *attested wrongly*. `supersedes` is the case
that matters most — an absent field and a mistyped one used to be
indistinguishable, which turned a supersession into a base receipt.

`source` is validated as a bare name because [`Campaign::source`] joins it
into a path. Allowing a separator there would let a receipt point
verification at any file ending in `.db`.

Chain rules on top of the schema: a `supersedes` target must exist
(`DanglingSupersession`) and belong to the same session
(`ForeignSupersession`); the chain must reach a single unreferenced head
(`AmbiguousEffective`, `CyclicChain`, `DisconnectedChain`).

Filename/link agreement (R5), checked at validation before any chain
resolution: `journal-v2` receipts never carry `supersedes` — that journal is
append-immutable. A plain `journal-v3/<id>.json` may target nothing (a head)
or exactly `journal-v2/<id>.json`. `<id>.<N>.json` (N ≥ 2) must target
`<id>.<N-1>.json`, where generation 1 is the plain file. A mislinked receipt
reports `LinkAgreement` rather than the ambiguity its bad link would also
produce. Consequence, recorded deliberately: cycles and disconnected chains
cannot form inside `journal-v2`/`journal-v3` anymore, so those rules survive
as defense-in-depth for any other `journal-*` directory.

`supersedes` is a receipt **path**, not a digest: `journal-v2/<file>` or
`journal-v3/<file>`. Both verifier implementations use addresses, the chain
fixtures pin that shape, and the R5 rule above is only statable over paths.

### Destination and source identities

The same `ses_*` id is the primary key in the destination, in every source
snapshot, and in `recovered-rows.db` — except for variants. A variant receipt
attests to a derived session but carries `sourceSessionID` naming the session
it derives from, and `--sources` recomputes `inputDigest`/`recoveryDigest`
against that session, not the attested one. There is still no mapping table:
a session is either present under the id the receipt names or absent, which
is what makes `missing`, `source_errors`, and `absent_destination`
meaningful as three separate outcomes.

Changed and delta-new sessions attest to frozen boundary snapshots, not to
the work snapshots. Their `provenance` names the boundary, and
`provenance.boundaryRecordSha256` binds the receipt to the exact record
bytes: SHA-256 over the canonical-JSON stringification of the parsed
boundary record, matching the merge driver's `digest(boundary)`. Binding is
mandatory on every pass, with or without `--sources`: a `supersession` or
`delta-new` receipt without that pin is `sources_uncovered`, and one naming
an unknown boundary or a malformed digest is a `source_error`. Only a
receipt whose pin is present and well-formed proceeds; under `--sources` its
value is then compared to the record on disk (mismatch is a `source_error`)
and its source digests are recomputed against the snapshot the record names.
That record may carry a converted snapshot alongside the raw one: the raw
file is the frozen bytes the holder gate cleared, while the converted file
is the same bytes after the candidate backfilled `session_v2` rows into a
copy (pinned separately because the copy diverges by design). Both hashes
are verified when a converted snapshot is present, and recomputation reads
the converted file — `inputDigest` attests to converted content, never to
the raw bytes.

### Tool closure and adoption

`install` and `rollback` resolve their scripts out of the campaign's
`tools.json` and verify the digest immediately before use. The pin is a
closure, not a single entrypoint: every file listed is re-hashed on every
resolution, and the requested tool's relative `./`/`../` imports must
resolve to pinned files. A tampered transitive import or an unpinned import
refuses to run.

`adopt` pins a store as a digest-addressed input without writing to it. It
refuses held stores (before hashing and after opening; an unreadable `/proc`
is a scan failure, not a clear scan), refuses stores with `-wal`/`-shm`/
`-journal` sidecars (adoption never checkpoints, so a store with sidecars is
not frozen), requires `v2` stores to carry both `session_v2` and
`session_message`, and refuses when the file's size or mtime moves between
the hash and the read view — the hash, counts, and health checks must
describe the same bytes.

### Variant re-key proofs (`--sources` only)

For a `divergent-variant` receipt the pass additionally re-derives, from
receipt fields and database row order rather than from the driver's mapping:

- the session id from `(source, sourceSessionID, idAttempt)` (`session-id`);
- the two-sided re-key digest — SHA-256 over `original NUL derived newline`
  for each message pair in `(seq, id)` order — against
  `messageIDMapDigest` (`message-map`);
- every message id at attempt 0 with native shape (`message-pair`);
- `idTransformation` against the variant identity (`transformation`);
- the receipt's message count against its pinned mapping entry
  (`mapping-entry`).

Failures land in `remap_mismatch` and fail `clean()`. The mapping pin itself
(`mappingDigest` recomputed over the staged mapping, `mappingFile` required
to name exactly that path) is verified once per pass, and variant receipts
without `journal-v3/identity-v3.json` are a hard error rather than a
skipped check.

---

## 3. Consistency mode

Each database the pass reads is opened read-only and put inside a read
transaction before the first query:

- The transaction is opened with one read of `sqlite_master`, because `BEGIN`
  is deferred and SQLite takes no snapshot until the transaction actually
  reads. Without that the snapshot would not exist yet at the moment
  `data_version_before` was recorded.
- `quick_check`, `foreign_key_check`, and all session digests run against
  **one snapshot** of the destination, reported as `consistent_read`.
- Each source snapshot and `recovered-rows.db` get their own transaction, so
  an `inputDigest` recomputation does not straddle a rewrite of a 27 GB file.
- `PRAGMA data_version` is read before the transaction opens and again after
  it commits. It is a **per-connection change counter**, not an identifier
  for the database: unrelated databases share its values and it resets with
  the connection. What it can say is whether *this* connection observed
  another connection commit to this file between those two reads, which is
  reported as `snapshot.concurrent_write`.
- Because `data_version_before` is taken before `BEGIN`, it is not sampled
  atomically with the snapshot. Reads *inside* the transaction all see one
  snapshot; only the bookend comparison is approximate, which is a second
  reason it is reported rather than gated.

`concurrent_write` is reported, not failed. The pass itself stayed
consistent — every number in the report describes the one snapshot it read —
but a `true` means the destination has moved since, so the result is stale
and should be re-run before anyone relies on it. A `false` means no external
commit was observed on that path and nothing more: it is not a generation
identity for the database, and it does not travel with the report the way a
digest does.

---

## 4. Report semantics

A successful check states its inventory, its properties, and its coverage.

### Inventory reconciliation

Four set differences, each a distinct outcome:

| field | means |
| --- | --- |
| `missing_receipts` | the inventory expects a session the journals never attest to |
| `unexpected_receipts` | a receipt exists for a session the inventory never sanctioned |
| `absent_destination` | a receipt has no `session_v2` row behind it |
| `unexpected_destination` | the destination holds a session no receipt accounts for |

All four, plus `deferred == 0`, `errors == 0`, and `total == expected`, give
`inventory.reconciled: true`. This is checked over the **whole** inventory,
not only the sessions this pass selected, so a bounded pass still fails if
the campaign as a whole does not reconcile.

### Coverage

| field | means |
| --- | --- |
| `sessions_effective` | sessions the journals resolve to |
| `sessions_checked` | sessions whose destination digest was recomputed |
| `sessions_verified` | checked sessions whose digest **and** message count matched |
| `complete` | inventory reconciled, driver finished, and every effective receipt was selected |
| `source_coverage` | checked sessions whose source **and** recovery digests were recomputed |
| `variants_expected` / `delta_expected` | sessions sanctioned by the variant mapping / delta inventory pins |
| `driver_digests` | distinct `driverDigest` values found per journal — reported, never compared in-band; the gate compares them against the pinned driver |

`clean()` additionally requires `source_coverage == sessions_checked` whenever
`--sources` was requested, so a partially covered source pass cannot pass.
`sources_uncovered` lists any session skipped for want of receipt metadata;
receipt validation should keep it empty, and it exists so a future schema
change cannot turn "no metadata" into "not checked".

### Failure lists

`chain_errors`, `missing`, `digest_mismatch`, `superseded_unverified`,
`message_count_mismatch`, `source_mismatch`, `recovery_mismatch`,
`remap_mismatch`, `source_errors`, and `sources_uncovered` — all must be
empty, along with `quick_check == "ok"` and `foreign_key_violations == 0`,
for the pass to be clean.

`digest_mismatch` versus `superseded_unverified`: a digest disagreement on a
session whose chain depth is 1 means its base attestation is broken. The
same disagreement at depth above 1 means the session changed again since
its head receipt was written and needs re-verification. Both fail the pass;
the bucket tells the operator which failure they are looking at.

`clean()` is about *what was checked*. `complete` is about *how much was
selected*. `succeeded` requires `clean()` plus either `complete` or an
explicit `--allow-partial`.

### Exit codes

| code | meaning |
| --- | --- |
| `0` | clean, and complete — or clean, bounded, and `--allow-partial` |
| `1` | clean-but-incomplete without `--allow-partial`, or any failure, or a campaign that could not be read |
| `2` | the command line itself was rejected |

A campaign that cannot be read at all is an error on stderr with no report,
not a failing report: the operator needs to know the difference between
"verification ran and failed" and "verification could not run".

---

## 5. What this does not establish

- **Source coverage is opt-in.** Without `--sources`, `inputDigest` and
  `recoveryDigest` are reported as unchecked (`checked_sources: false`) and
  are not part of `clean()`. Only the destination digest is attested.
- **`readyForCutover` is read, never set.** It stays `false` until its own
  gate; verification does not turn it on.
- **No float-equivalence claim beyond this schema.** The encoder reproduces
  JavaScript `JSON.stringify` for the value shapes the supported SQLite schema
  emits — text, integers within and beyond 2^53, and finite reals — proved
  against a machine-generated reference table, not against all possible
  doubles.
- **Digests are only as good as their receipts.** Verification re-derives what
  a receipt claims; it cannot tell whether the receipt was honestly produced.
  That is what pinning `driverDigest` against the frozen
  `assemble-canonical-history.mjs` is for.
