# S02 — bounded navigation qualification

**The existing bounded-reader fix passes the recorded Canix case through CLI and
a fresh native MCP process.** It is already in the published memory candidate;
S02 adds an executable-selectable regression and qualification evidence. The
installed consumer still fails, and promotion belongs to S25.

See the [receipt](s02-bounded-navigation.json), [S01b](s01b-resource-attribution.md)
and [workcard](../tasks/session-efficiency/todo.md).

## Existing implementation and runtime mismatch

`query_cmd.rs` and the ordinary `mcp_server.rs` `context` tool both call
`GraphReader::context`. Commit `4d109369527f058201cb3e6a0f52d043366df34a`
replaced the whole-export path with bounded lexical seeds and neighborhood
expansion; commit `e8326c125fa45a9554e259afb775fc07b4143a06` added graphs above
both export caps.

`reader_navigation.rs` searches each lexical term with one sentinel result,
expands build-pinned neighborhoods under a 2,000-row traversal budget and trims
the packet to node/character limits (`truncated` reports saturation; `exhaustive`
stays false). `TypeDbReader::adjacent_relationships` applies bounded
incoming/outgoing reads before a deterministic merge. No complete export is
needed, while independent export caps stay enforced.

Canonical source and the published candidate have byte-identical navigation
code, and even the declared installed pin `9d6c81d202aa5e9b4e0a513161891e8a4ff01e96`
contains the fix, yet the installed executable still hits the 10,000-node
export-cap error. **The declared pin does not identify its build source.** Its
deriver was resolved, but further derivation inspection was denied by tool
authorization; the receipt records that unresolved provenance, so S25 must
reconcile the actual executable rather than infer it from a pin.

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

These are single sequential samples on a shared server with uncontrolled cache
state. They prove execution and observed client costs, not p95, exclusive server
I/O or an accepted resource budget; numeric limits remain deferred by operator
request. Transient measurement scopes and the test's 60-second subprocess
timeout are experiment/hang bounds, not production policy.

Both successful CLI exports are byte-identical: **10 nodes, 5 links, 4,994 packet
characters**, `truncated=true`, `exhaustive=false`, pinned to
`build:8ac2e28136c3`, generation 3, snapshot `snap:cba2b1e91c07`.
`RuntimeManifest` returns as `ent:574d2717a6af` at
`cli/crates/canix-foundation/src/runtime.rs:17`.

The new test checks every node's snapshot, file and span against its pinned
TypeDB row, rejects dangling returned links, then starts the selected binary's
ordinary `mcp` entrypoint, initializes MCP and calls `context` with the same
arguments, requiring exact CLI/MCP packet equality without a workspace artifact
or preloaded memory graph.

This verifies the candidate MCP path; the managed OpenCode MCP process still
runs the installed executable and was not switched here.

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

Opt-in live tests ran explicitly via `--ignored` (no passing skips). The new
case requires the recorded Canix graph and an explicit expected build; routine
offline tests keep portable large-graph coverage.

Disposable conformance used TypeDB 3.13.0 at `127.0.0.1:21729`, separate scratch
storage and `CHAOSBOX_REQUIRE_TYPEDB=1`, under a 4 GiB hard limit with no swap
(sampled 154,562,560-byte peak), then stopped with data and logs retained.
Production stayed on PID 3067 with unchanged start timestamp and S01 graph
status.

### Complete output preservation

A fresh canonical debug run over S01's complete frozen Canix corpus produced
**35,952,413 bytes**, byte-identical to both earlier S01 outputs:

```text
SHA-256 78b2a22ffe28ad445578063604479dfcde8016ef278065c35d76504f41fd60b5
65,872 nodes / 45,946 edges
build:760ac5e00503 / generation 1 / snap:5f76b151537a
```

The debug run took 21.50 s and 356,340 KiB peak RSS (unoptimized profile, not a
release-speed claim). Bounded navigation removed no facts or source paths.

## Reproduction and evidence

Run in Bash from the canonical Chaosbox checkout in its approved direnv
environment, delivering credentials by file path:

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

Omit `CHAOSBOX_TEST_BIN` to test Cargo's current binary; pointing it at the
installed store binary reproduces the expected failure. Because the live graph
may move, inspect current status before choosing a new expected build and never
relabel old evidence as a fresh run.

`/data/scratch/tmp/opencode/chaosbox-s02-20261016/` retains source manifests,
patches, binary hashes, raw metrics, test logs, full offline output, stopped
disposable data and the final concurrent-staging observation. All **153 artifact
checksums** passed; manifest SHA-256:

```text
1f2ea2feb8aba017d8b92a8fa7cbe36f6058dcfae76d84cd38536d42da7ec611
```

Verify with `sha256sum --check SHA256SUMS` there. The source manifest was frozen
after the test and before these report/workcard edits; the only changed code
file is `crates/chaosbox/tests/navigation_live.rs`.

## Handoff

- S02 bounded-reader/source qualification is complete. Next workcard: **S03a**,
  targeted build/header/membership reads.
- S01b allocation/history attribution stays open; no numeric RAM, latency, swap
  or context budget was adopted.
- S25 must promote and identify the actual consumer executable; repeating the
  existing source fix will not fix the installed artifact.
- Canonical Chaosbox stays at `42f0e64f…` with its existing foreign edits;
  excluding the new test, its tracked diff still hashes to `5b7da061…`.
  Canix's index changed concurrently from `32ae1fa3…` to the last observed
  `42004496…` as other work staged five Pink Raven storage-test files. S02 made
  no index writes; both observations are retained, with full identities in the
  receipt. S02 changes are uncommitted, with no pins, production migrations or
  activation.
