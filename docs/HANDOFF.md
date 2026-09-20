# Handoff (TypeDB migration, 2026-09-20, atlas)

## Landed this round

- `chaosbox-typedb`: schema/encoder/store/reader, all live-proven against
  TypeDB 3.13.0 (driver 3.12.3): migrate idempotent, publish+readback,
  predecessor/generation guards, concurrent-publisher exactly-once,
  reference `check_conformance` green. Rust 1.93.0 toolchain (driver needs
  >=1.88). Workspace tests + clippy pedantic + fmt clean.
- CLI/MCP/lifecycle switch (`CHAOSBOX_DB_BACKEND=typedb`, contract v2):
  migrate/check/run/query/export verified live end to end (fixture run
  published 22 nodes / 27 edges; search/neighbors/status/export answer).
- Nix: harbor-db re-pinned to `typedb-backend`; deployment module rewritten
  for `services.typedb` + harbor-db `typedb` operations (credential-file
  delivery, systemd ordering); `typedb-integration` VM test replaces the Gel
  one; `test-typedb.sh` disposable gate; simit gate `ci-chaosbox-typedb.yaml`.
  `deployment-eval` (7 checks) and flake eval green; temp `nixpkgs-typedb`
  pin (binaries substitute from `attic.candee.baby/canix`; removal:
  nixpkgs#565068 merge).
- Upstream: typedb/typedb#7978 (flake, eval-verified); companion
  typedb-tools lock-refresh identified (tag pins driver 3.12.0 vs required
  3.12.3; nixpkgs carries the one-liner until a fixed tag).
- nixpkgs: NixOS/nixpkgs#565068 (draft; by-name layout fixed, duplicate
  maintainer dropped). review-gha dispatched; iterating on evidence.
- harbor-db#7 (`Backend::Typedb` label; generic runner/credentials/ordering
  reused, readiness stays application-owned).

## Still blocked / pending

- Remote TypeDB builds (review-gha runs iterating; RocksDB unit gates TBD).
- Flake-head remote build: OAuth token lacks `workflow` scope, so no ad-hoc
  Actions validation from here (`gh auth refresh -s workflow`, then push the
  saved `nix-verify` workflow and dispatch on `caniko/typedb@nix-flake-verify`).
- NixOS VM test realization (needs substituted binaries from the review
  run, then KVM run here; CI runs it under emulation).
- Live-Jev smoke: key absent in this session.
- Cutover: flip `CHAOSBOX_DB_BACKEND` default after validation; remove Gel
  runtime wiring (crate stays as conformance reference).

## Exact next commands

```sh
cd /data/nvme0/can/canix/projects/repos/owned/chaosbox
git log --oneline -5   # typedb-migration
gh pr list --head typedb-migration  # open when green
nix flake show         # eval only (policy)
```

## Prior rounds

 Kept below for continuity; the Gel path above is superseded.

### Container-free gel-integration (2026-09-19)

- `run_migrate` passes explicit `--credentials-file` alongside the env
  passthrough; `nix/gel-vm-test.nix` docker-in-guest 7.1 digest-pinned
  matrix (pending → migrate → ready → idempotent → wrong-creds).
- Verified without realizing (eval + dry-run only, policy-gated).
