# S02c.4 — Bounded ordinary-MCP admission

Status: locally qualified, 2026-10-18. Signed implementation/test savepoint:
`75198f14c21df448cc14b033b9fbd2a070e781be`.
The [receipt](s02c4-mcp-admission.json) binds the final tested source and evidence.
This closes the overlapping-call review finding for the recorded ordinary-MCP
fixtures. Exact-publication follow-up reads remain **S02b.4**.

## Admission and request lifetime

The ordinary stdio loop now admits overlapping calls into a FIFO instead of
rejecting every overlap with `one request is already in flight`. The existing
`Frames` owner retains raw frames, typed IDs and admission deadlines. `exchange`
still owns at most one active backend future.

| Control | Selected bound |
|---|---:|
| Pending requests | 8 |
| Aggregate pending raw-frame bytes | 256 KiB |
| Admission wait after a complete frame is accepted | 10 seconds |
| Input frame, including newline | Existing 64 KiB |
| Serialized response, including newline | Existing 8 MiB |
| Output deadline from response creation | Existing 10 seconds |
| Pending control responses | 8, plus the frame being written |
| Reusable graph drivers | Existing maximum of 1 |
| Graph driver idle expiry | Existing 30 seconds |

The byte bound covers raw frames, excluding allocation overhead. These protocol
limits and the fixture tolerances below are distinct from production RAM
acceptance. A queued call owns no backend future, credentials, policy decision,
publication pin or query result. Dispatch rereads credentials and pins the
publication through the existing shared acquisition path. Admission wait has its
own deadline; backend/path deadlines still start at dispatch.

Expired frames receive `-32000` with `MCP admission wait deadline exceeded`
without backend acquisition. Overflow receives `-32000` with
`MCP admission queue full`; admitted calls remain in order. Pings and cancellation
bypass admission. Cancellation matches the typed ID: numeric `8` and string `"8"`
are distinct. Queued cancellation frees frame/count capacity and returns `-32800`
without disturbing the active read. Matching active cancellation drops that
future and its driver. Duplicate outstanding IDs receive `-32600` with a null
response ID; IDs become reusable after their response flushes.

Control responses drain before `exchange` returns. The final response and the
currently written frame keep their original monotonic output deadlines, so
incoming controls cannot extend a blocked write. EOF, oversized input, closed
output and response-queue overload terminate the connection and drop its queued
frames and in-place backend future. No input task or detached tool task was added.

## Regressions and native observations

The original stalled-acquisition regression failed with the pre-change `-32000`
busy response instead of queued cancellation's `-32800` (`red.log`). It now passes
with FIFO dispatch and typed queued/active cancellation. Count exhaustion, byte
exhaustion, capacity recovery, duplicate IDs and admission expiry pass. The expiry
case observes no second acquisition during the queued wait.

Final review extended the backpressured-output regression to reuse the active ID
after its final response flushed. That exposed an overly long duplicate-ID
lifetime (`id-reuse-red.log`); the final implementation releases the ID at flush.
The same test now passes in `affected-qualified.log`.

The new required disposable-TypeDB case proves:

- Eight queued status calls start no graph query while a wide lookup response is
  blocked. After publication moves from `build:ddc833021246` to
  `build:ac63871ab3f5`, all eight select the replacement build at dispatch.
- A credential-file replacement while another status call waits is rejected at
  dispatch; zero process-owned TCP sockets remain. A queued `tools/list` succeeds,
  and restored credentials allow a subsequent status call.
- Seven eight-call bursts each use 16 queries, eight transactions and zero new
  connections after warmup. RSS samples are
  `33336, 33344, 33348, 33352, 33360, 33360, 33360` KiB: 24 KiB observed growth.
  Every sample has seven threads, 15 FDs, one TCP socket and zero swap.

The existing native cleanup gate now queues a `tools/list` behind every active
read and uses a bypass ping as an admission barrier. After active cancellation,
that queued call succeeds. Cancellation, EOF, oversized input, closed output and
output overload each move from an observed active TypeDB read to zero; observed
cleanup times are 17, 1, 2, 2 and 17 ms. Wide-response EOF exits in 2 ms; the
blocked-output deadline fires after another 9,946 ms. The 147-call error-eviction
fixture retains four threads, ten FDs, zero TCP sockets and zero swap per sample.

Connection reuse and header batching retain their earlier behavior: 20 lookups use
one connection, 40 transactions and 60 queries. The driver is present after 16
seconds, a ping does not renew it, and the 31,114 ms idle sample has zero TCP
sockets. Fresh CLI, fresh MCP and persistent MCP retain their 14 path cases and
29 historical neighborhood pages / 4,124 delivered rows per transport. Ordinary
explanations and required backend conformance pass. Those page assertions do not
qualify historical endpoint hydration or new-anchor expansion; S02b.4 owns both.

## Verification and provenance

**152 affected tests passed**, with 13 opt-in cases ignored in the aggregate.
Seven selected opt-in cases then passed explicitly, along with required backend
conformance: **eight required disposable-TypeDB cases passed, zero ignored**.
The other six aggregate opt-in cases and 18 other backend live cases were not
selected. Scoped warnings-denied Clippy, treefmt CI and whitespace checks pass.
The initial Clippy failure for a long fixture function was resolved by extracting
focused fixture helpers; no new lint suppression was added.

Qualification used the harness-approved project environment with two Cargo jobs,
two Tokio workers, 200% CPU, `MemoryHigh=2G`, `MemoryMax=4G` and
`MemorySwapMax=0`. TypeDB CE 3.13.0 used disposable loopback ports 24779/24781.
Its final-run recorded peak was 454,725,632 bytes with zero swap. Final native
diagnostics show zero active transactions in every database.

Evidence: `/data/scratch/tmp/opencode/chaosbox-s02c4-20261018/`, including
`commands.txt`, exact executables, raw logs, native projections, source archives,
preservation patches and `qualification.sha256`. Only `*-qualified.log` native
results bind the final ID-lifetime revision; earlier failed and successful runs
remain available. `finalization.json` binds the frozen manifest and savepoints.

- Baseline HEAD: `97e9ec7ac6fe449447f78de5d9363d2b37d4603c`.
- Final source archive SHA-256:
  `2a37103b1d842c9a7e905768db3b53df9a90f78b2eba6ce33bd9b95bc806a061`.
- Final CLI SHA-256:
  `e68041d16fa7bf7f58bc47645db025b96449149d5324c554eeca09bb1474ff9c`.

The Git-filtered archive includes preserved foreign dirty/untracked source and
excludes `docs/` and `tasks/`; report/roadmap snapshots are retained separately.
Staged/committed code patches and tested/post-savepoint archives compare exactly.
Configured author/committer and OpenPGP signature are verified. Foreign Chaosbox
tracked edits compare unchanged. Canix had concurrent commits and edits; its
entry/finalization indexes were empty and this task made no Canix source/index
writes. The revised roadmap remains untracked.

Both disposable-server invocations stopped with SIGTERM, and their listeners are
gone. Production TypeDB's PID/start identity is unchanged. Savepoints remain local;
no consumer promotion, activation or production service restart occurred.

## Next workcard

**S02b.4 — exact-publication follow-up and recursive historical traversal.** Reuse
`GraphReader::published` so ordinary lookup, explanation and evidence hydration,
and a new neighborhood anchored at a historical endpoint, can select the page's
exact publication. Qualify independently expected nonempty evidence and reject
foreign, unavailable and conflicting builds without current fallback. Preserve
the admission, lifecycle, authorization and connection bounds qualified here.

The roadmap also records S05's separate identity, complete-generation admission
and multi-snapshot migration slices, plus the owning Pklr → Fleetix → Toolbelt →
Canix release dependency chain. S01b allocation/history attribution, S05a's corpus,
production resource acceptance and exact consumer release qualification remain
open. Exact cumulative session-context telemetry was unavailable; this session
closes one workcard and hands off S02b.4.
