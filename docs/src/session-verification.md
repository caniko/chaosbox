# Session verification contract

Use this contract to evaluate a gate against the `chaosbox sessions verify`
report without reading the source. The code enforces these rules: what gets
checked, what verification refuses to check, and what a pass can claim.

Both subcommands are read-only. Verification opens every database with
`SQLITE_OPEN_READ_ONLY` and never writes to a campaign directory.

---

## 1. The pinned inventory

A pass needs an independent inventory. Comparing the destination only to
discovered receipts lets an empty journal pass by leaving nothing to dispute.

The migration driver writes that inventory to `progress.json` before and
during its run:

| field | meaning |
| --- | --- |
| `total` | sessions the driver expected to migrate |
| `verified[].id` | one entry per session it actually migrated |
| `deferred` | sessions it deliberately left behind |
| `errors` | sessions it could not migrate |
| `complete` | whether the driver finished |
| `identityDigest` | pins the campaign identity |
| `driverDigest` | pins the frozen migration script |

`verified` must be an array of objects, each naming a distinct, real session
id. The pass reports anything else as `InvalidProgress` instead of inventing
an inventory.

`total`, `deferred` and `errors` are **required**: these counters are the
pass's only account of work not done. Defaulting `total` to the number of
`verified` entries would make an inventory with no total agree with itself;
defaulting the other counters to zero would hide unreported work. A missing
counter is `InvalidProgress`, leaving `reconciled` false. Since `clean()`
requires reconciliation, the pass fails even under `--allow-partial`.

`deferred` and `errors` can be bare counts or arrays of sessions. The pass
counts array elements without interpreting them: any non-empty array fails
reconciliation regardless of element shape.

An absent `complete` means "not finished" and appears as
`progress_complete: false`. If present, it must be boolean: a mistyped value
is rejected, not treated as `false`. The two optional digests must each be
64 lowercase hex digits when present.

The pass **reports but does not verify** the digests pinned in `progress.json`.
It does not re-derive them from the named artifacts. G1 independently compares
`driverDigest` against the frozen `assemble-canonical-history.mjs` in its own
evidence; a clean report does not imply that check ran.

The pass reports a `total` that disagrees with `verified` or a nonzero
`deferred` or `errors`. Each fails reconciliation because the driver left work
undone.

### Beyond the canonical run

After the canonical run, two more pins extend the inventory without weakening
reconciliation. Each new receipt must trace to the pin that authorizes it:

- **Variants** (`variants_expected`): `journal-v3/identity-v3.json` pins
  `mappingDigest` over the staged `variants-mapping.json`. The pass
   recomputes the digest and reads the authorized session ids from the mapping.
   An unauthorized variant receipt goes in `unexpected_receipts`, just like a
   canonical one.
- **Delta-new** (`delta_expected`): `journal-v3/identity-delta.json` must
  carry at least one chain field. Each must name the campaign's actual
  `journal-v3/identity-v3.json` with its current SHA-256, not another path.
  The identity pins `deltaInventory` (path plus SHA-256 over the delta file).
  Verification checks every hash, requires the pinned delta file's `status` to
  be `final`, and matches `derivedFrom.reconciliation.sha256` against the
  variant identity's `reconciliationDigest`. It reads `deltaNew.ids` from the
  pinned file and rejects duplicates and non-session ids. Preliminary
  inventories change as stores grow and authorize nothing. A delta identity
  without its variant identity, wrong chain path, tampered file, preliminary
  status, reconciliation drift or duplicate id is a hard `Mapping` error, not a
  fallback to zero. The count is zero until the delta identity lands.

The pass computes all four set differences against the **union** of the three
pins. A receipt that traces to none of them is still an error.

---

## 2. The receipt schema

Receipts are immutable. The base receipt lives in `journal-v2`. For a changed
session, a later journal records a new receipt with `supersedes` naming the
receipt it replaces. Verification reads the one unreferenced receipt per session
as the effective receipt.

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

`MissingField` means *not attested*; `InvalidField` means *attested wrongly*.
The distinction matters most for `supersedes`: treating a mistyped field as
absent used to turn a supersession into a base receipt.

Verification requires a bare name for `source` because [`Campaign::source`]
joins it into a path. A separator could point verification at any file
ending in `.db`.

Chain rules on top of the schema: a `supersedes` target must exist
(`DanglingSupersession`) and belong to the same session
(`ForeignSupersession`); the chain must reach a single unreferenced head
(`AmbiguousEffective`, `CyclicChain`, `DisconnectedChain`).

Filename/link agreement (R5), checked at validation before any chain
resolution: `journal-v2` receipts never carry `supersedes` — that journal is
append-immutable. A plain `journal-v3/<id>.json` may target nothing (a head)
or exactly `journal-v2/<id>.json`. `<id>.<N>.json` (N ≥ 2) must target
`<id>.<N-1>.json`, where generation 1 is the plain file. A mislinked receipt
reports `LinkAgreement`, not the resulting ambiguity. These constraints rule
out cycles and disconnected chains inside `journal-v2`/`journal-v3`; the chain
rules still protect any other `journal-*` directory.

`supersedes` is a receipt **path**, not a digest: `journal-v2/<file>` or
`journal-v3/<file>`. Both verifiers use addresses, the chain fixtures pin that
shape, and the R5 rule requires paths.

### Destination and source identities

Except for variants, the destination, every source snapshot and
`recovered-rows.db` share the same `ses_*` primary key. A variant receipt
attests to a derived session but names its original in `sourceSessionID`.
With `--sources`, verification recomputes `inputDigest`/`recoveryDigest`
from the original, not the derived session. There is no mapping table:
a session is present under the named id or absent. This keeps `missing`,
`source_errors`, and `absent_destination` distinct.

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
The record can also name a converted snapshot. The raw file holds the frozen
bytes cleared by the holder gate. The candidate copies it and backfills
`session_v2` rows; the boundary record pins that converted copy separately
because it diverges by design. When a converted snapshot is present,
verification checks both hashes and recomputes from the converted file:
`inputDigest` attests to converted content, not raw bytes.

### Tool closure and adoption

`install` and `rollback` resolve scripts from the campaign's `tools.json`
and verify the digest just before use. The pin covers the full closure:
every resolution re-hashes every listed file, and the tool's relative
`./`/`../` imports must resolve to pinned files. Tampered transitive imports
or unpinned imports prevent execution.

`adopt` pins a store by digest without writing to it. It rejects held stores
before hashing and after opening; an unreadable `/proc` fails the scan. It
rejects `-wal`/`-shm`/`-journal` sidecars because adoption never checkpoints
and a store with sidecars is not frozen. It requires both `session_v2` and
`session_message` in `v2` stores. It rejects changes in size or mtime between
hashing and the read view: hash, counts and health checks must describe the
same bytes.

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

Failures go in `remap_mismatch` and fail `clean()`. Once per pass, verification
recomputes `mappingDigest` over the staged mapping and requires `mappingFile`
to name that exact path. Variant receipts without `journal-v3/identity-v3.json`
are a hard error, not a skipped check.

---

## 3. Consistency mode

The pass opens each database read-only and starts a read transaction before
the first query:

- One read of `sqlite_master` opens the transaction: `BEGIN` is deferred, and
  SQLite takes no snapshot until the first read. Without it the snapshot would
  not yet exist when `data_version_before` was recorded.
- `quick_check`, `foreign_key_check`, and all session digests run against
  **one snapshot** of the destination, reported as `consistent_read`.
- Each source snapshot and `recovered-rows.db` get their own transaction, so
  an `inputDigest` recomputation does not straddle a rewrite of a 27 GB file.
- The pass reads `PRAGMA data_version` before the transaction opens and after
  it commits. It is a **per-connection change counter**, not a database
  identifier: unrelated databases share values, and it resets with the
  connection. `snapshot.concurrent_write` reports whether this connection
  observed another connection commit to this file between the two reads.
- Because `data_version_before` is taken before `BEGIN`, it is not sampled
  atomically with the snapshot. Reads *inside* the transaction all see one
  snapshot; only the bookend comparison is approximate, which is a second
  reason it is reported rather than gated.

`concurrent_write` is informational. Every result still describes one
consistent snapshot, but `true` means the destination has since changed.
Rerun verification before relying on a stale result. `false` means only
that the reader observed no external commit on that path. It neither
identifies the database generation nor travels with the report like a digest.

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

With `--sources`, `clean()` also requires `source_coverage == sessions_checked`:
partial source coverage fails. `sources_uncovered` lists sessions skipped for
missing receipt metadata. Receipt validation should keep it empty; the list
prevents future schema changes from turning "no metadata" into "not checked".

### Failure lists

For a clean pass, `chain_errors`, `missing`, `digest_mismatch`,
`superseded_unverified`, `message_count_mismatch`, `source_mismatch`,
`recovery_mismatch`, `remap_mismatch`, `source_errors`, and `sources_uncovered`
must all be empty.
The pass also requires `quick_check == "ok"` and `foreign_key_violations == 0`.

`digest_mismatch` versus `superseded_unverified`: at chain depth 1, a digest
disagreement means the base attestation is broken. Above depth 1, it means
the session changed again after the head receipt and needs re-verification.
Both fail the pass; the list tells the operator which occurred.

`clean()` is about *what was checked*. `complete` is about *how much was
selected*. `succeeded` requires `clean()` plus either `complete` or an
explicit `--allow-partial`.

### Exit codes

| code | meaning |
| --- | --- |
| `0` | clean, and complete — or clean, bounded, and `--allow-partial` |
| `1` | clean-but-incomplete without `--allow-partial`, or any failure, or a campaign that could not be read |
| `2` | the command line itself was rejected |

If verification cannot read the campaign at all, it writes an error to stderr
and emits no report. That distinguishes a check that could not run from one
that ran and failed.

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
