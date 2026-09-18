# Handoff (v0.2.0, 2026-09-18, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 39 passed, 0 failed; zero warnings
  (core 7, extract 5, jev 12 incl. 5 HTTP wire tests + round-trip, gel 5,
  chaosbox lib 5, vertical-slice 1, chaosbox bin 4 MCP routing tests)
- `cargo run -p chaosbox -- run fixtures/demo-repo --repo demo` — exit 0,
  publishes build, prints deterministic Graphify-compatible JSON
- `./target/debug/chaosbox query search hello --repo demo` (no Gel) — exit 1,
  contract-v1 `error` JSON on stdout, diagnostics on stderr
- MCP stdio: `initialize` negotiates (2024-11-05 and 2025-06-18 verified);
  `tools/list` paginates 5+4 with `inputSchema.required` + `readOnlyHint`;
  `tools/call migrate` -> `-32601`; missing args -> `-32602`; valid call
  without Gel -> `-32603` with contract-v1 data
- `harbor-db validate --manifest nix/chaosbox-db-plan.json` — valid
- `nix-instantiate --parse flake.nix` — ok; `nix flake show` — all outputs
  evaluate (pinned `gel` 7.10.2 in `db-migrate`/`test-gel` closures)
- `GEL_CREDENTIALS_FILE` verified as a documented Gel connection parameter
  (docs.geldata.com/reference/using/connection)

## Fixes since v0.1.0

- `search` sorts before truncating (was take-then-sort)
- `Materialization::identity` covers `accept_score` (was missing)
- Decision Noul/Score cutoffs routed through `Materialization` (single source)
- Serde `Answer` wire bug: removed `kind` fields that clashed with the
  internally-tagged `"type"` discriminant (failed to deserialize, double-emit
  on serialize); locked with a round-trip test
- Parallel-edge dedup uses numeric suffixes (N-way safe)
- HTTP-test env race fixed with a shared lock

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned)
- harbor-db local `5c605fd` (trunk `a1ae83b` has the `gel` backend)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`)
- gel-tokio 0.11.0; Gel server pinned 7.2; nixpkgs Gel CLI 7.10.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. Disposable Gel instance + credentials/readiness flow (harbor-db branch).
   `test-gel` exits 3 PENDING without a `gel` server.
2. simit named gates + ordered publication; nothing published, no tags pushed.
3. Live Jev quality: `run --live-jev` ready, needs operator key file.

## Exact next commands

```sh
cd /data/nvme0/can/canix/projects/repos/owned/chaosbox
git log --oneline -3
simit init ci
nix flake check --no-build
nix run .#test-gel
CHAOSBOX_JEV_API_KEY_FILE=/path/to/key cargo run -p chaosbox -- run fixtures/demo-repo --live-jev
cargo package -p chaosbox-core
```
