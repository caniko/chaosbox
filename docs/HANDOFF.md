# Handoff (v0.1.0 vertical slice, 2026-09-18, atlas)

## Commands + actual results (all executed this session)

- `cargo test --workspace` — 28 passed, 0 failed
  (core 7, extract 5, jev 6, gel 5, chaosbox lib 4, vertical-slice 1)
- `cargo run -p chaosbox -- run fixtures/demo-repo --repo demo` — exit 0,
  snapshot `snap:6a8e13ca1ec3`, 22 entities / 26 candidates, publishes build,
  prints deterministic Graphify-compatible JSON
- `cargo run -p chaosbox -- extract fixtures/demo-repo` — 22 entities, 26 candidates
- `./target/debug/chaosbox db check --json --repo demo` — exit 1, stdout
  `{"contract_version":1,"backend":"gel","operation":"db check",
  "status":"pending",...}`, diagnostics on stderr (correct: no active build)
- MCP stdio: `tools/list` -> 9 read-only tools; `tools/call export` -> ok;
  `tools/call migrate` -> rejected `-32601 read-only MCP`
- `harbor-db validate --manifest nix/chaosbox-db-plan.json` (harbor-db @
  `5c605fd`) — `database-operation plan is valid`
- `nix-instantiate --parse flake.nix` — ok; `nix flake lock` — ok;
  `nix flake show` — all required outputs evaluate
  (packages default/chaosbox, apps default/chaosbox/db-check/db-migrate/test-gel,
  devShells default, formatter, checks fmt/lint/unit/doc/packaging/gel-integration)
- Note: `derive_more` pinned to `=2.0.1` via `cargo update -p` (2.1.1 breaks
  `gel-stream 0.4.5`'s bare `TryFrom` derive); recorded in Cargo.lock

## Dependency handoff revisions

- harbor-rs trunk `7a3328e` (flake input pinned to this rev)
- harbor-db local `5c605fd` (trunk `a1ae83b` — Gel backend already present there;
  re-validate plan after updating)
- simit local `b16a5af` (branch `codex/release-whitespace`, trunk `39aed87`);
  local CLI 0.17.13
- gel-tokio 0.11.0 (crates.io); Gel pinned 7.2; Jev `jev-1.13.0`

## Remaining blockers (not copied, not faked)

1. harbor-db Gel runtime/test interfaces: plan validates, but the disposable
   Gel instance + credentials/readiness flow awaits the parallel branch.
   `scripts/test-gel.sh` exits 3 PENDING without a `gel` server (no fake pass).
2. simit named integration gates + dependency-ordered publication: `simit.toml`
   present; generation/drift-check + `cargo package` verification await the
   parallel simit session. Nothing published, no tags pushed.
3. Live Jev run: adapter complete; needs operator `CHAOSBOX_JEV_API_KEY_FILE`.
   Never auto-discovers ambient provider keys (tested).

## Exact next commands

```sh
cd /data/nvme0/can/canix/projects/repos/owned/chaosbox
git log --oneline -3            # verify this handoff commit
simit init ci                   # generate workflows with pinned simit (parallel session gates)
nix flake check --no-build      # eval gate (gel-integration needs a gel server to go green)
nix run .#test-gel              # disposable Gel gate (PENDING until harbor-db branch lands)
CHAOSBOX_JEV_API_KEY_FILE=/path/to/key cargo run -p chaosbox -- run fixtures/demo-repo
cargo package -p chaosbox-core  # staged-registry verification before any publish
```
