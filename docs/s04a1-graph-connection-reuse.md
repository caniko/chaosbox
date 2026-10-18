# S04a.1 — Ordinary MCP graph connection reuse

Status: locally qualified, 2026-10-18. Signed implementation/test savepoint:
`39d94d6022fa03cd6154dda16e41df69701d8aed`.
The [receipt](s04a1-graph-connection-reuse.json) records the exact candidate.
Parent S04a continues with S04a.2, bounded graph query batching/reuse.

## Ownership and lifetime

The ordinary MCP server keeps **at most one reusable TypeDB driver**. Connected
`TypeDbReader` clones share that driver through `Arc`; every graph query still
opens and closes its own read transaction through the existing `read_rows` path.
No build header, graph row, query result, transaction or publication pin is cached.

- Each prepared graph call rereads the operator's credential file. Reuse requires
  the same address, database, username and normalized password bytes. A changed
  identity drops the old driver before connecting. The four identity fields have
  a focused unit regression; live qualification replaces the credential file.
- Every request pins its current publication afresh. Neighborhood continuation
  selects and validates its exact historical build through the existing prepared
  request. Repository membership and scoped-view authority remain distinct.
- A graph call takes ownership of the reusable slot. Matching cancellation drops
  its connection clones; a graph-dispatch error also evicts the driver. A later call
  can acquire a new driver. There is no automatic retry that hides failed work.
- Successful graph completion sets one **30-second monotonic idle deadline**.
  Waiting for input also waits for expiry, and every graph reuse checks the
  deadline again. Pings, notifications and protocol responses do not renew it.
  Expiry during a non-graph protocol exchange is serviced on return to the input
  loop; that exchange retains the existing fixed 10-second output deadline.
  This idle policy is not a hard timer that preempts synchronous computation.
- Workspace/intelligence dispatch releases the ordinary graph slot. EOF,
  framing/output failure, output timeout and overload drop all server-owned
  connection state. Cancellation after backend completion still leaves the
  prepared response intact, as specified by S02c.3.
- Path acquisition, credential checks and pinning still execute inside
  `PreparedPath::read`'s existing logical work budget and whole-request deadline.
  A reused connection still consumes the acquisition operation. The shared CLI
  preparation and standalone reader calls keep their existing behavior.

This replaces S02c.3's zero-retained-TCP baseline only for successful ordinary
graph calls. One cached driver is a client ownership bound, not a universal TCP
count for every TypeDB cluster topology. The disposable single-server fixture
observed one retained TCP socket. Scoped `read_view` admission/revocation and
knowledge-generation validation were not changed.

## Native before/after comparison

The same fresh three-node/two-edge fixture was read by the retained S02c.3 binary
and this candidate, serially against TypeDB CE 3.13.0. Native diagnostics count
`CONNECTION_OPEN` globally and `TRANSACTION_QUERY` for the fixture database; no
other graph workload ran on that disposable server during either interval.

| Twenty successful lookups | S02c.3 baseline | S04a.1 candidate |
|---|---:|---:|
| New driver connections | 20 | 1 |
| Graph queries | 60 | 60 |
| Retained TCP sockets at sample | 0 | 1 |
| Threads at sample | 4 | 7 |
| File descriptors at sample | 10 | 15 |

The connection regression first failed behaviorally on the pre-change executable:
both sides opened 20 connections. The candidate then passed. This establishes
connection reuse, not fewer queries or a measured latency improvement.

One candidate MCP process handled **140 successful lookups in seven batches**:

- New connections per batch: `1, 0, 0, 0, 0, 0, 0`; 60 graph queries per batch.
- RSS (KiB): `30512, 30512, 30520, 30556, 30556, 30556, 30572`.
- Anonymous RSS (KiB): `4844, 4844, 4852, 4888, 4888, 4888, 4904`.
- All samples: 15 FDs, one TCP socket, zero swap; threads ranged from 7 to 8.
- The driver was present after 16 idle seconds. An intervening ping did not renew
  it: after 31 seconds the process had zero TCP sockets, four threads and ten FDs.
  A following graph request reacquired successfully.

The same client observed an empty successor publication instead of its old
entity, resumed an old neighborhood cursor against the original build, rejected
a cursor in another repository, failed after bad credential replacement with
zero retained TCP sockets, and recovered after restoring the credential file.

## Retained lifecycle and parity gates

The existing 653-node/650-edge lifecycle fixture now primes a successful graph
call before each active-read failure. Native monitoring observed an active read
then zero reads in all five warmed-connection cases:

| Event | Cleanup |
|---|---:|
| Matching cancellation | 17 ms |
| EOF | 2 ms |
| Oversized input | 2 ms |
| Closed stdout followed by ping | 2 ms |
| Output overload | 16 ms |

Cancellation also left zero process-owned TCP sockets and a working subsequent
ping. The wide lookup released its read before output stalled; EOF exited in
3 ms, and blocked output expired after another 9,925 ms. The real silent-TCP
acquisition cancellation/EOF regression also passes.

The persistent path fixture passed 147 calls, ending each batch with an exhausted
request that evicted the reused driver. Its RSS samples were
`32196, 32232, 32232, 32236, 32240, 32268, 32272` KiB, with four threads, ten FDs,
zero TCP sockets and zero swap at every batch boundary.

Fresh CLI, fresh MCP and persistent MCP retained exact parity across:

- 14 path cases per transport, explicit query-budget exhaustion and historical pins;
- 29 neighborhood pages / 4,124 delivered rows per transport, with exact
  relationship IDs, directions, types and endpoints after pointer movement;
- nine explanation MCP calls and three CLI cases.

## Verification and reproduction

**153 affected tests passed**: 148 ordinary/shared tests and five TypeDB library
tests. Eleven opt-in cases were ignored in that aggregate; the five required
reuse, lifecycle, path, neighborhood and explanation cases then passed explicitly
with zero ignored. Six other opt-in cases were not run. Scoped warnings-denied
Clippy, treefmt CI and whitespace checks passed.

Commands and native worker observations are retained in `commands.txt` and
`worker-resources.txt` in the evidence directory. Use the approved project
environment and a fresh disposable database for each live test. The reuse case
requires `CHAOSBOX_REQUIRE_TYPEDB=1`, an `s04a1_*` database,
`CHAOSBOX_TYPEDB_ADDR`, `CHAOSBOX_TYPEDB_PASSWORD_FILE`,
`CHAOSBOX_TEST_MONITOR_ADDR` and `CHAOSBOX_BASELINE_TEST_BIN`:

```bash
cargo test --locked --offline -p chaosbox --test mcp_lifecycle --jobs 2 \
  disposable_ordinary_mcp_connection_reuse_is_pinned_and_bounded \
  -- --exact --ignored --nocapture
```

Workers used two Cargo jobs, two Tokio workers, 200% CPU, `MemoryHigh=2G`,
`MemoryMax=4G` and `MemorySwapMax=0`. The disposable server's recorded peak was
444,579,840 bytes with zero swap. Fixture tolerances remain 16 MiB RSS growth,
two additional threads and two additional FDs over warmup. These observations
do not establish a production RAM budget, indefinite retention, server scan/wire
bounds or the unresolved S01b allocation/history attribution.

## Exact source and handoff

Evidence: `/data/scratch/tmp/opencode/chaosbox-s04a1-20261018/`, with
`qualification.sha256`, source, exact executables, raw logs, native diagnostics,
preservation patches and roadmap snapshots. Qualification used the dirty canonical
checkout at baseline `6cce3cc50e5e5f7d88a034880c03322f0cdbeed5`, including preserved
foreign source work. This is not clean-release or installed-consumer qualification.

- Source archive SHA-256:
  `3859ea58a193e0952bbfb887803cb728711ede5ea8d20c25c11b0d771e86f0ac`.
  The Git-filtered archive excludes `docs/` and `tasks/`; reports and roadmap
  snapshots are separately retained. Per-file hashes compare source after commits.
- Candidate CLI SHA-256:
  `aff30c339f1e8089426c1bfa2ee7f9e71fa996f5f81a2b39328f5431e0f2365c`.
- Baseline CLI SHA-256:
  `9d47955929934532cbabd29b72c48adac5023d85dec5dbfaa3399b1ff6721ccf`.

The implementation commit contains only the reader ownership/config identity,
ordinary acquisition/dispatch and lifecycle tests. Foreign tracked edits and
Canix staging compared byte-identical through qualification. Production TypeDB's
PID/start identity remained unchanged; the disposable server was stopped with
SIGTERM. Savepoints are local. No publication, consumer promotion, activation or
production-service restart was performed.

Next: **S04a.2 — bounded graph query batching/reuse**. Start from this connection
contract and measure query/transaction reduction while retaining exact pins,
work accounting, complete paging and cleanup. S03b.1 can investigate knowledge
independently; S01b still gates substantial schema/storage changes.
