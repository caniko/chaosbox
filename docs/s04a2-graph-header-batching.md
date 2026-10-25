# S04a.2 — Pinned graph-header read batching

Status: locally qualified, 2026-10-18. Signed implementation/test savepoint:
`ea51f2ed7ae638787e28c679ee4fc30ab44999e6`.
The [receipt](s04a2-graph-header-batching.json) identifies the tested candidate.
Together with [S04a.1](s04a1-graph-connection-reuse.md), this closes S04a's
fixture-backed graph connection/query-reuse gate. S04 continues with knowledge
retrieval and reuse after S03b.1/S03b.2.

## Shared batch and lifetime

`TypeDbReader::build_header` now opens one read transaction and uses the existing
`common::read_rows_in` helper for both the selected publication header and that
build's snapshot fingerprint. Both `active_build` and `published_build` use this
shared path. A missing header returns after the first query.

The transaction belongs to that header read and drops on return, error or
cancellation. It uses the existing 60-second backend transaction timeout for the
whole batch. Ordinary path requests retain their tighter acquisition-through-search
deadline. The snapshot query still selects only the exact build ID returned by the
header query; both reads now observe one backend read snapshot.

Fresh per-request pins, historical continuation, repository/credential checks,
optional coverage/sealing fields, deterministic snapshot ordering and error
propagation keep their existing contracts. Logical work accounting is unchanged:
acquisition and the build-header operation each consume their existing operation,
including when the driver is reused. Native query/transaction counts are separate
measurements from those logical operations.

S04a.1's single-driver ownership, credential reread, 30-second monotonic idle
deadline and failure/cancellation eviction continue to apply. The batch retains
no transaction, header or query result across requests. This replaces S04a.1's
one-transaction-per-query description specifically for the two-query header read.

## Native comparison

The retained S04a.1 executable and this candidate read the same fresh fixture
serially on TypeDB CE 3.13.0. Diagnostics count successful `TRANSACTION_OPEN` and
`TRANSACTION_QUERY` actions for the selected database. `CONNECTION_OPEN` is
server-global; the disposable server had no other graph workload in either
measurement interval.

| Twenty successful lookups | S04a.1 baseline | S04a.2 candidate |
|---|---:|---:|
| New connections | 1 | 1 |
| Read transactions opened | 60 | 40 |
| Graph queries | 60 | 60 |
| Retained TCP sockets at sample | 1 | 1 |

This saves one transaction open per successful pin, a **33.3% reduction** for the
measured lookup workload. Query count stays at three per lookup: header,
fingerprint and entity. Latency and large-corpus scan/wire savings were not
measured. The behavioral regression first failed on the pre-change implementation
with 60 candidate transaction opens instead of 40 (`red.log`).

The required batch case also passed:

- Zero- and one-snapshot status responses equal the baseline exactly; each uses
  one transaction and two queries.
- After pointer movement, a fresh reader resolves the exact historical header,
  including its original generation, snapshot and evidence-sealed flag.
- Foreign/missing historical headers return no build. The three historical reads
  use three transactions and four queries in total.
- Current lookup sees the replacement publication, while an old neighborhood
  cursor continues its original build. Wrong-repository continuation fails.
- Replacing the credential file with a bad password evicts the driver; restoration
  succeeds. A missing current header uses one transaction and one query and leaves
  zero process-owned TCP sockets.

## Lifecycle, traversal and retention

One persistent MCP process completed 140 successful lookups in seven batches.
Every batch used 40 transactions and 60 queries; connection opens were
`1, 0, 0, 0, 0, 0, 0`. RSS samples were
`30328, 30332, 30332, 30336, 30340, 30340, 30340` KiB. Each sample had seven
threads, 15 FDs, one TCP socket and zero swap. The idle test retained the driver
at 16 seconds; a ping did not renew it. At 31,103 ms it had zero TCP sockets,
four threads and ten FDs, and a following graph call reacquired successfully.

Warmed active-read cleanup passed for matching cancellation (15 ms), EOF
(55 ms), oversized input (4 ms), closed output with ping (47 ms), and overload
(14 ms). Native monitoring observed an active read and then zero in every case.
Cancellation also released process-owned TCP sockets and preserved a subsequent
ping. The real stalled acquisition regression passed in the affected suite.
Wide-output EOF exited in 3 ms; blocked output expired after another 9,927 ms.

The 147-call path-retention fixture ended each batch with explicit budget
exhaustion and driver eviction. RSS samples were
`32308, 32356, 32356, 32364, 32364, 32368, 32368` KiB; every sample had four
threads, ten FDs, zero TCP sockets and zero swap.

Fresh CLI, fresh MCP and persistent MCP preserved 14 path cases per transport,
explicit query-budget exhaustion, and exact historical reads. Each transport
enumerated 29 neighborhood pages / 4,124 rows with exact relationship IDs,
directions, types and endpoints after pointer movement. Nine MCP explanation
calls and three CLI cases passed. Required backend conformance also passed,
including current/historical snapshot and membership-prefix boundaries.

## Verification and evidence

**153 affected tests passed**: 148 ordinary/shared tests and five TypeDB library
tests. Twelve opt-in cases were ignored in that aggregate. The six required
batch/reuse/lifecycle/path/page/explanation cases then passed explicitly, as did
the required backend conformance case: **seven passed, zero ignored**. Six other
aggregate opt-in cases and the other 18 backend live tests were not run. Scoped
warnings-denied Clippy for both affected crates, treefmt CI and whitespace checks
passed.

Evidence is retained at
`/data/scratch/tmp/opencode/chaosbox-s04a2-20261018/`: `commands.txt`, exact
executables, source/archive hashes, raw test logs, native result projections,
preservation patches and `qualification.sha256`. The commands ledger gives the
bounded invocations and required fresh database names. The new gate is:

```bash
cargo test --locked --offline -p chaosbox --test mcp_lifecycle --jobs 2 \
  disposable_ordinary_mcp_header_batches_reduce_transactions \
  -- --exact --ignored --nocapture
```

It requires `CHAOSBOX_REQUIRE_TYPEDB=1`, a fresh `s04a2_*` database, explicit
disposable credentials/monitoring and the exact S04a.1 baseline in
`CHAOSBOX_BASELINE_TEST_BIN`. Native workers used the approved project
environment, two Cargo jobs, two Tokio workers, 200% CPU, `MemoryHigh=2G`,
`MemoryMax=4G` and `MemorySwapMax=0`. The disposable server's recorded peak was
383,610,880 bytes with zero swap.

Qualification used the dirty canonical checkout at baseline
`cde0c6e30aaf11fc2d8205aa5e335f90875aa265`, including preserved foreign source
work. The Git-filtered source archive excludes `docs/` and `tasks/`; report and
roadmap snapshots are retained separately.

- Tested source archive SHA-256:
  `e8c7e61e21d59d99f49525e2c415f7baf412955d227493ca3c8798b4d75b4287`.
- Candidate CLI SHA-256:
  `9abc00424401db5b703d198a38ef5083042dcb725ed70942d3f2214560bf6c75`.
- S04a.1 comparison CLI SHA-256:
  `aff30c339f1e8089426c1bfa2ee7f9e71fa996f5f81a2b39328f5431e0f2365c`.

The staged and committed implementation patches compare exactly, and source
bytes/file inventory compare equal after that savepoint. Foreign Chaosbox tracked
edits are preserved. Concurrent Canix work added two staged files; all seven
entry-staged paths still compare byte-identical. This session made no Canix
source/index writes. Production TypeDB's PID/start identity is unchanged; the
disposable server stopped with SIGTERM and its listeners are gone. Savepoints
remain local. No consumer promotion, activation or production restart occurred.

## Follow-up gates

An exploratory two-snapshot publication failed under the existing schema:
`graph-build`'s `owns snapshot-id` has the implicit `@card(0..1)` ceiling. The
unchanged baseline schema and failure are retained in `baseline-snapshot-schema.txt`
and `green.log`. Supported zero/one-snapshot cases passed; multiple-snapshot
TypeDB publication remains an S05 contract/migration issue, gated by S01b.

These fixtures establish transaction reuse and bounded observed retention. They
do not establish production RAM acceptance, allocation/history attribution,
indefinite retention, million-fact performance, a clean release or the installed
consumer's executable provenance. Existing fixture growth tolerances remain
16 MiB RSS, two threads and two FDs. Exact cumulative session-context telemetry
was unavailable.

Next workcard: **S03b.1 — knowledge validation/current-eligibility investigation**.
Trace complete-generation validity before selecting targeted knowledge reads;
S01b and S05a remain prerequisites for subsequent schema/storage work.
