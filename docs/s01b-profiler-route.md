# S01b — disposable allocation profiling route

**The existing Valgrind/Massif tools can resolve allocation ownership in the
installed TypeDB executable.** Two required native graph-history and knowledge
cases passed under a bounded disposable server. No new profiler dependency or
runtime implementation was needed. Parent S01b remains open for production
allocation/retention and history attribution.

See the [receipt](s01b-profiler-route.json), [operator measurements](s01b-operator-followup.md)
and [original attribution report](s01b-resource-attribution.md).

## Qualified route

The installed TypeDB 3.13.0 executable retains function symbols. Valgrind 3.26.0
already exists at
`/nix/store/9mw8a1jy9pasfsix1az35byjxs1lspr2-valgrind-3.26.0`.
Massif records live requested allocations with resolved Rust and RocksDB call
stacks. Its existing `vgdb` monitor commands can take phase-specific
`detailed_snapshot` and `xtmemory` dumps. Native `ms_print` and
`callgrind_annotate` rendered the retained profiles successfully.

The disposable process used the same TypeDB executable SHA-256 as the operator
follow-up, `f2eda580af8a386f25d68d82bcd2984fa3b03b5f330b12a5c12a6bc88d2db3b8`.
Its only listener was gRPC at `127.0.0.1:24789`; HTTP, admin, monitoring and
external diagnostics reporting were disabled. Data/logs and the profiler's
control files belong to the new private evidence directory. No production data
or credentials were used.

The user service enforced two CPU cores of quota, 2 GiB memory-high, 4 GiB
memory-max, zero swap, a 1,200-second runtime deadline and a 256 MiB per-file
limit. Cargo used two jobs; the native workers had the same CPU/memory/swap
bounds. Resource controls are experiment bounds, not production policy.

The two test executables came from the recorded dirty Chaosbox source archive.
Their local Cargo artifacts were reused; the native test runs are fresh. The
inventory CLI was copied byte-for-byte from the previously qualified S01b.1
executable, with its earlier source archive and receipt identity retained.

## Native execution and phase measurements

Both cases ran with `CHAOSBOX_REQUIRE_TYPEDB=1` against the owned server:

- `disposable_inventory_enumerates_history_without_confusing_status_and_pointer`:
  **one passed, zero ignored**, 9.09 seconds under profiling. This checks two
  retained graph generations, exact node/edge/sealed-attribute counts, exclusive
  pagination, current-pointer semantics and foreign/escaped selectors.
- `private_session_knowledge_is_pinned_idempotent_and_predecessor_guarded`:
  **one passed, zero ignored**, 2.59 seconds. This checks generation readback,
  scope isolation, predecessor checks and the competing-publication guard.

| Forced phase snapshot | Live requested heap bytes | Modeled allocator-extra bytes |
| --- | ---: | ---: |
| Startup after readiness | 115,281,500 | 12,062,876 |
| After graph-history case | 127,393,121 | 12,234,751 |
| After knowledge case | 133,026,757 | 12,358,203 |

The final profile retained 32 snapshots. Its peak requested-heap snapshot was
133,171,995 bytes, with 12,358,349 modeled extra bytes. The profiled process's
cgroup peaked at 463,716,352 bytes (442.23 MiB), with zero swap. That broader
charge includes the instrumentation and other process memory; subtracting the
Massif counters would not identify native allocator retention.

The fixture's storage files and allocated-byte directory inventories were also
captured before/after each case and after shutdown. They describe the disposable
databases as a whole, not per-build physical ownership. The history command's
`physical_bytes` remains null.

## Concrete finding: all-host process sampling at startup

One **85,934,394-byte (81.95 MiB) allocation subtree** remained unchanged in all
three forced snapshots. Its resolved stack is:

```text
alloc::raw_vec::RawVecInner::try_allocate_in
  sysinfo::unix::linux::process::copy_from_file
    sysinfo::unix::linux::process::update_proc_info
      sysinfo::unix::linux::process::retrieve_all_new_process_info
```

The byte count is one observed subtree, not the entire cost of `sysinfo` or a
production estimate. Other recorded branches include RocksDB arena/memtable
allocation and TypeDB ID allocation. The profile distinguishes these owners
without inferring them from RSS alone.

Immutable TypeDB source `7e3baa369a5afd3c42c3e21a4672785dbdbb09ec`,
`diagnostics/metrics/system_sampler.rs:48–52`, constructs a `System` with
`ProcessRefreshKind::everything()` for all processes. It then immediately calls
`sampler.refresh()` at line 68, which already refreshes host memory and only
`ProcessesToUpdate::Some(&[self.pid])` at lines 75–80. The sampler retains that
`System` in its mutex-protected state. This startup work happens even in the
tested configuration with reporting and monitoring disabled.

The source inspection is pinned separately from the executable; its exact
source-to-binary derivation binding remains unverified.

**Next producer workcard:** remove the redundant all-process initialization and
reuse the existing own-PID refresh. A small regression should verify that the
sampler retains only its own process before and after refresh while preserving
its reported metrics. That repair has not been implemented or qualified here.
The workspace-selected TypeDB fork is still on `d86c6763d7dcffdff754ca65b60a1ffb068e0d2d`
and lacks this newer source file; establish the appropriate current producer
checkout before editing it.

## Measurement boundary and remaining S01b gate

Massif's default heap mode traces requested `malloc`/`new` allocations. It
replaces allocation behavior; it does not measure glibc's native retained arenas,
all direct mappings, or the unprofiled production working set. Stack profiling
was disabled; `mem_stacks_B = 0` means excluded. Backtraces have depth 40 and a
0.1% aggregation threshold, with aggregate bytes retained below that threshold.
See the retained [official manual](https://valgrind.org/docs/manual/ms-manual.html).

Valgrind reported unimplemented `fcntl` command 1036. The raw messages are
retained. Both named functional cases passed, but this run establishes neither
complete syscall compatibility nor a production performance comparison.

The route requires starting an owned profiled process; it cannot retrospectively
recover allocation stacks for the existing production heap. Representative
production allocation stacks, native allocator retention, cache/pinned-block
ownership and retained payload versus engine-history costs remain unresolved.
The 26.60 GiB production footprint is not attributed to the startup finding.
Numeric resource-policy decisions remain deferred, and S05's applicable storage
gates remain open.

## Custody and cleanup

Artifacts are in `/data/scratch/tmp/opencode/chaosbox-s01b-profiler-20261019/`:
frozen dirty-source archive/checksums, retained clients/test executables, profiler
configuration, native logs, phase/complete profiles, rendered allocation trees,
storage inventories and resource counters. The command record provides exact
reproduction commands. The adjacent receipt binds `evidence.sha256`.

Seventeen native consistency assertions pass, including raw profile/receipt
equality, allocation-tree byte reconciliation, exact native test results and
the source-blob identity. Scoped treefmt exited zero but emitted no files for
processing; formatter coverage is not claimed. JSON parsing and staged whitespace
checks pass.

The owned server stopped successfully with `MainPID=0`, exit status 0 and no
listeners on its three reserved ports. Its data remain retained for inspection.
Production PID/start identity and the pre-existing source/index state were
checked independently. Exact cumulative context telemetry was unavailable.
