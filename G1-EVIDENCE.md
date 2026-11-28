# Gate G1 — all 7644 existing receipts verify

The campaign (`2026-09-22-v2/`) stayed read-only during evaluation. This file is
the evidence slot outside the campaign; `PLAN-REVISION.md` inside it is unchanged.

## Executable

Built from branch `session-verifier`, worktree
`/data/scratch/tmp/opencode/chaosbox-session-verifier`, commit `1123b05`
plus its `test(sessions)` follow-up on top of trunk `33d7477` (after the
`feat!` that removed the Gel backend). The branch started at `03339f7` and
rebased onto `33d7477` with only a `Cargo.lock` conflict. The full gate ran
again after the rebase. See below for the re-run after verifier hardening.

- Binary: `target/release/chaosbox` (23640520 bytes, built 2026-09-23 15:51)
- Command: `chaosbox sessions verify --root .../canonical-staging-FP6mrj`
- Exit code: `0`, wall clock `93 s` (`elapsed_ms 93295`)
- The run opened the campaign read-only and wrote nothing to it.

A run from the pre-rebase build also reported `7644 / 7644 / complete`
(`elapsed_ms 74186`, `/data/scratch/tmp/opencode/verify-7644.json`); the
post-rebase run below is the one that counts.

## Result

| field | value |
| --- | --- |
| `sessions_effective` | 7644 |
| `sessions_checked` | 7644 |
| `sessions_verified` | 7644 |
| `complete` | `true` |
| `quick_check` | `ok` |
| `foreign_key_violations` | 0 |
| `chain_errors` | 0 (field absent) |
| `missing` | 0 (field absent) |
| `digest_mismatch` | 0 (field absent) |
| `message_count_mismatch` | 0 (field absent) |
| `source_mismatch` / `recovery_mismatch` / `source_errors` | 0 (absent) |
| `checked_sources` | `false` (deliberate — see below) |

Full report: `/data/scratch/tmp/opencode/verify-7644-final.json`.

## Campaign state read alongside it

| fact | value |
| --- | --- |
| receipts in `journal-v2` | 7644 (+ `identity.json`) |
| `progress.json` | `complete: true`, `integrityVerified: true`, `scannedAll: true`, `readyForCutover: false` |
| `progress.driverDigest` | `62f6fb3544875b4f22c63d3df315e2c7d5106eff21bedc26c7e1fd696b9f6604` |
| `progress` receipts | 7644 sessions, 0 supersessions |
| destination `session_v2` | 7644 |
| destination `session_message` | 560595 |
| destination `project_directory` / `worktree` | 0 / 0 |
| driver digest vs frozen `assemble-canonical-history.mjs` (sha256 `62f6fb35…`) | matches — G1's driver-side precondition holds |

Status report: `/data/scratch/tmp/opencode/status-7644.json`.

## Why this proves the encoder fix

An earlier run against the same receipts with the inherited encoder reported
`300 checked / 288 verified / 12 mismatch`
(`/data/scratch/tmp/opencode/verify300.json`): every failure was a `REAL`
`cost` in a range the encoder rendered wrongly. The two bugs were:

1. `0 < cost < 1` — `trim_end_matches('0')` on the fraction dropped leading
   zeros, so position was lost (`0.0001051064` became `0.1051064`).
2. `cost >= 1` with a fractional part — the inherited fix built `digits` from
   `rendered.trim_end_matches('0')`, which keeps the `.`, so the decimal-position
   split emitted a double dot (`7.3637754` became `7..3637754`).

Measured shape of all 7644 rows (all `REAL`, all `not-null`):

| range | rows |
| --- | --- |
| `zero` | 4072 |
| `0 < cost < 1` | 2803 |
| `>= 1` fractional | 769 |
| `>= 1` integer | 0 |

3572 of 7644 rows exercise a previously broken encoder path. All 3572 now
reproduce their JavaScript-produced digests byte-for-byte.

## Not covered by this gate

- `--sources` was not used: it re-derives `inputDigest` over the 27.7 GB
  `primary.db`, which belongs to gate G2/G3, not G1.
- `readyForCutover` stays `false` until gate G4.
- The 204 divergent variants are not materialized yet (Phase 2).

## Reproduce

```sh
cd /data/scratch/tmp/opencode/chaosbox-session-verifier
cargo clippy -p chaosbox --all-targets -- --deny warnings
cargo test -p chaosbox
cargo build --release -p chaosbox
./target/release/chaosbox sessions verify \
  --root /data/nvme0/can/ProjectState/opencode-migration/2026-09-22-v2/canonical-staging-FP6mrj
```

Expected: exit `0`, `sessions_verified: 7644`, `complete: true`,
`quick_check: "ok"`.

---

## Re-run after verifier hardening

The gate ran again after the verification contract (`docs/SESSION_VERIFICATION.md`)
and verifier hardening landed on the same branch. The campaign didn't change;
the verifier must prove more before it can return `0`.

### Executable

- Binary: `target/release/chaosbox` (23691800 bytes, built 2026-09-23 16:29)
- Command: unchanged — `chaosbox sessions verify --root .../canonical-staging-FP6mrj`
- Exit code: `0`, `elapsed_ms 102108` (up from `93295`: the pass now also
  reads every `session_v2` id and reconciles the inventory)
- The run opened the campaign read-only and wrote nothing to it.
- Report: `/data/scratch/tmp/opencode/verify-7644-hardened.json`

Then rebased onto trunk `dc30fef` (`fefa183` + `dc30fef`, the jev and
extract refactors — neither touches this crate) and re-run unchanged:
**identical to `verify-7644-hardened.json` field for field, except
`elapsed_ms 99430`** — that is, the hardened pass before the rebase versus
the hardened pass after it, not a comparison against the original
pre-hardening run above. Binary 23692488 bytes, built 2026-09-23 16:37,
report `/data/scratch/tmp/opencode/verify-7644-postrebase.json`.

### G1 fields, unchanged

| field | value |
| --- | --- |
| `sessions_effective` / `sessions_checked` / `sessions_verified` | 7644 / 7644 / 7644 |
| `complete` | `true` |
| `quick_check` | `ok` |
| `foreign_key_violations` | 0 |
| `checked_sources` | `false` (deliberate — G1 is destination-only) |
| `chain_errors` / `missing` / `digest_mismatch` / `message_count_mismatch` | all absent (empty) |
| `source_mismatch` / `recovery_mismatch` / `source_errors` | all absent (empty) |
| exit code | `0` |

### What the report can now also state

The pass now reports what it measured, not just what it found:

| field | value | means |
| --- | --- | --- |
| `inventory.reconciled` | `true` | inventory, receipts, and destination agree |
| `inventory.total` / `expected` / `receipts` / `destination_rows` | 7644 each | the four sets reconcile, all four directions |
| `inventory.deferred` / `errors` | 0 / 0 | the driver left nothing undone |
| `inventory.progress_complete` | `true` | the driver finished the run |
| `inventory.identity_digest` | `9ad916d8…` | which campaign identity was checked |
| `inventory.driver_digest` | `62f6fb35…` | the frozen driver, G1's precondition |
| `inventory.missing_receipts` / `unexpected_receipts` / `absent_destination` / `unexpected_destination` | all empty | each checked in both directions |
| `snapshot.consistent_read` | `true` | all reads ran in one SQLite snapshot |
| `snapshot.data_version_before` / `data_version_after` | 2 / 2 | the generation the pass read |
| `snapshot.concurrent_write` | `false` | nobody committed while it ran |
| `source_coverage` | `0` | sources were not requested, and the report says so |

The report now states G1's original criteria directly: reconciled totals,
`deferred == 0`, `errors == 0` and driver digest `62f6fb35…`. Readers no longer
have to infer them from `progress.json`.

### Paths this closed

Each let the verifier return `0` without verifying its claims. Each now has a test:

- **Vacuous completeness.** Completeness was measured against the receipts
  the pass discovered, so an empty journal over a populated inventory passed.
  Now reconciled against `progress.json`, and `an_empty_journal_cannot_pass_...`
  shows `--allow-partial` does not rescue it.
- **Unchecked set directions.** The report now distinguishes missing receipts,
  absent destination rows and unattested destination rows across the whole
  inventory, regardless of `--limit`.
- **Mistyped `supersedes`.** A non-string value read as "absent", turning a
  supersession into a base receipt with a competing set of attestation fields.
  Now `InvalidField`.
- **Path traversal via `source`.** `source` was joined into a path
  (`work/<source>.db`) with no shape check, so a receipt could aim
  verification at any `.db`. Now restricted to a bare database name — a
  pre-existing requirement, since all 7644 real receipts already use
  `primary` / `stable` / `old-backup` / `local` / `quarantine` / `canary`.
- **`--sources` silently skipping receipts.** The verifier didn't count coverage,
  so it skipped receipts without usable `source` metadata without reporting
  them. Now `source_coverage` must equal `sessions_checked`.
- **Multi-file consistency.** `quick_check`, the foreign-key walk, and the
  digests each read whatever the database looked like at that moment. Now one
  read transaction per database, with the generation reported.
- **`table_exists` swallowing errors.** Every failure was read as "table
  absent", which would silently drop a whole table out of a digest and still
  emit a well-formed 64-hex number that verifies. Now only
  `QueryReturnedNoRows` means absent.
- **Exit codes assumed rather than tested.** `cli::run` was called with an
  already-built `Command`, skipping clap and `main` entirely.
  `tests/sessions_cli.rs` runs the real binary for each exit path, including
  `2` for a usage error — a typo must not look like a bad campaign.

### Checks

- `cargo clippy -p chaosbox --all-targets -- --deny warnings` → **0**
- `cargo fmt -p chaosbox -- --check` → **0**
- `cargo test -p chaosbox` → **101 passed, 1 pre-existing ignored**
  (was 70 passed: +27 — inventory reconciliation, receipt schema,
  source coverage, snapshot behaviour, and real-binary exit codes)

The campaign's receipts passed the stricter schema before enforcement: all 7644 carry
`sessionID`, `source`, `inputDigest`, `recoveryDigest`, `destinationDigest`,
`transformation`, `messages`, `drafts` with correct types and 64-lowercase-hex
digests — **0 contract violations**. The stricter rules reject no existing
receipt and constrain only the 204 variants Phase 2 will write.

---

## Re-run after the inventory closeout

A review found that missing `total`, `deferred` and `errors` had defaults:
`total` used the number of `verified` entries; the other two used zero. An
inventory with no counters could then reconcile and pass with `--allow-partial`.
All three counters are now required. If present, `complete` must be a boolean
rather than falling back to `false`; each of `identityDigest`/`driverDigest`, if
present, must be 64 lowercase hex digits.

The campaign passed the new rules *before* enforcement. Its `progress.json`
carries all six fields in the forms the tightened parser expects.

The same review also corrected two overstatements in this document and in
`docs/SESSION_VERIFICATION.md`: `PRAGMA data_version` is a per-connection
change counter rather than a generation identity for the database, and the
comparison recorded above is hardened-pre-rebase against
hardened-post-rebase, not against the original pre-hardening run.

### Executable

- Binary: `target/release/chaosbox` (23693168 bytes, built 2026-09-23 17:15)
- Command: unchanged — `chaosbox sessions verify --root .../canonical-staging-FP6mrj`
- Exit code: `0`, `elapsed_ms 100024`
- The run opened the campaign read-only and wrote nothing to it.
- Report: `/data/scratch/tmp/opencode/verify-7644-closeout.json`
- **Identical to `verify-7644-postrebase.json` field for field except
  `elapsed_ms 99430 → 100024`** (24 fields compared).

### G1 criteria, still met

| criterion | value |
| --- | --- |
| reconciled totals | `true` — `total = expected = receipts = destination_rows = 7644` |
| `deferred` / `errors` | 0 / 0, both **read** rather than defaulted |
| driver digest | `62f6fb3544875b4f22c63d3df315e2c7d5106eff21bedc26c7e1fd696b9f6604` |
| sessions effective / checked / verified | 7644 / 7644 / 7644 |
| `complete` | `true` |
| `quick_check` / foreign-key violations | `ok` / 0 |
| exit code | `0` |
| `checked_sources` | `false` (deliberate — G1 is destination-only) |

### Checks

- `cargo fmt -p chaosbox -- --check` → **0**
- `cargo clippy -p chaosbox --all-targets -- --deny warnings` → **0**
- `cargo test -p chaosbox` → **104 passed, 1 pre-existing ignored**
  (101 → 104: a missing counter cannot be allowed partial, a mistyped
  `complete` is refused rather than read as `false`, and a present but
  malformed campaign digest is refused)

### Status of this branch

**Not integrated.** Trunk is `dc30fef`; `session-verifier` sits on top of it
and has not been merged. The shared checkout
(`/data/nvme0/can/canix/projects/repos/owned/chaosbox`) holds *untracked*
copies of `crates/chaosbox/src/sessions/`, `tests/sessions*.rs` and
`docs/SESSION_VERIFICATION.md` synced from an earlier tip — whoever lands
this branch must reconcile those against it rather than delete them blind,
since other sessions may have edited them since.

The campaign directory `2026-09-22-v2/` remains read-only and untouched;
this file stays outside it as G1's evidence slot.
