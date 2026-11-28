# S02a — count-only explanations

**CLI and MCP now share an exact count-only explanation path.** TypeDB returns
two aggregate scalar rows instead of relationship collections, while the
reference backend counts directly over the selected build's edges.

See the [receipt](s02a-count-only-explanations.json) and
[workcards](../tasks/session-efficiency/todo.md).

## Shared contract and implementation

`GraphReader::explain` keeps its entity lookup, JSON fields and successful
`null` for a missing member. For a present member it calls
`GraphQueries::neighbor_counts(build, id, all_relation_types())` once and
propagates errors with no collection fallback. CLI `query explain` now delegates
to that method alongside MCP.

TypeDB binds exact build membership, anchor ID and relation types before
`reduce $count = count($rel)`, once per direction. Distinct counting keeps set
semantics for repeated filter values and parallel edges; a self-loop counts once
per direction, and an empty filter returns `(0, 0)` without querying. Strings
use the shared TypeQL encoder. The common decoder accepts scalar `Concept::Value`
and attribute values, failing explicitly on unexpected shapes or negative counts.

The reference backend scans edges directly without allocating a relationship
projection (deliberate O(build edges) ceiling, marked in source). Unsupported
backends fail explicitly instead of materializing neighborhoods, and the raw
query API keeps its trusted-caller ownership requirement.

This slice updates six production files and adds one focused integration target:
`chaosbox-store/src/{queries,memory_reader}.rs`,
`chaosbox-typedb/src/{common,reader}.rs`,
`chaosbox/src/{reader,query_cmd}.rs` and `chaosbox/tests/explain_counts.rs`.
MCP dispatch needs no change.

## Verification

The initial regression failed on the old path at the forbidden `neighbors_out`
call (its test backend permits only headers, keyed entity lookup and counts,
failing collection/export/evidence reads). The repaired path passes, including
count-error propagation and missing-member short-circuiting.

Both reference and disposable TypeDB run the same count cases:

- 1,026 nodes / 1,027 edges in the current build (1,024 outgoing hub-to-leaf
  calls, a parallel references edge, a reverse edge and a self-loop);
- hub counts **1,026 outgoing / 2 incoming**, isolated-node zero degree;
- historical build sharing the hub identity with only one edge;
- missing anchor/build and an entity present only in another build;
- empty, repeated, selective and encoded hostile-string type filters.

Three CLI cases and nine calls in one MCP process compare complete JSON values
(including source spans and compiler-field nullability) against expected hub,
isolated and missing-entity outputs, using the Cargo-built executable selected
at compile time rather than the installed launcher.

| Gate | Result |
| --- | --- |
| Initial collection-forbidden regression | Expected failure, exit 101 |
| Affected library tests | `chaosbox`: 44; store: 9; TypeDB: 4 passed |
| Focused reference regressions | 2 passed |
| Explicit disposable TypeDB/CLI/MCP regression | 1 passed, 0 ignored |
| Required existing TypeDB reader conformance | 1 passed, 0 ignored |
| Warnings-denied scoped Clippy | Passed |
| Scoped treefmt and diff checks | Passed |
| Frozen S03a evidence checksums | All 200 passed |

The ordinary affected-test run ignored the opt-in disposable test and one
pre-existing isolated publication-RSS test; the disposable test then ran
explicitly, and the unrelated RSS ignore is not acceptance evidence. Logs keep
both the initial test-setup compile and Clippy invocation/test lint errors and
the passing reruns.

## Materialization and observed costs

TypeQL counts below are **source-derived**, not instrumented wire-query
counters; Connection/authentication/database-metadata RPCs are excluded.

| Operation | TypeQL queries | Materialized rows |
| --- | --- | --- |
| Nonempty `neighbor_counts` | 2 | 2 scalar rows; 0 relationship rows |
| Present `explain` on an already pinned reader | 3 | 1 entity + 2 scalar rows |
| Missing `explain` on an already pinned reader | 1 | 0 rows |
| Ordinary present CLI/MCP call, including pinning | 5 | 1 header + S snapshots + 1 entity + 2 scalars |
| Ordinary missing CLI/MCP call, including pinning | 3 | 1 header + S snapshots |

For this one-snapshot fixture, nine MCP calls imply 39 TypeQL queries and 36
returned rows, with **1,518 UTF-8 payload bytes** measured across those calls
(excluding JSON-RPC framing). Backend scan/aggregation work can still scale with
degree; no server CPU/I/O complexity bound is claimed.

The explicit disposable regression ran in **11.79 s** at **28,784 KiB** peak
client RSS (`time -v`), including fixture publication, driver setup and spawned
CLI/MCP children (not explanation-only latency or simultaneous process memory).
The server cgroup recorded **217,894,912 bytes** peak RAM across both fixture
runs and conformance, with zero current swap before shutdown, under two CPU
cores, 2 GiB MemoryHigh, 4 GiB MemoryMax and zero swap (test limits, not
production policy).

Nine same-process MCP calls prove response parity, not retained-memory or
steady-state acceptance. S02c still owns cumulative request/deadline/cancellation
and persistent-process retention measurements. Numeric production limits remain
deferred, and this fixture is not representative full-workspace qualification.

## Evidence, corrections and next workcard

Evidence is retained at
`/data/scratch/tmp/opencode/chaosbox-s02a-20261016/`. The receipt binds the manifest,
tested source manifest, dirty patch and exact test/CLI binary hashes. Source
copies, binaries, commands, raw results and stopped disposable TypeDB data/logs
are retained; check `SHA256SUMS` there.

Canonical Chaosbox HEAD is `42f0e64fddd50f612516e39e82d5c2ea4d2ead45` plus the
recorded dirty source; Canix HEAD is
`2694bfe7cee3c5b96a7b4dfa764e5c12839e1266`. The foreign tracked Chaosbox patch
and Canix index match entry byte-for-byte. Production TypeDB stayed on PID 3067
and its activation timestamp; all test writes hit the separate loopback server,
which stopped cleanly with its data kept. This is local qualification, not a
release or installed MCP transition.

The S03a report/receipt now claim **boundary-summary parity**
(build/limit/counts/serialized lengths) rather than row-payload parity, since
seeded spans were not equality-asserted by that helper. The original
report/receipt are copied here, and the frozen 200-artifact bundle verifies
unchanged.

**Next: S02b**, complete bounded neighborhood enumeration with versioned
anchor/build/scope-bound continuation and filtering before limits/cursor
boundaries. Workcards also mark S09a as an explicit S25b native-acceptance gate;
S03b.1 investigation, S04a graph reuse, the early S05a corpus and the authorized
memory-only consumer preparation keep their existing scopes.

### Focused reproduction

In the approved cached direnv with bounded workers, start the disposable server
using the retained config (loopback port 23729, scratch-only storage), then
select a fresh `s02a_*` database:

```bash
direnv exec . env TOKIO_WORKER_THREADS=2 cargo test --locked --offline \
  -p chaosbox-store -p chaosbox-typedb -p chaosbox --lib --test explain_counts \
  --jobs 2 -- --test-threads=2
direnv exec . env TOKIO_WORKER_THREADS=2 CHAOSBOX_REQUIRE_TYPEDB=1 \
  CHAOSBOX_TYPEDB_ADDR=127.0.0.1:23729 CHAOSBOX_TYPEDB_USER=admin \
  CHAOSBOX_TYPEDB_PASSWORD_FILE=/data/scratch/tmp/opencode/chaosbox-s02a-20261016/disposable-password \
  CHAOSBOX_TYPEDB_DATABASE=s02a_explain_reproduction \
  cargo test --locked --offline -p chaosbox --test explain_counts --jobs 2 \
  -- --ignored --exact disposable_counts_and_native_cli_mcp_preserve_explanations \
  --nocapture --test-threads=1
```
