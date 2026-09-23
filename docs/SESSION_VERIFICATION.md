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

`deferred`, `errors`, and a `total` that does not match `verified` are
reported and fail reconciliation: they mean the driver left work undone.

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
`transformation` (string), `version` (positive integer).

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

### Destination and source identities

The same `ses_*` id is the primary key in the destination, in every source
snapshot, and in `recovered-rows.db`. There is no mapping table and no
renaming: a session is either present under that id or absent, which is what
makes `missing`, `source_errors`, and `absent_destination` meaningful as
three separate outcomes.

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
- `PRAGMA data_version` is read before the snapshot opens and after it
  commits. A change between the two is `concurrent_write: true`.

`concurrent_write` is reported, not failed. The pass itself stayed
consistent — it read one snapshot and everything in the report describes
those bytes — but the destination has moved since, so the result is stale
and should be re-run before anyone relies on it.

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

`clean()` additionally requires `source_coverage == sessions_checked` whenever
`--sources` was requested, so a partially covered source pass cannot pass.
`sources_uncovered` lists any session skipped for want of receipt metadata;
receipt validation should keep it empty, and it exists so a future schema
change cannot turn "no metadata" into "not checked".

### Failure lists

`chain_errors`, `missing`, `digest_mismatch`, `message_count_mismatch`,
`source_mismatch`, `recovery_mismatch`, `source_errors`, and
`sources_uncovered` — all must be empty, along with `quick_check == "ok"` and
`foreign_key_violations == 0`, for the pass to be clean.

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
