# Handoff (v0.6.0, 2026-09-19, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 45 passed, 0 failed; zero warnings
- `cargo run -p chaosbox -- run fixtures/demo-repo --repo demo` — exit 0
- `nix-instantiate --parse flake.nix` — ok (no flake changes this round)

## Changes since v0.5.0 (Round 3 schema-first: run identity + key storage)

- Core: `Decision.cache_key` (documented reuse contract), `CATALOG_VERSION`
  (`catalog-v1`), order-invariant + change-sensitive `catalog_digest()`;
  `decide()` stamps every key (source snapshot, whole-catalog digest,
  questions, model, rubric; conservative: any catalog change re-asks all)
- Gel schema: `Decision.cache_key` required field + `m3_decision_cache_key`
  migration (schema v3, chain-linked, asset-tested)
- Run identity: `Store::ensure_run` + `Store::put_candidate` on both stores
  (set-identity mismatch and unregistered sets rejected); `run_pipeline`
  mints deterministic run/set ids and registers the catalog before deciding;
  vertical slice asserts candidate/run counts
- Write conformance extended with run/candidate registration cases

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned)
- harbor-db local `5c605fd` (trunk `a1ae83b` has the `gel` backend)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`)
- gel-tokio 0.11.0; Gel server pinned 7.2; nixpkgs Gel CLI 7.10.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. Disposable Gel instance + credentials/readiness flow (harbor-db branch).
   Unblocks: live write/read conformance, cache proof.
2. simit named gates + ordered publication; nothing published, no tags pushed.
3. Live Jev quality: `run --live-jev` ready, needs operator key file.
4. Note: `00001.edgeql` is a module stub — full SDL↔migration reconciliation
   happens on the first live `gel migration create`/apply cycle.

## Exact next commands (Round 3 behavior: lookup + flush)

```sh
cd /data/nvme0/can/canix/projects/repos/owned/chaosbox
git log --oneline -3
# 1. Store::find_decision; decide() skips on cache-key match
# 2. decision/evidence/claim flush methods on GelHandle; GelStore full chain
# 3. per-axis invalidation tests (catalog/model/rubric vs thresholds)
nix run .#test-gel            # PENDING until the harbor-db branch lands
```
