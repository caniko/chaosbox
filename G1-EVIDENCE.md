# Gate G1 — all 7644 existing receipts verify

Recorded outside the campaign directory, because the campaign
(`2026-09-22-v2/`) is treated as read-only while this gate was evaluated.
`PLAN-REVISION.md` inside the campaign was left untouched; this file is its
evidence slot.

## Executable

Built from branch `session-verifier`, worktree
`/data/scratch/tmp/opencode/chaosbox-session-verifier`, commits `1123b05`
and `513a0ef` on top of trunk `33d7477` (after the `feat!` that removed the
Gel backend). The branch was originally cut from `03339f7` and rebased onto
`33d7477`; only `Cargo.lock` conflicted, and this gate was re-run in full
after the rebase.

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
