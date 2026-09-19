# Handoff (v0.4.0, 2026-09-18, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 46 passed, 0 failed; zero warnings
- `cargo run -p chaosbox -- run fixtures/demo-repo --repo demo` — exit 0
- `query neighbors xxx --rel Frobnicate` — exit 1, vocabulary error before
  any Gel connection attempt
- `nix-instantiate --parse flake.nix` — ok; `nix flake show` — evaluates

## Changes since v0.3.0 (Round 1: persistence inside decide)

- `decide()` takes `store: &mut S` and persists each decision + evidence as
  produced (all outcomes incl. `Failed`/`Abstained`); 7 call sites updated
- `build_and_publish()` assembles one `Claim` per materialized relation
  (supporting evidence + same-triple same-batch rejections as contradicting),
  persisted via `put_claim`; non-materialized outcomes stay decision-level only
- `MemoryStore::stats()` for pipeline observability; vertical-slice test
  asserts per-run store counts, claim-per-edge, and cross-rerun supersedure
- Removed dead `db_check_report` (zero callers; `db_check_gel` is the path)
- `validate_rel_filter` shared by CLI (pre-connect), MCP (`-32602`), and
  `GelReader::neighbors` (defense in depth); case-insensitive, canonical output

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned)
- harbor-db local `5c605fd` (trunk `a1ae83b` has the `gel` backend)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`)
- gel-tokio 0.11.0; Gel server pinned 7.2; nixpkgs Gel CLI 7.10.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. Disposable Gel instance + credentials/readiness flow (harbor-db branch).
   Unblocks: Gel-backed `Store`, decision cache, live conformance run.
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
