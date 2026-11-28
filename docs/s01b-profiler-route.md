# S01b — disposable allocation profiling route

**Existing Valgrind/Massif tools resolve allocation ownership in the installed
TypeDB executable.** Two required native graph-history and knowledge cases
passed against a bounded disposable server with no new profiler dependency or
runtime implementation. Parent S01b stays open for production
allocation/retention and history attribution.

See the [receipt](s01b-profiler-route.json), [operator measurements](s01b-operator-followup.md)
and [original attribution report](s01b-resource-attribution.md).

## Qualified route

The installed TypeDB 3.13.0 executable retains function symbols. Valgrind 3.26.0
is installed at
`/nix/store/9mw8a1jy9pasfsix1az35byjxs1lspr2-valgrind-3.26.0`.
Massif records live requested allocations with resolved Rust and RocksDB call
stacks, and its `vgdb` monitor commands capture phase-specific
`detailed_snapshot` and `xtmemory` dumps. Native `ms_print` and
`callgrind_annotate` rendered the retained profiles.

The disposable process used the same TypeDB executable SHA-256 as the operator
follow-up (`f2eda580af8a386f25d68d82bcd2984fa3b03b5f330b12a5c12a6bc88d2db3b8`),
listening only on gRPC at `127.0.0.1:24789` with HTTP, admin, monitoring and
external diagnostics reporting disabled. Data/logs and profiler control files
live in the new private evidence directory; no production data or credentials
were used.

The user service enforced two CPU cores of quota, 2 GiB memory-high, 4 GiB
memory-max, zero swap, a 1,200-second runtime deadline and a 256 MiB per-file
limit. Cargo used two jobs; native workers had the same CPU/memory/swap bounds.
These controls are experiment bounds, not production policy.

Both test executables came from the recorded dirty Chaosbox source archive,
reusing local Cargo artifacts with fresh native test runs. The inventory CLI was
copied byte-for-byte from the previously qualified S01b.1 executable, keeping
its earlier source archive and receipt identity.

## Native execution and phase measurements

Both cases ran with `CHAOSBOX_REQUIRE_TYPEDB=1` against the owned server:

- `disposable_inventory_enumerates_history_without_confusing_status_and_pointer`:
  **one passed, zero ignored**, 9.09 seconds under profiling. Checks two
  retained graph generations, exact node/edge/sealed-attribute counts, exclusive
  pagination, current-pointer semantics and foreign/escaped selectors.
- `private_session_knowledge_is_pinned_idempotent_and_predecessor_guarded`:
  **one passed, zero ignored**, 2.59 seconds. Checks generation readback,
  scope isolation, predecessor checks and the competing-publication guard.

| Forced phase snapshot | Live requested heap bytes | Modeled allocator-extra bytes |
| --- | ---: | ---: |
| Startup after readiness | 115,281,500 | 12,062,876 |
| After graph-history case | 127,393,121 | 12,234,751 |
| After knowledge case | 133,026,757 | 12,358,203 |

The final profile kept 32 snapshots, peaking at 133,171,995 bytes of requested
heap with 12,358,349 modeled extra bytes. The profiled cgroup peaked at
463,716,352 bytes (442.23 MiB) with zero swap; that broader charge includes
instrumentation and other process memory, so subtracting Massif counters does
not identify native allocator retention.

Fixture storage files and allocated-byte directory inventories were captured
before/after each case and after shutdown. They measure the disposable databases
as a whole, not per-build physical ownership; the history command's
`physical_bytes` remains null.

## Concrete finding: all-host process sampling at startup

One **85,934,394-byte (81.95 MiB) allocation subtree** stayed unchanged across
all three forced snapshots:

```text
alloc::raw_vec::RawVecInner::try_allocate_in
  sysinfo::unix::linux::process::copy_from_file
    sysinfo::unix::linux::process::update_proc_info
      sysinfo::unix::linux::process::retrieve_all_new_process_info
```

This byte count is one observed subtree, not total `sysinfo` cost or a
production estimate. Other recorded branches include RocksDB arena/memtable
allocation and TypeDB ID allocation. The profile separates these owners without
inferring them from RSS alone.

Immutable TypeDB source `7e3baa369a5afd3c42c3e21a4672785dbdbb09ec`
(`diagnostics/metrics/system_sampler.rs:48–52`) builds a `System` with
`ProcessRefreshKind::everything()` for all processes, then immediately calls
`sampler.refresh()` at line 68, which already refreshes host memory and only
`ProcessesToUpdate::Some(&[self.pid])` at lines 75–80. The sampler retains that
`System` in its mutex-protected state; the all-process startup initialization
ran even with reporting and monitoring disabled in the tested configuration.

This source inspection is pinned separately from the executable; exact
source-to-binary derivation binding remains unverified.

**Next producer workcard:** remove the redundant all-process initialization and
reuse the existing own-PID refresh, with a small regression verifying the
sampler retains only its own process before and after refresh while preserving
reported metrics. That repair is not implemented or qualified here. Because the
workspace-selected TypeDB fork is still on `d86c6763d7dcffdff754ca65b60a1ffb068e0d2d`
and lacks this file, establish the current producer checkout before editing.

## Measurement boundary and remaining S01b gate

Massif's default heap mode traces requested `malloc`/`new` allocations and
replaces allocation behavior; it does not measure glibc's native retained
arenas, all direct mappings, or the unprofiled production working set. Stack
profiling was disabled (`mem_stacks_B = 0` means excluded). Backtraces use depth
40 and a 0.1% aggregation threshold, retaining aggregate bytes below that
threshold; see the retained [official manual](https://valgrind.org/docs/manual/ms-manual.html).

Valgrind logged unimplemented `fcntl` command 1036 (raw messages retained). Both
functional cases passed, but this run proves neither complete syscall
compatibility nor a production performance comparison.

This route requires starting an owned profiled process and cannot
retrospectively recover allocation stacks for the existing production heap.
Representative production allocation stacks, native allocator retention,
cache/pinned-block ownership and retained payload versus engine-history costs
remain unresolved. The 26.60 GiB production footprint is not attributed to this
startup finding; numeric resource-policy decisions remain deferred and S05's
applicable storage gates remain open.

## Custody and cleanup

Artifacts are in `/data/scratch/tmp/opencode/chaosbox-s01b-profiler-20261019/`:
frozen dirty-source archive/checksums, retained clients/test executables, profiler
configuration, native logs, phase/complete profiles, rendered allocation trees,
storage inventories and resource counters. The command record gives exact
reproduction commands, and the adjacent receipt binds `evidence.sha256`.

Seventeen native consistency assertions pass (raw profile/receipt equality,
allocation-tree byte reconciliation, native test results and source-blob
identity). Scoped treefmt exited zero with no files processed, so formatter
coverage is not claimed. JSON parsing and staged whitespace checks pass.

The owned server stopped cleanly (`MainPID=0`, exit status 0, no listeners on
its three reserved ports) with its data retained for inspection. Production
PID/start identity and pre-existing source/index state were checked
independently. Exact cumulative context telemetry was unavailable.
