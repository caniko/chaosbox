# Handoff (v0.7.0, 2026-09-19, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 46 passed, 0 failed; zero warnings
- `cargo run -p chaosbox -- run fixtures/demo-repo --repo demo` — exit 0
- `nix-instantiate --parse flake.nix` — ok (no flake changes this round)
- gel-protocol audit: positional tuples cap at 12 params (span/evedence
  statement splits), `Option<T>` args supported, `<uuid><str>` casts avoid
  a new uuid dependency

## Changes since v0.6.0 (Round 3 behavior: lookup + flush)

- `Store::find_decision` on both stores (`GelHandle` select const +
  JSON-outcome round-trip conversion); shared evidence assembler so cache
  reuse rebuilds byte-identical rows; skip-on-key-match with failures
  always re-asked
- Supersedure generalized: replace on cache-key change (model/rubric/
  catalog invalidation lands in the store, not just the pipeline);
  conditional upsert const mirrors the rule for live Gel
- Per-axis invalidation tests (catalog/model/rubric invalidate, thresholds
  reuse via a failing responder that never gets called)
- Full-chain flush in `GelStore.publish` (runs, sets, candidates,
  decisions, evidence, claims, relationship evidence links) in FK order;
  `LINK_EVIDENCE` const closes the relationship-evidence gap
- Write conformance covers the new surface; live variant structured

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned)
- harbor-db local `5c605fd` (trunk `a1ae83b` has the `gel` backend)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`)
- gel-tokio 0.11.0; Gel server pinned 7.2; nixpkgs Gel CLI 7.10.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. Disposable Gel instance + credentials/readiness flow (harbor-db branch).
   Unblocks: live write/read conformance, cache proof, `test-gel` green.
2. simit named gates + ordered publication; nothing published, no tags pushed.
3. Live Jev quality: `run --live-jev` ready, needs operator key file.
4. Note: `00001.edgeql` is a module stub — full SDL↔migration reconciliation
   happens on the first live `gel migration create`/apply cycle.

## Exact next commands

```sh
cd /data/nvme0/can/canix/projects/repos/owned/chaosbox
git log --oneline -3
nix run .#test-gel            # PENDING until the harbor-db branch lands
simit init ci
CHAOSBOX_JEV_API_KEY_FILE=/path/to/key cargo run -p chaosbox -- run fixtures/demo-repo --live-jev
cargo package -p chaosbox-core
```
