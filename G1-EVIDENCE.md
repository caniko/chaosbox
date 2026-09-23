# Gate G1 — all 7644 existing receipts verify

Recorded outside the campaign directory, because the campaign
(`2026-09-22-v2/`) is treated as read-only while this gate was evaluated.
`PLAN-REVISION.md` inside the campaign was left untouched; this file is its
evidence slot.

## Executable

Built from branch `session-verifier`, worktree
`/data/scratch/tmp/opencode/chaosbox-session-verifier`, commit `1123b05`
plus its `test(sessions)` follow-up on top of trunk `33d7477` (after the
`feat!` that removed the Gel backend). The branch was originally cut from
`03339f7` and rebased onto `33d7477`; only `Cargo.lock` conflicted, and this
gate was re-run in full after the rebase. A further re-run after verifier
hardening is recorded at the end of this file.

- Binary: `target/release/chaosbox` (23640520 bytes, built 2026-09-23 15:51)
- Command: `chaosbox sessions verify --root .../canonical-staging-FP6mrj`
- Exit code: `0`, wall clock `93 s` (`elapsed_ms 93295`)
- The campaign was opened read-only; nothing in it was written.

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

So 3572 of 7644 rows exercise an encoder path that was previously wrong, and
all 3572 now reproduce their JavaScript-produced digests byte-for-byte.

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

The same gate, re-run after the verification contract
(`docs/SESSION_VERIFICATION.md`) and the verifier-hardening work landed on
the same branch. Nothing about the campaign changed; what changed is what the
pass has to prove before it may answer `0`.

### Executable

- Binary: `target/release/chaosbox` (23691800 bytes, built 2026-09-23 16:29)
- Command: unchanged — `chaosbox sessions verify --root .../canonical-staging-FP6mrj`
- Exit code: `0`, `elapsed_ms 102108` (up from `93295`: the pass now also
  reads every `session_v2` id and reconciles the inventory)
- The campaign was opened read-only; nothing in it was written.
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

The pass used to say only what it found. It now says what it measured:

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

G1's original criteria — reconciled totals, `deferred == 0`, `errors == 0`,
driver digest `62f6fb35…` — are now all *in* the report rather than being
inferred by a human from `progress.json` alongside it.

### Paths this closed

Each was a way for a pass to answer `0` without having verified what it
claimed; each now has a test:

- **Vacuous completeness.** Completeness was measured against the receipts
  the pass discovered, so an empty journal over a populated inventory passed.
  Now reconciled against `progress.json`, and `an_empty_journal_cannot_pass_...`
  shows `--allow-partial` does not rescue it.
- **Unchecked set directions.** Missing receipts, absent destination rows,
  and unattested destination rows are now three distinct reported outcomes
  over the whole inventory, independent of `--limit`.
- **Mistyped `supersedes`.** A non-string value read as "absent", turning a
  supersession into a base receipt with a competing set of attestation fields.
  Now `InvalidField`.
- **Path traversal via `source`.** `source` was joined into a path
  (`work/<source>.db`) with no shape check, so a receipt could aim
  verification at any `.db`. Now restricted to a bare database name — a
  pre-existing requirement, since all 7644 real receipts already use
  `primary` / `stable` / `old-backup` / `local` / `quarantine` / `canary`.
- **`--sources` silently skipping receipts.** Coverage was not counted, so a
  receipt without usable `source` metadata was skipped without appearing
  anywhere. Now `source_coverage` must equal `sessions_checked`.
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

Receipt schema conformance was measured against the campaign itself before
the stricter validation was enforced: all 7644 receipts already carry
`sessionID`, `source`, `inputDigest`, `recoveryDigest`, `destinationDigest`,
`transformation`, `messages`, `drafts` with correct types and 64-lowercase-hex
digests — **0 contract violations**. So the stricter rules reject no receipt
that exists today; they only constrain the 204 variants Phase 2 will write.

---

## Re-run after the inventory closeout

A review found that `total`, `deferred` and `errors` were defaulted when
absent — `total` to the number of `verified` entries, the other two to zero.
That defaulting manufactures the very agreement those counters exist to
measure: an inventory reporting none of them would still have reconciled,
and `--allow-partial` would have waved it through. They are now required,
`complete` must be a boolean when present rather than decaying into `false`,
and `identityDigest`/`driverDigest` must be 64 lowercase hex digits when
present.

The campaign was checked against the new rules *before* they were enforced:
its `progress.json` carries all six fields with the expected shapes, so the
tightened parser accepts it.

The same review also corrected two overstatements in this document and in
`docs/SESSION_VERIFICATION.md`: `PRAGMA data_version` is a per-connection
change counter rather than a generation identity for the database, and the
comparison recorded above is hardened-pre-rebase against
hardened-post-rebase, not against the original pre-hardening run.

### Executable

- Binary: `target/release/chaosbox` (23693168 bytes, built 2026-09-23 17:15)
- Command: unchanged — `chaosbox sessions verify --root .../canonical-staging-FP6mrj`
- Exit code: `0`, `elapsed_ms 100024`
- The campaign was opened read-only; nothing in it was written.
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
