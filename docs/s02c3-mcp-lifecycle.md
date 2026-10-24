# S02c.3 — Ordinary MCP lifecycle and retention

Status: locally qualified, 2026-10-18. **S02c's ordinary path/request gate is
complete across S02c.1, S02c.2 and S02c.3.** Production resource acceptance remains
a separate gate.

Implementation savepoint: `0050c4631a951015247fe4d6ed947ff5ea19b9dd`.
The commit containing this report adds the ordinary-client lifecycle test,
updates existing test clients and records the [qualification receipt](s02c3-mcp-lifecycle.json).

## Transport contract

The ordinary `mcp` command keeps polling input during backend work and output
writes. A request runs in place; no detached tool task or request queue survives
its exchange. Existing graph, workspace and intelligence dispatch remains behind
the same argument and authorization checks.

- Input is newline-delimited JSON, capped at **65,536 bytes including newline**.
  A cap-plus-one read detects overflow even without newline or EOF. Partial bytes
  survive competing response events, including split UTF-8. EOF in a partial
  frame is an explicit transport error.
- JSON-RPC envelopes require an object, version `2.0`, a string method and, when
  present, an integer or control-free string ID of at most 128 UTF-8 bytes.
  Notifications produce no reply. Malformed JSON/envelopes return `-32700` /
  `-32600`; an oversized frame closes the connection with a nonzero process exit.
- One backend request runs at a time. Pings can complete during it. Another
  ordinary request receives `-32000`; a duplicate active ID receives `-32600`
  with null ID. At most one control response and one final response are retained.
  Further requests against an occupied output slot close the connection.
- `notifications/cancelled` drops only an active request whose ID equals
  `params.requestId`. String and numeric IDs are distinct. Matching cancellation
  returns `-32800` and leaves the process available for later calls. Cancellation
  after backend completion does not retract an already prepared response.
- Serialized responses are capped at **8,388,608 bytes including newline**.
  Overflow returns `-32603` with the original ID, never a truncated success.
  Both payload and envelope use the existing bounded JSON serializer. This caps
  serialized buffers; decoded backend payloads and rounded `Vec` capacity are
  not an exact allocation/RSS bound.
- Each queued output frame has one **10-second monotonic write/flush deadline**.
  Partial writes retain their offset and deadline while input is processed.
  This output deadline is separate from S02c.2's acquisition-inclusive 10-second
  path-search deadline; a path budget error can still be delivered afterward.
- EOF, input failure, oversized input, write/flush failure, output timeout and
  output overload drop active request state. Linux stdout pipes are reopened
  nonblocking: Tokio's ordinary blocking stdout worker could otherwise outlive
  a cancelled write during runtime shutdown. Terminal/file and non-Linux stdout
  retain the existing fallback; their shutdown behavior is not qualified here.

Cancellation remains cooperative at async poll boundaries. This change does
not preempt synchronous computation. The scoped `read_view` transport retains
its separate admission/revocation contract. Only its pure serializer becomes
publicly reusable; no read authority is granted by that helper.

## Required live qualification

TypeDB CE 3.13.0 ran in a disposable test deployment with gRPC at
`127.0.0.1:24739`; its native monitoring endpoint exposed active transactions.
The fresh `s02c3_qualified_final` database contains 653 nodes / 650 relationships,
including a 650-edge hub, an isolated node and an entity with a 600,000-byte name
and qualified name. Build: `build:ddc833021246`.

Before each active-read failure, the test observed a real gRPC read transaction
in that database. The final run returned to zero reads within the two-second
assertion window:

| Event | Observed active reads | Reads after cleanup | Measured cleanup |
|---|---:|---:|---:|
| Matching cancellation | 1 | 0 | 7 ms |
| Input EOF | 1 | 0 | 2 ms |
| Oversized input | 1 | 0 | 14 ms |
| Closed stdout, followed by ping | 1 | 0 | 2 ms |
| Output overload | 1 | 0 | 14 ms |

An unrelated string cancellation ID preserved the numeric-ID search, as proven
by an intervening ping and another observed active read. Matching cancellation
then returned `-32800`; native process-FD/TCP-inode observations found zero
remaining TCP sockets, and a subsequent ping succeeded.

The wide lookup completed its three backend queries and released its read before
its large response stalled on an unread OS pipe. EOF ended that process cleanly
in 2 ms. With stdin still open, its fixed output deadline ended the process with
an error after another 9,943 ms. A separate silent-TCP test proves that cancelling
or ending an acquisition also closes the actual connector socket.

These observations establish local/remote cleanup for the fixture's graph reader.
They are not a server cancellation-acknowledgement protocol, a qualification of
every federation provider, or a production workload/resource acceptance result.

## Persistent-process observations

One ordinary MCP process handled **147 path calls**: seven batches of 20
found/absent/complete-negative calls plus one exhausted search per batch. The
first 21 calls form the warmup. Every batch ended with zero server read
transactions and zero process-owned TCP sockets.

- RSS samples (KiB): `32016, 31224, 23948, 14892, 14260, 14476, 14632`.
- Anonymous RSS (KiB): `5164, 5156, 5148, 5188, 5188, 5376, 5576`.
- All samples: **4 threads, 10 file descriptors, zero swap**.
- The test permits at most 16 MiB RSS growth over warmup, two additional threads
  and two additional FDs. These are fixture regression tolerances, not an
  operator-approved production RAM budget. Finite repetitions do not prove
  indefinite retention behavior or allocator/history attribution.

The disposable server's recorded cgroup peak was 445,706,240 bytes with zero
swap. The final lifecycle worker consumed 34.991 seconds wall time and a native
175.7M peak. Test workers used two Cargo jobs, two Tokio workers, 200% CPU,
`MemoryHigh=2G`, `MemoryMax=4G`, and `MemorySwapMax=0`.

## Verification and reproduction

- **148 affected tests passed**, with 10 opt-in cases ignored in that aggregate.
  Required lifecycle, path-client and explanation cases were then run explicitly:
  **three passed, zero ignored**. Seven other opt-in cases were not run.
- Fresh CLI, fresh MCP and persistent MCP still agree across 14 path cases per
  transport; all three report query-budget exhaustion as an error. Existing and
  fresh historical readers retain the old path after publication.
- Existing explanation qualification passes 9 ordinary MCP calls and 3 CLI
  cases. Intelligence/federation process tests retain stdin until replies arrive,
  so EOF exercises cleanup rather than implicitly acting as a drain request.
- Warnings-denied scoped Clippy, treefmt CI and whitespace checks pass. Local
  tests cover exact frame boundaries, split UTF-8, bounded response overflow,
  local drop guards and a fixed backpressure deadline despite notifications.

Use the commands in `worker-resources.txt`, with a fresh explicitly named
disposable database, for the required live run:

```bash
cargo test --locked --offline -p chaosbox --test mcp_lifecycle --jobs 2 \
  disposable_ordinary_mcp_cleanup_and_retention -- --exact --ignored --nocapture
```

It requires `CHAOSBOX_REQUIRE_TYPEDB=1`, `CHAOSBOX_TYPEDB_ADDR`, an `s02c3_*`
`CHAOSBOX_TYPEDB_DATABASE`, a disposable `CHAOSBOX_TYPEDB_PASSWORD_FILE`, and
`CHAOSBOX_TEST_MONITOR_ADDR`. Keep the recorded cgroup limits. Production
credentials and a missing/ignored live test cannot substitute for this gate.

## Source and handoff

Evidence: `/data/scratch/tmp/opencode/chaosbox-s02c3-20261018/`. The directory
retains the tested source archive, selected exact executables, raw commands,
logs, native diagnostics, preservation snapshots and checksums. The source
archive includes Git-filtered tracked/untracked source and packaging files,
excluding `docs/` and `tasks/`; its SHA-256 is
`13418e8214ff10a954ad19f77f937339670f18cb74bbdff37bc9124de162f107`.
The CLI SHA-256 is
`9d47955929934532cbabd29b72c48adac5023d85dec5dbfaa3399b1ff6721ccf`.

Qualification used the dirty canonical checkout, including foreign source
changes. It is not clean-release qualification. Concurrent documentation commits
advanced HEAD from `a91cf36e` through `55847b0a` before the implementation
savepoint; archived source bytes still compared equal. Foreign formatting,
the unstaged export and unrelated source edits remain. The foreign unpublished
native helper in `tests/navigation_live.rs` has the same stdin-lifetime adapter
in place, retained unstaged with that existing addition and in the archive.
Do not stage that whole file as part of this workcard.

Canix's concurrent commits rebased its existing staging in `.gitattributes` and
the OpenCode candidate package; before/after index patches are retained. This
session made no Canix/index writes. Production TypeDB's PID/start identity
compared unchanged. The disposable server was stopped with SIGTERM. Savepoints
remain local; no consumer promotion or host activation was performed.

Next: **S04a graph connection/query reuse**, preserving per-request publication
pinning and lifecycle/retention checks. Any intentionally retained connection
pool needs its own bounded lifetime/count contract; this baseline releases all
request-owned TCP connections. S03b.1's knowledge-validation
investigation can proceed independently. S01b still gates schema/storage work;
release and whole-stack resource/quality acceptance retain their own gates.
