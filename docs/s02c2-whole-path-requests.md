# S02c.2 — Ordinary path requests from acquisition

Status: locally qualified, 2026-10-18. Shared path validation and
acquisition-inclusive budgets pass through ordinary CLI and MCP clients.
**Parent S02c remains open for S02c.3's transport lifecycle and retention gate.**

Implementation savepoint: `1836030050f0c5c260d5848303a4af829d43e0af`.
The commit containing this report adds the ordinary-client regression and
[qualification receipt](s02c2-whole-path-requests.json).

## Request contract

`PathQuery::prepare` validates before either transport polls credential/backend
acquisition. Both use `PreparedPath::read` through `AnyReader::path_query`.

- Repository and endpoint labels must be nonblank, control-free and 1..256 UTF-8
  bytes. Validation preserves identity bytes; it never trims or normalizes them.
- `max_hops` defaults to 4 and accepts integers in **0..=8** through both clients.
  Zero permits only a verified existing self endpoint. Trusted already-pinned
  library calls retain their existing independent hop argument.
- MCP rejects unknown fields and malformed supplied hop values, including null,
  strings, negative/fractional numbers, booleans, lists and out-of-range integers.
  Only an omitted hop argument selects the default.
- Successful responses retain `{from, to, path}`. Found paths are shortest
  undirected paths within the hop bound. Missing endpoints and complete negative
  searches return null. Exhaustion and backend failures remain errors: nonzero CLI
  exit or MCP `-32603`; invalid MCP arguments return `-32602` before credentials.

## One monotonic budget

The prepared request starts one `Work` instance before acquisition. Acquisition
and `active_build` each consume one logical operation; endpoints and every
directional page consume the same remaining counter. Build-header, endpoint and
page JSON consume one aggregate projected-byte allowance. Neither acquisition nor
pinning resets the deadline, counters or BFS. Existing pinned-reader APIs reuse
the same work/deadline implementation from their already-pinned starting point.

Defaults remain 100,000 visits, 20,000 accepted relationship rows, 500 logical
operations, 8 MiB projected JSON and 10 seconds. Pages contain at most 200 rows;
an overflow probe may fetch one extra row beyond the accepted-row budget.
Operations are not driver RPC counts: acquisition and a TypeDB build header each
perform internal work. JSON is accounted **after decoding**, including the
header's snapshot projection; it does not bound wire bytes, database scans or
pre-allocation memory. The async deadline is cooperative. Stdio input/output and
transport cancellation are the next slice. These are request-work limits, not an
accepted production RAM budget.

## Verification

| Gate | Result |
| --- | --- |
| Pre-change RED | Two MCP regressions and CLI validation fail on the old behavior |
| Final affected suites | 109 passed; five disposable cases ignored in this command |
| Required disposable TypeDB path case | 1 passed, zero ignored; 12.50 seconds |
| Scoped warnings-denied Clippy | Passed |
| Scoped treefmt CI / Git whitespace | Passed |

Four new instrumented cases extend S02c.1's ten path tests. They prove exact
operation/row completion (eight operations succeed where seven cannot prove
completion), no acquisition poll at zero operations, exact header-plus-endpoint
byte accounting, cancellation of pending acquisition/pinning futures, and one
deadline across individually shorter acquisition, pinning and page delays.
These future-drop checks establish local behavior only.

A silent localhost TCP listener exercises the actual TypeDB connector through
fresh CLI and persistent ordinary MCP. Deadline errors arrive after 10,005 ms
and 10,001 ms respectively; the same MCP process answers a subsequent ping and
exits normally. This tests acquisition deadlines, not matching-ID cancellation
or remote transaction cleanup.

The required live case uses TypeDB CE 3.13.0 at `127.0.0.1:24729`, fresh database
`s02c2_clients_20261018`. The main fixture has 652 nodes / 654 relationships,
uneven hub directions (227 outgoing, 427 incoming), self-loops and a cycle. An
additional published repository supplies a real foreign endpoint. Fresh CLI,
fresh MCP and one persistent MCP process agree across **14 cases per transport**:
late outgoing/incoming targets, reverse and two-hop paths, hop-bound negatives,
isolated/missing/foreign endpoints, zero/eight-hop self queries, omitted defaults,
explicit exhaustion and repinning after publication. All three exhausted searches
return the query-budget error, never null.

The old reader remains pinned to `build:f1b86c0a1066` after publication of
`build:2492310e4d7b`. A fresh exact historical reader still finds the old path;
new requests through all three clients use the new publication and return null.
The four earlier disposable explanation/neighborhood cases were not rerun.

## Reproduce and provenance

Run native Cargo in the approved project environment, with `TOKIO_WORKER_THREADS=2`,
`--jobs 2` and the retained systemd scope settings: `MemoryHigh=2G`,
`MemoryMax=4G`, `MemorySwapMax=0`, `CPUQuota=200%`.

```nu
cargo test --locked --offline -p chaosbox --lib --bin chaosbox --test path_budgets --test path_clients --test neighborhood_clients --test neighborhood_continuation --test neighborhood_pages --test explain_counts --test reader_navigation --test read_views --jobs 2 -- --nocapture
cargo clippy --locked --offline -p chaosbox --lib --bin chaosbox --test path_budgets --test path_clients --jobs 2 -- -D warnings
```

The live command, explicit disposable configuration, scope limits and original
worker logs are retained at `/data/scratch/tmp/opencode/chaosbox-s02c2-20261018/`.
Require `CHAOSBOX_REQUIRE_TYPEDB=1`, an isolated `s02c2_*` database and disposable
credentials when explicitly running the ignored live test. The normal suite
does not turn ignored live cases into acceptance evidence.

Qualification uses the dirty canonical checkout based on
`accc0e5b1289b10c6c51733acc8b11737430c7a1`. The source archive includes foreign
work and the local execution plan; this is not clean-release qualification of
the scoped commits alone. Archive SHA-256:
`abb25d35bd17307cab7f0ab429c4d8dab816629ed9cc5b812e67344c2640d99b`.
Exact CLI/test executables, test output and checksums are retained alongside it.

The live worker records 13.485 seconds / 619.4M native memory peak including warm
compilation. The disposable server records 181,825,536 peak bytes and zero swap;
it was stopped with SIGTERM and its data/logs retained. Production TypeDB's
PID/start identity and Canix staging compare unchanged. Foreign changes remain,
including the separate `write_export_json` export in `lib.rs`; its surrounding
diff context moves with the new re-exports. No consumer promotion is established.

## Next-session handoff

Continue with **S02c.3**. Refresh Git/index state and read this report plus the
ordinary `serve_mcp` loop. Implement bounded frames and output backpressure,
matching-ID cancellation, EOF/output-failure cleanup and measured repeated-process
retention. Verify unrelated cancellation IDs preserve work, and retain actual
local/remote read-cleanup evidence. Keep parent S02c open until that gate passes.
