# S02c.1 — aggregate-bounded path search

The shared, already-pinned path reader is locally qualified. CLI and ordinary
MCP dispatch already call `GraphReader::path`, so both use this implementation.
**Parent S02c remains open:** transport cancellation/framing, deadlines including
connection/build acquisition, live TypeDB path qualification and persistent-MCP
retention measurements belong to S02c.2.

See the [receipt](s02c1-bounded-path-search.json) and the
[parent workcard](../tasks/session-efficiency/todo.md#--s02c--bounded-path-expansion-and-request-cost).

## Implementation and contract

Baseline HEAD is `d42a93c04efca667a8b5b05c4b9324465c26e1ed` in the canonical
Chaosbox checkout. This slice changes `reader.rs`, adds `reader_path.rs`, exports
`PathBudget` from `lib.rs`, and adds `tests/path_budgets.rs`. The foreign
`write_export_json` export in `lib.rs` remains outside the scoped savepoint.

The former BFS requested complete outgoing/incoming neighborhoods before its
visit guard ran. It now reuses `GraphQueries::neighbor_page`: at most 200 rows per
read, ordered by relationship ID, with exclusive continuation in each direction.
All operations retain the reader's immutable build ID. No export, evidence load
or unbounded-neighborhood fallback is used.

Default trusted-library limits are:

| Cost | Limit |
| --- | ---: |
| Distinct visits, including the root | 100,000 |
| Accepted relationship rows across all pages | 20,000 |
| Backend operations, including endpoint and empty-page reads | 500 |
| Aggregate serialized endpoint/page JSON | 8 MiB |
| Entire already-pinned search | 10 seconds |

These are request-work defaults, not accepted production RAM targets. Repeated
edges, both directions, self-loops and empty-page operations still incur costs.
An overflow-detection read can return one row beyond the remaining row allowance;
it fails explicitly, so at most `max_rows + 1` rows are fetched before a row-budget
error from a conforming backend. Page size never exceeds 200. JSON accounting
uses the existing bounded serializer after decoding; it is not a wire-byte,
database-scan or pre-allocation memory bound. Trusted library callers may select
another `PathBudget`; MCP arguments do not expose it.

`Some(path)` is a shortest undirected path within `max_hops`; tie-breaking follows
direction then relationship-ID page order. `None` means complete negative
reachability within the hop bound, or an endpoint absent from this build. Budget
and deadline exhaustion are errors, never negative results. Equal endpoints now
require actual membership; a missing self endpoint cannot fabricate a path.
Endpoints must be nonempty, control-free and at most 256 bytes.

Over-returned, wrong-anchor and nonprogressing backend pages fail closed. The
search uses one monotonic deadline, yields between operations, and starts no
background worker. Timeout or dropping its future drops the pending backend
future. Tests establish this behavior for an instrumented backend, not remote
server cancellation acknowledgement. Node-at-a-time round trips remain the
deliberate ceiling; frontier/connection reuse belongs to S04a.

## Fresh verification

The focused test target passed all 10 cases. The affected aggregate then passed
**98 tests**, with two disposable TypeDB cases explicitly ignored. That aggregate
covers library and binary/MCP tests, graph navigation, neighborhood backend and
continuation contracts, scoped read views, and the new path target. Ignored cases
do not qualify the live backend.

The new instrumented fixture has 450 outgoing hub relationships. It rejects any
unbounded neighborhood/export access, bounds requested pages, finds a target on
the third page, and independently counts backend operations. Cases cover reverse
traversal, complete negative/hop-bounded results, each work budget, exact row
completion, missing self endpoints, malformed backend pages, one whole deadline
across slow reads, pending-future cleanup and successful reuse after cancellation.

Native tests and warnings-denied Clippy ran in the harness-loaded project direnv
environment (`IN_NIX_SHELL=impure`, project `DIRENV_DIR`), under user systemd scopes
with `MemoryHigh=2G`, `MemoryMax=4G`, `MemorySwapMax=0`, `CPUQuota=200%`, two Cargo
jobs and two Tokio workers. The initially attempted nested `nix develop` worker
was denied; it was not run and is not verification evidence. No pre-change RED
execution is claimed. No environment refresh or new Nix realization was needed.

Commands, with the systemd scope above applied to Cargo:

```sh
cargo test --locked -p chaosbox --lib --bin chaosbox --test path_budgets --test reader_navigation --test neighborhood_pages --test neighborhood_continuation --test read_views --jobs 2 -- --test-threads=2
cargo clippy --locked -p chaosbox --lib --test path_budgets --jobs 2 -- -D warnings
treefmt --ci crates/chaosbox/src/reader.rs crates/chaosbox/src/reader_path.rs crates/chaosbox/src/lib.rs crates/chaosbox/tests/path_budgets.rs
git diff --check
```

Treefmt reports zero changes, Clippy passes, and the whitespace check is clean.
Evidence is retained at `/data/scratch/tmp/opencode/chaosbox-s02c1-20261017/`,
including the complete affected-test output and tested-source archive. Qualification
uses the dirty canonical checkout, not a clean isolated release candidate.
Unrelated source/staging is preserved; this slice performs no publication,
consumer pin transition, activation or service restart.

## Next-session handoff

Continue as S02c.2 from the commit containing this report. Recheck HEAD and dirty
state, then load this report plus the S02b.3 ordinary-client report. The shared
reader does not consume MCP cancellation notifications, bound ordinary stdio
frames/backpressure, or include backend acquisition in its deadline. Implement
those transport controls with compatible found/null/error semantics and qualify
fresh CLI, fresh MCP and repeated persistent MCP against disposable TypeDB.
Measure retention and remote-read cleanup rather than extrapolating from these
mock-future checks. Parent S02c must stay open until that evidence exists.
