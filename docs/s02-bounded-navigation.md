# S02 — bounded navigation qualification

**The existing bounded-reader fix passes the recorded Canix case through CLI and
a fresh native MCP process.** It is already present in the published memory
candidate. S02 adds an executable-selectable regression and qualification
evidence. The installed consumer still fails; promotion belongs to S25.

See the [receipt](s02-bounded-navigation.json), [S01b](s01b-resource-attribution.md)
and [workcard](../tasks/session-efficiency/todo.md).

## Existing implementation and runtime mismatch

The real flow is shared by `query_cmd.rs` and the ordinary `mcp_server.rs`
`context` tool: both call `GraphReader::context`. Commit
`4d109369527f058201cb3e6a0f52d043366df34a` already replaced the whole-export path
with bounded lexical seeds and neighborhood expansion. Commit
`e8326c125fa45a9554e259afb775fc07b4143a06` added graphs above both export caps.

`reader_navigation.rs` searches each lexical term with one sentinel result,
expands build-pinned neighborhoods under a 2,000-row traversal budget and trims
the packet to the node/character limits. Saturation is visible through
`truncated`; `exhaustive` remains false. `TypeDbReader::adjacent_relationships`
applies bounded incoming/outgoing reads before a deterministic merge. No complete
export is required for this path. The independent export caps remain enforced.

The canonical source and published candidate have byte-identical navigation
source. Even the declared installed pin `9d6c81d202aa5e9b4e0a513161891e8a4ff01e96`
contains that fix. Nevertheless, the actual installed executable still reports
the 10,000-node export-cap error. **The declared pin does not identify its actual
build source.** Its deriver was resolved, but further derivation inspection was
denied by tool authorization. The receipt records that unresolved provenance;
S25 must reconcile the exact executable rather than infer it from a pin.

## Recorded request

```text
context(repo="canix", query="RuntimeManifest", depth=2,
        max_nodes=10, max_chars=6000)
```

| Executable | Result | Fresh CLI wall time | Peak client RSS |
| --- | --- | ---: | ---: |
| Installed facade binary | Export-cap failure, reproduced by the new regression | S01b: 17.69 s | S01b: 94,628 KiB |
| Published `7313e6e5…` production candidate | Success | 0.93 s | 14,988 KiB |
| Canonical dirty native debug build | Success | 1.46 s | 28,980 KiB |

These are single, sequential samples on a shared server with uncontrolled cache
state. They establish successful execution and observed client costs, not p95,
exclusive server I/O or an accepted resource budget. Numeric limits remain
deferred as the operator requested. Transient measurement scopes and the test's
60-second subprocess timeout are experiment/hang bounds, not production policy.

Both successful CLI exports are byte-identical: **10 nodes, 5 links, 4,994 packet
characters**, `truncated=true`, `exhaustive=false`. They pin
`build:8ac2e28136c3`, generation 3, snapshot `snap:cba2b1e91c07`.
`RuntimeManifest` is returned as `ent:574d2717a6af`, with its source span at
`cli/crates/canix-foundation/src/runtime.rs:17`.

The new test checks every node's snapshot, file and span against its pinned
TypeDB row and rejects dangling returned links. It then starts the selected
binary's ordinary `mcp` entrypoint, initializes MCP and calls `context` with the
same arguments. Its packet must equal the CLI result. There is no workspace
artifact or preloaded memory graph in this test.

This verifies the candidate MCP path. The managed OpenCode MCP process continues
to use the installed executable; it has not been switched by this task.

## Verification

| Gate | Outcome |
| --- | --- |
| New regression against installed executable | Expected failure: export truncated at 10,000 nodes |
| Published candidate: CLI/MCP case and existing live neighborhood test | 2 passed, 0 ignored |
| Canonical binary: CLI/MCP case | 1 passed, 0 ignored |
| Reader and pure navigation regressions | 10 passed, 0 ignored; includes 11,000 nodes / 21,997 edges |
| Memory-backend navigation contract | 1 passed, 0 ignored |
| Required disposable TypeDB reference conformance | 1 passed, 0 ignored |
| Scoped warnings-denied Clippy, treefmt and diff check | Passed |

The opt-in live tests were explicitly selected with `--ignored`; none was
reported as a passing skip. The new case deliberately requires the recorded
Canix graph and an explicit expected build. Routine offline tests retain the
portable large-graph coverage.

Disposable conformance used TypeDB 3.13.0 at `127.0.0.1:21729`, separate scratch
storage and `CHAOSBOX_REQUIRE_TYPEDB=1`. Its service scope had a 4 GiB hard limit,
no swap and a sampled 154,562,560-byte peak. It was stopped after the test; its
data and logs are retained. The production service remained at PID 3067 with
the same start timestamp. The production graph status is identical to S01.

### Complete output preservation

A fresh canonical debug run over S01's complete frozen Canix corpus produced
**35,952,413 bytes**, byte-identical to both earlier S01 outputs:

```text
SHA-256 78b2a22ffe28ad445578063604479dfcde8016ef278065c35d76504f41fd60b5
65,872 nodes / 45,946 edges
build:760ac5e00503 / generation 1 / snap:5f76b151537a
```

The debug run took 21.50 s with 356,340 KiB peak RSS. Its unoptimized profile
makes it unsuitable for a release-speed regression claim. No facts or source
paths were removed to obtain bounded navigation.

## Reproduction and evidence

Run from the canonical Chaosbox checkout in its approved direnv environment.
These examples explicitly use Bash. Credentials are delivered by file path.

```bash
direnv exec . env TOKIO_WORKER_THREADS=2 \
  CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 \
  CHAOSBOX_TYPEDB_USER=chaosbox-app CHAOSBOX_TYPEDB_DATABASE=chaosbox \
  CHAOSBOX_TYPEDB_PASSWORD_FILE=/var/lib/typedb-auth/app-password \
  CHAOSBOX_NAVIGATION_BUILD=build:8ac2e28136c3 \
  CHAOSBOX_TEST_BIN=/nix/store/kg922178hx87ydnxn2nilmyiplaa5qyj-chaosbox-0.1.0/bin/chaosbox \
  cargo test --locked -p chaosbox --test navigation_live --jobs 2 \
  -- --ignored --test-threads=1 --nocapture

direnv exec . cargo test --locked -p chaosbox \
  --test reader_navigation --test graph_navigation --jobs 2 -- --test-threads=2
direnv exec . cargo clippy --locked -p chaosbox --test navigation_live --jobs 2 -- -D warnings
```

Omit `CHAOSBOX_TEST_BIN` to exercise Cargo's current binary. Selecting the
installed store binary reproduces the expected failure. The live graph may
move: inspect its current status before selecting a new exact build, and do not
silently relabel old evidence as a fresh result.

`/data/scratch/tmp/opencode/chaosbox-s02-20261016/` retains source manifests,
patches, binary hashes, raw metrics, test logs, full offline output and stopped
disposable data and the final concurrent-staging observation. All **153 artifact
checksums** passed. Manifest SHA-256:

```text
1f2ea2feb8aba017d8b92a8fa7cbe36f6058dcfae76d84cd38536d42da7ec611
```

Verify with `sha256sum --check SHA256SUMS` inside that directory. The source
manifest was frozen after the test and before these report/workcard edits; its
only changed code file is `crates/chaosbox/tests/navigation_live.rs`.

## Handoff

- S02's bounded-reader/source qualification is complete. Next workcard: **S03a**,
  targeted build/header/membership reads.
- Keep S01b's allocation/history attribution open. No numeric RAM, latency,
  swap or context budget was adopted here.
- S25 must promote and identify the actual consumer executable. The existing
  source fix is sufficient; repeating it would not repair the installed artifact.
- Canonical Chaosbox remains at `42f0e64f…` with its existing foreign edits.
  Excluding the new test, its tracked diff still hashes to `5b7da061…`.
  Canix's index changed concurrently from `32ae1fa3…` to the last observed
  `42004496…` as other work staged five Pink Raven storage-test files. S02 made
  no index writes; both observations are retained. The full identities are in
  the receipt. S02 changes are uncommitted. No pins, production migrations or
  activation were made by S02.
