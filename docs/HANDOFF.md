# Handoff (v0.5.0, 2026-09-19, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 44 passed, 0 failed; zero warnings
  (3 store tests merged into 1 write-conformance test vs v0.4.0's 46)
- `cargo run -p chaosbox -- run fixtures/demo-repo --repo demo` — exit 0
- `db check --json` without Gel — exit 1, contract-v1 error JSON
- `nix-instantiate --parse flake.nix` — ok (no flake changes this round)
- gel-protocol audit: positional arg tuples cap at 12 params; `Option<T>`
  args supported; `uuid` passed as `<uuid><str>` cast (no new dep)

## Changes since v0.4.0 (Round 2: Gel write path, graph half)

- `Store` is async end-to-end (`MemoryStore`, `decide`, `build_and_publish`,
  all callers; gel store tests now `#[tokio::test]`)
- `GelStore`: in-memory staging with identical semantics + flush at
  publication (snapshots, files, entities+spans, relations, memberships,
  build row) with a generation-guarded pointer swing; concurrent publisher
  wins, retry idempotent, last-good stays active on failure
- Fixed `UPSERT_ENTITY` (was missing the required span link — would have
  failed on live Gel); split span insert (12-param ceiling); canonical
  `entity_kind_name` shared by fake and inserts (fake previously used Debug)
- Decision-chain EdgeQL consts reviewed (conditional Failed-supersedure
  upsert, run/set/candidate/attempt/evidence/claim inserts); row-flush
  methods deferred to Round 3 with the candidate chain (FK requires
  run/set identity born in `run_pipeline`)
- Shared write-conformance suite (`check_write_conformance`) over any
  `Store`: linkage, idempotency, supersedure, publication guards

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned)
- harbor-db local `5c605fd` (trunk `a1ae83b` has the `gel` backend)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`)
- gel-tokio 0.11.0; Gel server pinned 7.2; nixpkgs Gel CLI 7.10.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. Disposable Gel instance + credentials/readiness flow (harbor-db branch).
   Unblocks: live write/read conformance, decision cache proof.
2. simit named gates + ordered publication; nothing published, no tags pushed.
3. Live Jev quality: `run --live-jev` ready, needs operator key file.
4. Note: `00001.edgeql` is a module stub — full SDL↔migration reconciliation
   happens on the first live `gel migration create`/apply cycle.

## Exact next commands (Round 3: decision cache)

```sh
cd /data/nvme0/can/canix/projects/repos/owned/chaosbox
git log --oneline -3
# 1. run_pipeline mints run/set identity; ensure_run + put_candidate
# 2. decision/evidence/claim flush methods; cache_key() lookup in decide()
# 3. per-axis invalidation tests (catalog/model/rubric vs thresholds)
nix run .#test-gel            # PENDING until the harbor-db branch lands
```
