# S02a — count-only explanations

**CLI and MCP now share an exact count-only explanation path.** TypeDB returns
two aggregate scalar rows instead of relationship collections. The reference
backend counts directly over the selected build's edges.

See the [receipt](s02a-count-only-explanations.json) and
[workcards](../tasks/session-efficiency/todo.md).

## Shared contract and implementation

`GraphReader::explain` retains its entity lookup, JSON fields and successful
`null` for a missing member. For a present member it calls
`GraphQueries::neighbor_counts(build, id, all_relation_types())` once. Errors
propagate without a collection fallback. CLI `query explain` now delegates to
that same method; MCP already did.

The TypeDB implementation binds exact build membership, anchor ID and relation
types before `reduce $count = count($rel)`, once per direction. Distinct
relationship counting preserves set semantics for repeated filter values and
parallel edges. A self-loop contributes once to each direction. An empty filter
returns `(0, 0)` without a query. String values use the existing TypeQL encoder.
The common decoder now accepts scalar `Concept::Value` results as well as
attribute values; unexpected result shapes and negative counts fail explicitly.

The reference backend's direct scan allocates no relationship projection. Its
O(build edges) ceiling is deliberate and marked in source. Unsupported query
backends fail explicitly rather than defaulting to materializing neighborhoods.
The raw query API retains its existing trusted-caller ownership requirement.

This slice changes six production files and adds one focused integration target:
`chaosbox-store/src/{queries,memory_reader}.rs`,
`chaosbox-typedb/src/{common,reader}.rs`,
`chaosbox/src/{reader,query_cmd}.rs` and `chaosbox/tests/explain_counts.rs`.
The MCP dispatch needs no change.

## Verification

The initial regression failed on the old implementation at the forbidden
`neighbors_out` call. Its test backend permits only headers, keyed entity lookup
and counts; collection/export/evidence reads fail. The repaired path passes,
including count-error propagation and missing-member short-circuit behavior.

The same count cases run against both reference and disposable TypeDB:

- 1,026 nodes / 1,027 edges in the current build, with 1,024 outgoing hub-to-leaf
  calls, a parallel references edge, a reverse edge and a self-loop;
- hub counts **1,026 outgoing / 2 incoming**, isolated-node zero degree;
- historical build sharing the hub identity but containing only one edge;
- missing anchor/build and an entity present only in another build;
- empty, repeated, selective and encoded hostile-string type filters.

Three CLI cases and nine calls in one MCP process compare complete JSON values
against the same expected hub, isolated and missing-entity results. These
assertions include source spans and compiler-field nullability. They exercise
the Cargo-built executable selected at compile time, not the installed launcher.

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

The ordinary affected-test run lists the opt-in disposable test and one
pre-existing isolated publication-RSS test as ignored. The disposable test was
then explicitly executed; the unrelated RSS ignore is not acceptance evidence.
Logs retain an initial test-setup compilation error and Clippy invocation/test
lint errors as well as the successful corrected runs.

## Materialization and observed costs

The following TypeQL counts are **source-derived**, not instrumented wire-query
counters. Connection/authentication/database-metadata RPCs are excluded.

| Operation | TypeQL queries | Materialized rows |
| --- | --- | --- |
| Nonempty `neighbor_counts` | 2 | 2 scalar rows; 0 relationship rows |
| Present `explain` on an already pinned reader | 3 | 1 entity + 2 scalar rows |
| Missing `explain` on an already pinned reader | 1 | 0 rows |
| Ordinary present CLI/MCP call, including pinning | 5 | 1 header + S snapshots + 1 entity + 2 scalars |
| Ordinary missing CLI/MCP call, including pinning | 3 | 1 header + S snapshots |

For this one-snapshot fixture, nine MCP calls imply 39 TypeQL queries and 36
returned rows by that source trace. The test measured **1,518 UTF-8 payload
bytes** across those calls, excluding JSON-RPC framing. Backend scan/aggregation
work may still grow with degree. No server CPU/I/O complexity bound is claimed.

The final explicit disposable regression took **11.79 s**, with **28,784 KiB**
peak client RSS reported by native `time -v`. That includes fixture publication,
driver setup and spawned CLI/MCP children; it is not an explanation-only latency
or aggregate simultaneous process-memory measurement. The server cgroup recorded
**217,894,912 bytes** peak RAM and zero current swap before shutdown, across both
fixture runs and conformance. Test limits were two CPU cores, 2 GiB MemoryHigh,
4 GiB MemoryMax and zero swap, not adopted production policy.

Nine same-process MCP calls establish response parity, not retained-memory or
steady-state acceptance. S02c still owns cumulative request/deadline/cancellation
and persistent-process retention measurements. Numeric production limits remain
deferred, and this fixture is not representative full-workspace qualification.

## Evidence, corrections and next workcard

Evidence is retained at
`/data/scratch/tmp/opencode/chaosbox-s02a-20261016/`. The receipt binds the manifest,
tested source manifest, dirty patch and exact test/CLI binary hashes. Source
copies and binaries are retained alongside commands, raw results and the stopped
disposable TypeDB data/logs. Check `SHA256SUMS` from that directory.

Canonical Chaosbox HEAD is `42f0e64fddd50f612516e39e82d5c2ea4d2ead45` plus the
recorded dirty source; Canix HEAD is
`2694bfe7cee3c5b96a7b4dfa764e5c12839e1266`. The foreign tracked Chaosbox patch and
Canix index compare byte-identically with entry. Production TypeDB retains PID
3067 and its activation timestamp; all test writes targeted the separate
loopback server. The disposable server stopped cleanly with its data retained.
This is local implementation qualification, not a published release or installed
MCP transition.

The S03a report/receipt now correctly claim **boundary-summary parity**
(build/limit/counts/serialized lengths), not row-payload parity. Its seeded spans
were not equality-asserted by that shared helper. The original report/receipt are
copied into this bundle; its frozen 200-artifact bundle is unchanged and verifies.

**Next: S02b**, complete bounded neighborhood enumeration with versioned
anchor/build/scope-bound continuation and filtering before limits/cursor
boundaries. The workcards also record S09a as an explicit S25b native-acceptance
gate. S03b.1 investigation, S04a graph reuse, the early S05a corpus and the
already-authorized memory-only consumer preparation retain their existing scopes.

### Focused reproduction

In the approved cached direnv, use bounded workers. The retained server config
uses loopback port 23729 and scratch-only storage. With that disposable server
running, select a fresh `s02a_*` database:

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
