# Handoff (v0.3.0, 2026-09-18, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 45 passed, 0 failed; zero warnings
  (core 8, extract 5, jev 12, gel 8 incl. conformance + lease + supersedure,
  chaosbox lib 7 incl. fake-backed reader, vertical-slice 1, bin 4)
- `nix-instantiate --parse flake.nix` — ok (no flake changes this round)
- Previous MCP/CLI verifications (v0.2.0) unaffected: no protocol changes

## Changes since v0.2.0

- Phase 0 seam: `GelQueries` trait (live `GelHandle` + `MemoryReader` fake +
  `check_conformance` suite, also runnable against future live Gel),
  `Pipeline<S: Store>` generic; canonical `relation_type_name` /
  `evidence_class_name` helpers shared by fake and future inserts
- Phase 2.1: every read scoped to the pinned build via membership-filtered
  EdgeQL (`SEARCH/LOOKUP/NEIGHBORS/EVIDENCE` through `GraphMembership` /
  `GraphEdgeMembership`); LIKE wildcards escaped reader-side; leakage tests
  assert cross-build invisibility on both fake and reader layers
- Phase 2.2: `Materialization.abstain_confidence` (in identity, `validate()`
  enforces floor ordering, abstain-first precedence); per-candidate `Failed`
  decisions with catch-and-continue (fault text never enters evidence);
  `Failed`-once supersedure in `put_decision`, other outcomes immutable
- Phase 2.3: `WorkerTask` SDL + `m2_worker_tasks` migration (schema v2);
  lease claim/heartbeat/reclaim with injected clocks and generation guards

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned)
- harbor-db local `5c605fd` (trunk `a1ae83b` has the `gel` backend)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`)
- gel-tokio 0.11.0; Gel server pinned 7.2; nixpkgs Gel CLI 7.10.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. Disposable Gel instance + credentials/readiness flow (harbor-db branch).
   Unblocks: write-path integration, decision cache, live conformance run.
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
