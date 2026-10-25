# Local RAM fixes: client composition and TypeDB sampler

Status: implemented locally, 2026-10-19. Client gates pass; the sampler's unit
and memory checks pass, but full TypeDB producer qualification remains blocked.
Nothing was pushed, promoted or activated. [Receipt](ram-local-qualification.json).

## Three independent tracks

| Track | Local result | Remaining boundary |
| --- | --- | --- |
| Client/indexing | `efd45454ceb418bb09f5f1e80de03dccf6926d8e`; immutable composition, complete-export parity, memory and required native cases pass | Exact producer/package and Canix consumer qualification; serving transition |
| Sampler | TypeDB `674b573c82ceafded9b629a7d1177005c081f58e`; own-PID repair, 12 diagnostics tests and paired native/allocation checks pass | Strict Clippy fails on 23 unchanged errors outside the sampler; package qualification and installed-source binding |
| Remaining server attribution | Not executed | Native allocator/cache semantics, complete-history restoration, replica admission and agreed residual/storage-cost acceptance |

These tracks do not redefine or close [S01b](s01b-operator-followup.md), lower
its dependent S05/S03b.2/S04b gates, authorize a knowledge cache or set a production
RAM budget. SQLite custody, graph/history identities, evidence and authorization
contracts are unchanged.

## Client: exact source, not a dirty-checkout combination

Applied only memory commit `1cb8def7f1af0bcbd65f1411edc71ab1b05ac18a` to
`a6883f2a898527bea56cd86ba7919e9e4fea02a2`. A private index produced immutable
tree `863ec3a5e71481c438a5c8b5acf75a8adbfc1cfd`; its archive supplied every
accepted client build/test. The final normal signed commit has exactly that
tree and patch. A full branch merge preview conflicted in unrelated files and
was not adopted. Existing dirty source and staging were preserved.

The integration reuses borrowed candidate/staging rows, streamed publication
hashing and export, and removes redundant graph copies. It includes the existing
validation-before-write, canonical-digest and export-parity regressions. It
preserves the later bounded-reader, FIFO admission, driver reuse, header batching
and exact historical follow-up implementations.

Affected libraries and CLI/MCP suites, scoped warnings-denied Clippy and treefmt
pass. Ten required disposable-TypeDB cases each ran exactly once with zero
ignored: historical hydration/recursive expansion, admission, cleanup/retention,
connection reuse, header batching, paths, historical pages, counts, reference
conformance and unpublished-build rejection. Optional unselected live/model
tests are not claimed as qualified by these focused suites.

The existing isolated publication-digest check grew peak RSS by **2,088 KiB**,
below its unchanged fixture tolerance. The explicit pipeline-memory test also
passed complete-export equality and lower peak RSS. Neither tolerance is a
production memory budget.

## Sampler: repair initialization once

The selected older producer checkout lacked `system_sampler.rs`. The existing
clean linked worktree was adopted into the canonical TypeDB checkout through
Canix, preserving its old branch, then a new branch started from independently
pinned source `7e3baa369a5afd3c42c3e21a4672785dbdbb09ec`.

`System::new_with_specifics(...with_processes(...))` initially loaded all host
processes. Subsequent own-PID refreshes did not remove those unrelated entries.
The regression failed before repair with **5,851 entries instead of one**.
`System::new()` now starts empty; the existing constructor's `refresh()` loads
host memory and only the server PID. No refresh, telemetry or locking engine was
replaced. The process-map and metrics regression passes; all **12 diagnostics
tests**, including the explicit memory case, pass with zero ignored.

Strict diagnostics Clippy was not relaxed. Baseline and candidate both exit 101
and normalize to the identical **23 errors**, with matching locations/codes/text,
all outside `diagnostics/metrics/system_sampler.rs`. This remains a failed
producer gate, not a passing check or a reason to absorb unrelated cleanup.

## Comparative measurements

`protocol.json` was recorded before the paired comparisons. Each track ran three
pairs in alternating order (baseline/candidate, candidate/baseline,
baseline/candidate). These are one-shot workloads: no warm-up, cache flush or
idle/steady-state window was claimed. GNU time records peak RSS, elapsed/user/
system time, faults and filesystem I/O separately.

| Workload | Baseline median peak RSS | Candidate median peak RSS | Reduction |
| --- | ---: | ---: | ---: |
| Frozen Canix offline indexing, release CLI | 754,348 KiB | 342,544 KiB | 54.6% |
| Isolated sampler initialization plus three refreshes | 103,768 KiB | 4,332 KiB | 95.8% |

All six release exports are byte-identical: **65,872 nodes / 45,946 edges**,
SHA-256 `78b2a22ffe28ad445578063604479dfcde8016ef278065c35d76504f41fd60b5`.
Indexing uses the frozen S01 corpus, memory backend, a 40-candidate inference cap
and no model decisions; the cap does not reduce the complete structural graph.
The installed baseline and new client use different build toolchains, and host
filesystem caches are uncontrolled. Timing/I/O observations therefore do not
isolate the RAM patch's latency or disk effect. They establish no server scan or
million-fact acceptance result.

The sampler retained 5,704/5,705/5,705 processes in native baseline runs and one
in every candidate run. A separate paired Massif check passed the same measurement
case: peak requested heap was **90,380,201 → 331,756 bytes**; modeled extra bytes
were **10,472,623 → 22,188**. These instrumented unit-process allocation totals
are not native allocator retention, total sysinfo cost or production TypeDB GiB
savings. They cannot be subtracted from another process/run's RSS or swap.

## Remaining server work: fail closed at prerequisites

Before any representative server replay, separately establish:

1. Exact executable-to-producer/source provenance. The installed executable's
   unknown deriver is not resolved by this independent source repair.
2. A native measurement mechanism with demonstrated live-object, allocator and
   cache semantics; Massif and aggregate RSS/swap alone are insufficient.
3. Authorized, complete-history capture/restoration covering retained graph and
   knowledge generations, with explicit database versus whole-process scope.
4. Separate resource admission for a full replica. The 4 GiB/no-swap worker and
   disposable-server ceilings here are not replica admission or TypeDB budgets.
5. A predeclared representative comparison: workload, repetitions, warm-up, idle
   interval, duration and method, distinguishing legitimate history growth from
   unexplained retention and assessing RAM, swap, latency and I/O separately.
   Residual attribution and marginal per-build storage acceptance must be agreed
   before claiming S01b closure.

A failed prerequisite blocks that track; it does not authorize broader profiling,
history pruning, service lifecycle changes or gate revision.

## Evidence and unchanged production

Retained commands, protocol, immutable sources, binaries, tests, profiles and
measurements are in `/data/scratch/tmp/opencode/chaosbox-ram-final-20261019/`.
`SHA256SUMS` verifies **173 artifacts**; its hash is
`e8f8f330614c9176597093658c89d48939177f5441a00dd8f0dde4dfd6ce44f0`.
Fixture credentials, mutable indexes and extracted/archive-duplicate directories
are excluded. Retain or relocate the evidence with checksum verification.

The owned loopback fixture server stopped successfully with no remaining
listeners; it peaked at 398,290,944 cgroup bytes with zero swap. Production TypeDB
remained PID 3079 with monotonic start 92940067 throughout the before/after check.
No production restart/activation, full replica, publication, consumer pin update
or serving-process promotion occurred. The earlier evidence-only headroom handoff
was not changed or resumed. Concurrent Canix commits were retained; no parent
checkout edits were made by this work.
