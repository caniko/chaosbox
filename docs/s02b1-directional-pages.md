# S02b.1 — Directional backend pages

Status: implemented and locally qualified, 2026-10-16.

The reference and TypeDB readers expose `GraphQueries::neighbor_page`.
Final-source reference and required disposable TypeDB paging, affected library
tests, existing required backend conformance, treefmt and warnings-denied Clippy
pass, including exact-size boundaries and genuinely multi-page sparse filters.
S02b.1 is complete; parent S02b retains its S02b.2/S02b.3 gates.

## Contract and implementation

- The page anchors one endpoint in one explicit build and direction.
- Results sort by relationship ID ascending, strictly after the optional raw
  key. Build/anchor/type/boundary predicates precede the backend row limit.
- Empty filters and nonpositive limits return no rows. Duplicate filter values
  do not duplicate relationships; self-loops appear once per direction.
- Callers may request `page_size + 1` to detect a sentinel, then resume after
  the last delivered row rather than the sentinel.
- TypeDB executes one limited projection query per nonempty positive-limit
  call. This query count is source-derived; server scan/sort work is unmeasured.
- The reference reader scans a BTreeMap range and projects at most the requested
  rows. Its remaining-edge scan ceiling is documented in the implementation.
- Unsupported implementations fail explicitly; there is no unbounded fallback.

This raw backend method is not an authorization or continuation-token API.
S02b.2 must bind version/scope and resolve exact historical publications;
S02b.3 must expose complete enumeration through ordinary CLI/MCP requests.

Owned production files: `chaosbox-store/src/{queries,lib,memory_reader}.rs`
and `chaosbox-typedb/src/reader.rs`, under `crates/`. The focused regression
is `crates/chaosbox/tests/neighborhood_pages.rs`. Existing foreign changes
in `chaosbox-store/src/lib.rs` were retained.

## Executed checks

Initial commands used the approved project direnv; final commands used its
unchanged, already-realized GC-rooted profile with explicit operator approval
(see environment recovery below).
Worker scopes bounded memory to 4 GiB, disallowed swap and limited CPU to two
cores; Cargo used two jobs. These are execution safeguards, not accepted
production resource targets.

| Gate | Observed result |
| --- | --- |
| Initial reference regression with unsupported default | Failed at `neighborhood pages unavailable`, before implementation |
| Reference regression after implementation | 1 passed; disposable case intentionally excluded by default |
| Explicit disposable TypeDB regression | 1 passed, zero ignored, with `CHAOSBOX_REQUIRE_TYPEDB=1` |
| Scoped warnings-denied Clippy | Passed after splitting test helpers and replacing integer casts |
| Scoped treefmt before the last boundary additions | Passed |
| Final-source scoped treefmt | Passed |
| Final reference regression | 1 passed, 0 ignored; disposable case filtered out |
| Final required disposable TypeDB paging | 1 passed, 0 ignored; 21.92 seconds |
| Affected library and count-only reference suites | 59 passed; one unrelated isolated-RSS test ignored; S02a disposable count case filtered out |
| Existing required backend conformance | 1 passed, 0 ignored; 0.43 seconds |
| Final scoped warnings-denied Clippy | Passed |

The first live fixture had 1,026 current-build nodes and 2,052 relationships,
with 1,027 relationships in each hub direction. Two older/foreign builds
exercise membership isolation. Assertions compare relationship IDs, types and
both endpoints against the complete fixture; they cover self-loops, parallel
types, empty/repeated/encoded filters and late-sorting rare matches after more
than 512 excluded relationships.

Complete enumeration alone made 332 page calls, reading 4,432 rows including
sentinels and 625,277 projected JSON bytes. These totals exclude the other
boundary/isolation assertions, connection, migration and publication. They are
not MCP wire or whole-request measurements.

The first disposable test reported 21.25 seconds. Its timed Cargo invocation
took 22.32 seconds and peaked at 484,088 KiB RSS, including recompilation and
children. The disposable server's recorded cgroup peak was 160,423,936 bytes,
with zero current swap. None of these measurements establishes steady-state,
large-corpus, installed-consumer or production-stack acceptance.

The final fixture adds three `references` matches in each direction, separated
by more than 128 excluded relationships, with the first following more than 512
excluded rows. A one-row delivered page plus sentinel must enumerate all three
pages without losing the sentinel. Complete equality checks cover every
relationship ID, type and endpoint in each direction.

The final build has 1,026 nodes and 2,056 relationships, with 1,029 hub rows per
direction. Complete enumeration alone made 336 calls and read 4,444 rows including
sentinels / 627,028 projected JSON bytes. These figures exclude boundary/isolation
queries, migration and publication. The live worker's native summary records
22.015 seconds wall time and 92.6 MiB peak memory; compilation was already warm.
The disposable server's recorded peak was 184,041,472 bytes, with zero current
swap. These are bounded fixture observations, not production acceptance targets.
Existing backend conformance retains S03a's boundary-summary scope, not complete
row-payload/source-span parity.

## Environment recovery

Editing the Rust files invalidated nix-direnv's watched-source cache. Harness
preparation timed out at 600 seconds; bounded direct diagnosis also failed to
refresh it. The installed Canix CLI lacks `direnv prepare --directory`, so the
existing canonical `cli/target/debug/canix` was used for the supported route:

```text
canix repo direnv prepare --directory /data/nvme0/can/canix/projects/repos/owned/chaosbox --wait-seconds 30
Error: refusing Nix evaluation: only 17 GiB RAM available (MemAvailable), need at least 24 GiB to start the 32 GiB scope
```

The operator authorized a 4 GiB/zero-swap refresh; it timed out after 300 seconds.
The operator then separately authorized reuse of the unchanged approved profile:

```text
nix develop --extra-experimental-features read-only-local-store --store 'local?read-only=true' /nix/store/0y0zaqh1s6m4lk9734hc02rv5xdjq8qw-nix-shell-env --command ...
```

The shell inputs `.envrc`, `flake.nix`, `flake.lock`, `Cargo.toml`, `Cargo.lock`
and `rust-toolchain.toml` compare unchanged against the earlier pending-source
archive. No normal admission or direnv fallback policy changed. Read-only
rooted-profile entry succeeded without checkout evaluation or Nix realization;
test workers held no evaluation lease. A first live invocation omitted the
required password-file variable and failed before connection; its log is retained.
The corrected invocation passed against a fresh disposable database.

The final disposable server received SIGTERM after verification and is inactive;
its data and logs remain. Production TypeDB's PID/start identity is unchanged.

## Evidence and savepoint

Evidence root: `/data/scratch/tmp/opencode/chaosbox-s02b1-20261016/`.
`qualification.sha256` hashes the retained initial test executable, logs,
pending-source archive and production/server observations. Its SHA-256 is
`592ef16a1c3ca276999324fcef59479144c09cdf9270b388474204c6d00c46af`.
The JSON receipt records exact identities and evidence boundaries.

Final evidence root: `/data/scratch/tmp/opencode/chaosbox-s02b1-final-20261016/`.
Its 26-entry `qualification.sha256` verifies, with manifest SHA-256
`700ad8589720a2f4a9b4fe026abdbe1cb925230e07ce9858820dd6ebed616373`.
It freezes both exact test executables, all qualification logs, successful native
worker commands/resources, server lifecycle and preservation observations.
`source-tested.tar` has SHA-256
`eb6744ec06a3b854927152e5b4beffa766f9d1e07ce47db5ca22025b14bf1eb7`
and compares byte-for-byte with the complete tested source after all checks.

Qualification ran on the dirty canonical checkout based on signed S02a commit
`92d635848ae0b57443a4cebb21cb1ef69b140ffe`; the archived source includes unrelated
changes. The scoped S02b.1 savepoint contains the four production changes, focused
regression and this report/receipt. It is not clean-release qualification of that
commit alone. Foreign tracked changes outside those four files and Canix staging
compare unchanged; foreign hunks within `chaosbox-store/src/lib.rs` remain outside
the savepoint. The earlier bundle remains historical and unchanged.

Next: S02b.2 scope-bound continuation and exact historical pinning, then S02b.3
ordinary CLI/MCP enumeration. Fresh `s02b1_*` databases are required for reruns:
the regression publishes a fixed predecessor chain. No installed-consumer
transition or whole-stack resource acceptance is established here.
