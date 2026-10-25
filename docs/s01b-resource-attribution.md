# S01b — resource attribution

The accessible read-only observation pass is complete. **S01b is not green:**
allocator/object ownership and retained-history size remain unresolved. The
operator explicitly deferred numeric resource limits until the repaired read
path has been measured. This report supports the bounded-read fixes in S02; it does
not justify a production TypeDB memory limit or major storage/schema changes.

See the [machine-readable receipt](s01b-resource-attribution.json),
[S01 baseline](s01-efficiency-baseline.md) and
[session plan](../tasks/session-efficiency/plan.md).

## What the memory figures actually contain

At 13:56:38 UTC on 2026-10-16, PID 3067 and its service cgroup reported:

| Component | Observation | Interpretation |
| --- | ---: | --- |
| Process RSS | 3,501,696 KiB / 3.34 GiB | Resident mapped memory, mostly anonymous |
| Process anonymous RSS | 3,467,752 KiB | Not an allocator/object-level breakdown |
| Process swap | 23,821,648 KiB / 22.72 GiB | Large existing swapped anonymous footprint |
| Anonymous RSS plus swap | 26.03 GiB | Approximate footprint, not a working-set requirement |
| Cgroup current RAM | 7,523,958,784 bytes / 7.01 GiB | Includes cache and compressed swap |
| Cgroup anonymous | 3,550,978,048 bytes | Resident anonymous charge |
| Cgroup file | 756,936,704 bytes | File cache, not the RocksDB block-cache allocation |
| Cgroup kernel | 3,207,340,032 bytes | Includes the zswap charge below |
| Zswap compressed bytes | 3,103,397,118 bytes / 2.89 GiB | Already included in kernel/current RAM |
| Kernel minus zswap | About 99 MiB | Approximate residual; counters are sequential |

The zswap entry represents 8,390,770,688 logical bytes (`zswapped`). Neither that
logical size nor compressed zswap should be added again to cgroup RAM. The
process's historical RSS high-water mark is 10,610,856 KiB; the service's historical
peak is a different, broader quantity.

**Conclusion:** the footprint is not explained by file cache alone. A substantial
amount is anonymous memory or its swapped representation. These counters cannot
tell whether that anonymous memory belongs to live graph/query structures,
RocksDB caches, pinned storage blocks or allocator-retained freed memory.

## Client, shared backend and server are different measurements

Fresh CLI processes used the installed facade binary, with transient user scopes
limited to `MemoryHigh=512M`, `MemoryMax=1G`, `MemorySwapMax=0`, `CPUQuota=200%`.
These are experiment bounds, not accepted production policy.

| Read on Canix | Wall time | Peak client RSS | Result |
| --- | ---: | ---: | --- |
| Search `RuntimeManifest`, limit 5 | 1.39 s | 13,176 KiB | Five results, exit 0 |
| Context, depth 2, 10 nodes, 6,000 characters | 17.69 s | 94,628 KiB | Export-cap failure, exit 1, empty stdout |

The build remains `build:8ac2e28136c3`, generation 3, snapshot
`snap:cba2b1e91c07`, with the S01 coverage denominator. The context error is still:

```text
query navigation failed: consumer: export truncated at 10000 nodes; narrow the repo
```

The observed persistent Chaosbox child of the OpenCode service was idle at its
sample: 2,412 KiB RSS, 20 KiB PSS and 4,848 KiB swap. That is not a peak during
these fresh CLI queries, nor proof that the child is this harness's effective MCP
host. Shared mappings also make RSS/PSS comparisons important.

The OpenCode main process sample had 395,272 KiB RSS, 392,330 KiB PSS and
328,740 KiB swap. Its lifetime I/O was 126,733,504,512 read bytes and
135,800,463,360 write bytes. These are **shared-backend lifetime counters**, not
this conversation's I/O. The service's 13,899,816,960-byte historical RAM peak
includes its process tree; it is not the main process's RSS high-water mark.
User-cgroup `io.stat` was unavailable, so tree-wide I/O remains unknown.

### Running-runtime correction to S01

`/proc/3464594/exe` resolves to:

```text
/nix/store/lwlhs766ad8pi7hq82xkg521zcn8z5d5-opencode-v2-candidate-2.0.22+047c74e/bin/.opencode-wrapped
```

This is **different from the `opencode` PATH resolution recorded in S01**
(`2.0.12+f18083c78e`). Both observations are valid for their named surfaces.
S01b hashes the actual running executable and the actual Chaosbox child ELF,
separately from the facade wrapper. Future native/session-quality receipts must
identify the running backend, configuration layers and adapter, not only the
launcher found on PATH. Native custody compatibility is still unverified.

## I/O windows and competing activity

The snapshot service completed successfully at 13:55:11 UTC, before these windows.
Its last process-tree peak was 174,608,384 bytes. The decisions unit was failed,
with a last peak of 226,336,768 bytes. Both were inactive at the read-window
endpoints; their unchanged execution timestamps are retained.

| TypeDB cgroup counter | Before bounded reads, about 243 s | Bounded-read window, about 64 s |
| --- | ---: | ---: |
| CPU time increase | About 13.47 s | 20.95 s |
| Read-byte increase | 7,852,032 | 205,266,944 (195.76 MiB) |
| Write-byte increase | 4,415,488 | 94,208 |

The second window contains the search and failing context requests. Its RAM
charge rose 396,693,504 bytes while its swap charge fell 102,637,568 bytes.
Those deltas are **not exclusive per-query allocation or I/O costs**: the server
was shared, background work was not suspended and the counters were sampled
sequentially. There was no forced cache drop or controlled cold/warm reset.

The native ten-second `pidstat` sample reported about 6% of one CPU and 1.6 major
faults/second for TypeDB. Its process I/O columns were `-1`, meaning unavailable,
not negative I/O. `vmstat`'s first row is a since-boot average; subsequent rows
showed some swap-in but no swap-out in that short sample. Host `iostat`, cgroup
pressure and broader cgroup samples are retained for attribution context.

A later host sample had 36,881,453,056 available RAM bytes but
41,137,139,712 of 41,184,194,560 swap bytes occupied (**99.89%**). That is existing
swap occupancy, not evidence of continuous swap-out or exclusive TypeDB pressure.
This shared-host state does not support an isolated large-corpus claim.

## Source-backed mechanisms, not measured blame

The source inspection uses immutable Chaosbox `9d6c81d202aa5e9b4e0a513161891e8a4ff01e96`
and TypeDB 3.13.0 commit `7e3baa369a5afd3c42c3e21a4672785dbdbb09ec`, not the
dirty canonical sources. Copies are included in the checksum manifest.

| Mechanism | Exact source | What it establishes |
| --- | --- | --- |
| Storage caches | TypeDB `storage/keyspace/rocks_resources.rs:20–25,45–53` | The configured cache/write-buffer resources are shared across databases. The cache limit is soft and pinned index/filter blocks can exceed it. |
| Query caches | TypeDB `query/query_cache.rs:38–66,73–85`; `resource/constants.rs:121–123` | Parse/translation/plan capacities are 200/200/100 entries. The constructors have no byte weigher; source strings are retained as keys. This is not a measured byte attribution. |
| Disabled MVCC cleanup | TypeDB `database/database.rs:658–663,704–706` | The disabled worker clears its cleanup queue. Disabled key cleanup alone does not prove that this queue grows without bound in RAM. |
| Isolation timeline | TypeDB `storage/isolation_manager.rs:391–408` | Windows have freeing/eviction logic. Actual pinned-window allocation was not observed. |
| Per-row writes | Chaosbox `crates/chaosbox-typedb/src/store.rs:119–135`; `store/flush.rs:82–121,159–185` | Individual inserted rows commit separately; edge membership carries sealed evidence and is read back. This is a concrete source of transaction/work amplification, not a measured physical-write ratio. |
| Retained graph builds | Chaosbox `store/chain.rs:137–160` | Publication changes the current pointer and new build status; it does not delete predecessor builds in this path. `status="active"` is not equivalent to being the current pointer target. |
| Knowledge representation | Chaosbox `store/memory.rs:108–129` | A whole-generation envelope and typed item/evidence representations are staged. The amount of this data in the live database was not measured. |

The 1 GiB RocksDB cache setting therefore cannot serve as a whole-process budget.
Conversely, these source mechanisms do not prove that any one of them accounts
for the observed 26 GiB anonymous-plus-swap footprint. Retained generations and
storage-engine MVCC versions are distinct forms of history.

## Unresolved attribution and exact blockers

1. **Heap/storage-cache ownership:** `/proc/3067/smaps_rollup`, process I/O and
   allocator/RocksDB metrics were unavailable. A privileged read was rejected
   by the tool authorization layer before execution. Owner: the `typedb` service
   identity (UID 64505), with privileged inspection governed by operator policy.
   Completion requires authorized read-only allocation/process evidence.
2. **Retained-history costs:** `/var/lib/typedb/data` was inaccessible in S01,
   and no physical-size inventory was obtained. The installed Chaosbox CLI has
   readiness and graph queries but no bounded retained-build inventory command.
   The official console's safe password prompt requires a tty; redirected
   credential-file input failed before database inventory. Completion requires
   a secure native read-only inventory and storage/allocator evidence sufficient
   to distinguish retained builds, membership/evidence bytes and engine overhead.
   Generation 3 is not a count of three retained builds.
The resource-policy decision is recorded below. Numeric qualification criteria
will be agreed after the repaired path is measured, rather than blocking S02.

No global checkout freeze or production writer freeze is needed to continue
source work. S02 can proceed with bounded read-path repair and focused tests.
Major storage/schema implementation remains behind the unresolved S01b gate.

## Resource-policy decision

The operator selected **Revise the targets**, then **Defer numeric limits**.
RAM, latency and swap limits remain unset until successful repaired bounded reads
have been measured. The JSON receipt retains the earlier numbers under
`previousProposal` with `accepted: false`; they are not qualification gates.

The working requirements remain complete declared indexing scope, bounded query
responses, exact recoverable evidence and read-only graph behavior. Measure
fresh/warm latency, client peaks, server anonymous/cache/zswap, sustained swap
activity and read amplification on the same pinned scope. Report variability,
background activity and pre-existing swap separately before agreeing limits.

An absolute TypeDB/whole-stack RAM cap and an automatic-context token budget also
remain unset. Existing evidence cannot justify the earlier suggested 4/6 GiB
whole-stack limits. A RAM envelope needs warm-working-set, retained-history and
large-corpus evidence; context limits need S24's task-quality results.

## Evidence and reproduction

Artifacts are at `/data/scratch/tmp/opencode/chaosbox-s01b-20261016/`.
`SHA256SUMS` is bound by the committed receipt. It also covers the access-limit
record and failed/partial captures, so an empty failed file cannot be mistaken
for zero usage. Keep the bundle or relocate it with checksum verification.

These examples explicitly use **Bash** and existing native tools:

```bash
cd /data/scratch/tmp/opencode/chaosbox-s01b-20261016
sha256sum SHA256SUMS
# Expected: 1d605160b2aa88d7a40b01052eae0b9ae62c34780a596b2cf1741a47a6b3a9b8
sha256sum --check SHA256SUMS
sha256sum --check runtime-SHA256SUMS
```

For a fresh observation, resolve the current PID/cgroup with `systemctl show`
first; historical PIDs below are evidence identities, not future selectors.
Use a new evidence directory for each run. Keep sample times, `memory.stat`,
`memory.current`, `memory.swap.current`, `io.stat`, `cpu.stat`, pressure and unit
execution timestamps together. Inspect the actual `/proc/PID/exe` for the shared
OpenCode backend as well as PATH resolution.

```bash
systemctl show typedb.service --property=MainPID,ControlGroup,MemoryCurrent,MemoryPeak,MemorySwapCurrent,IOReadBytes,IOWriteBytes,CPUUsageNSec
systemctl --user show opencode.service --property=MainPID,ControlGroup,MemoryCurrent,MemoryPeak,MemorySwapCurrent
pidstat -r -d -u -p 3067 1 10
vmstat -w -t 1 10
iostat -d -x -t -y 1 10
systemctl --user show canix-chaosbox-snapshot.service canix-chaosbox-decisions.service --property=Id,ActiveState,MainPID,ExecMainStartTimestamp,ExecMainExitTimestamp,MemoryPeak
chaosbox-canix query search RuntimeManifest --repo canix --limit 5
chaosbox-canix query context RuntimeManifest --repo canix --depth 2 --max-nodes 10 --max-chars 6000
```

The S01 reproduction commands show how to retain GNU time metrics inside bounded
transient scopes. Do not interpret a repeated live read as an isolated benchmark
without recording competing activity. This session changed documentation and
local evidence only; no production configuration, database or service lifecycle
was changed.
