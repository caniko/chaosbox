# S01b — operator process and storage evidence

The operator-supplied measurements establish **8.85 GiB resident anonymous
memory, 17.75 GiB process swap and 28.20 GiB of Chaosbox database disk use**.
WAL accounts for 19.70 GiB, or 69.85% of that database's allocated disk tally.
**S01b remains partial:** allocation ownership and physical costs of individual
retained builds are still unresolved.

This follow-up supplements the original [S01b report](s01b-resource-attribution.md)
and completed [logical inventory](s01b1-retained-build-inventory.md). The
[receipt](s01b-operator-followup.json) binds the evidence and verification.

## Measurement provenance

The operator supplied output from these commands in this conversation on
2026-10-19; the agent received it at 06:21:17 UTC. The capture time is unknown.
Numeric values and paths are transcribed in the evidence bundle; separator
whitespace may be normalized.

```text
sudo -n cat /proc/4144752/smaps_rollup
sudo -n cat /proc/4144752/io
sudo -n du -x -B1 --max-depth=2 /var/lib/typedb/data
```

The agent's service observation identifies PID `4144752`, monotonic start
`158470931838`, the same identity as S01b.1. It is a separate observation, not a
synchronized sample of the supplied process counters. Full per-mapping `smaps`
was not supplied. The earlier tool-permission rejection remains a historical
fact; the supplied outputs now fill the process-total and filesystem-total gaps.

The running package path names TypeDB 3.13.0. Its executable SHA-256 is
`f2eda580af8a386f25d68d82bcd2984fa3b03b5f330b12a5c12a6bc88d2db3b8`.
`nix-store --query --deriver` returned `unknown-deriver`, so the inspected source
revision is not independently bound to that executable by a derivation receipt.

## Reconciled totals

Linux procfs `kB` fields use 1,024-byte units. GiB below uses 2^30 bytes.

| Process component | Bytes | Interpretation |
| --- | ---: | --- |
| RSS | 9,506,381,824 | 8.85 GiB resident mapped memory |
| Resident anonymous | 9,500,651,520 | 8.85 GiB; 99.94% of RSS |
| Process swap | 19,063,894,016 | 17.75 GiB of swapped mappings |
| Anonymous resident plus swap | 28,564,545,536 | 26.60 GiB footprint proxy, not required RAM |
| Remaining resident memory | 5,730,304 | 5.46 MiB; does not measure unmapped page cache |

Anonymous memory can contain live objects, caches, stacks and allocator-retained
pages. The rollup does not separate them. Anonymous huge pages are a subset,
not another charge. Process swap can include zswap-backed pages; adding it to
cgroup RAM would double-count their compressed representation. Neither this
snapshot nor its `Referenced` field establishes a warm working set or a leak.

| Filesystem component | Allocated-byte tally | Share or converted size |
| --- | ---: | --- |
| Entire TypeDB data root | 30,375,899,136 | 28.29 GiB |
| Chaosbox database | 30,278,262,784 | 28.20 GiB |
| Chaosbox WAL | 21,148,098,560 | 19.70 GiB; 69.85% of Chaosbox |
| Chaosbox storage | 9,121,206,272 | 8.49 GiB |
| Chaosbox checkpoint | 8,957,952 | 8.54 MiB charged in this traversal |
| All 47 `t_*` directories | 95,178,752 | 90.77 MiB; 0.31% of the root |

All 196 supplied directory rows parse uniquely. The three Chaosbox components
sum to its database total. The 49 top-level database-directory totals leave
4,096 bytes for the root directory itself. Test-named directories contribute
little to the observed disk tally; their memory costs were not measured.

`du -B1` reports allocated filesystem accounting, not apparent payload size or
unique physical extents. Its default hard-link deduplication can make the split
between checkpoint and storage traversal-order-dependent. The checkpoint tally
is not a standalone restore size or proof that it contains only 8.54 MiB of
referenced data. No live inode/extent inventory was supplied.

The process I/O counters report 86,344,593,408 read bytes (80.41 GiB) and
679,761,944,576 write bytes (633.08 GiB). `rchar` is 7,143,475,126,760 bytes
(6.50 TiB). These are lifetime counters with different semantics, not current
rates, one-request costs or a measured write-amplification ratio.

## Source trace and measurement surface

All 16 inspected files match Git blobs in immutable TypeDB revision
`7e3baa369a5afd3c42c3e21a4672785dbdbb09ec`. Local copies, upstream paths and blob
identities are retained in `source-blob-verification.tsv`.

| Source at that revision | Finding |
| --- | --- |
| `storage/keyspace/keyspace.rs:230–236`; `durability/wal.rs:39,302–330` | Keyspace writes disable RocksDB WAL and use TypeDB's separate durability log. Its 16 MiB threshold rotates files; it is not a total WAL-size cap. |
| `durability/durability.rs:43–74`; `durability/wal.rs:354–388` | `truncate_from` deletes records at or after a sequence and newer files. It is suffix truncation, not old-prefix reclamation. Delete/reset remove the log rather than implement retention. |
| `database/database.rs:616–627`; `storage/storage.rs:432–434`; `storage/recovery/checkpoint.rs:362–391` | The inspected checkpoint path writes storage metadata/checkpoints and removes preceding checkpoint directories. It does not prune old WAL segments. |
| `storage/keyspace/rocks_resources.rs:20–25,45–52`; `storage/isolation_manager.rs:391–409` | Cache/write-buffer resources are shared across databases, the cache limit is soft, and isolation windows have freeing/eviction logic. Neither configured limits nor disabled MVCC cleanup identify the measured heap owner. |
| `diagnostics/metrics/mod.rs:155–181`; `diagnostics/reports/prometheus_monitoring.rs:50–77`; `server/service/admin/proto/admin_service.proto:8–12` | The inspected monitoring surface reports host/process totals, and the admin RPCs provide version/status/password reset. These interfaces supply no allocator/live-object/cache-byte breakdown. |

This trace identifies engine-log retention separately from retained graph
generations. It does not measure which live WAL records are obsolete or how many
bytes are safely reclaimable. The active configuration has monitoring and the
admin socket disabled; simply enabling these inspected interfaces would still
not supply the missing allocation breakdown.

## Gate status and next evidence

| Gate | Status | Evidence or missing result |
| --- | --- | --- |
| S01b.1 native logical inventory | Done | Prior complete page independently found three Canix builds; `physical_bytes` is null. |
| S05a immutable source/oracle preparation | Done | Its existing signed qualification remains applicable to that workcard. |
| S01b process/filesystem totals | Obtained | Supplied rollup, I/O and directory totals reconcile. |
| S01b allocator/history attribution | Blocked | No measured ownership split, per-build payload costs or engine-history decomposition. |

The remaining producer is the TypeDB allocation/storage measurement surface.
Closure requires a receipt separating live allocations from allocator retention,
cache/pinned-block costs and retained graph/knowledge payloads versus engine
history. The inspected production interfaces cannot produce that receipt.
Per-mapping `smaps` could refine mapping attribution but would not provide object
ownership. A suitable supported measurement route must be established before
another live collection can close this gate.

The operator's deferred numeric-policy decision still applies. Substantial
S05 storage/schema work and representation-dependent S03b.2/S04b remain behind
S01b. This evidence establishes no production memory budget.

## Verification and retained artifacts

The bundle is `/data/scratch/tmp/opencode/chaosbox-s01b3-20261019/`.
It contains the transcribed inputs, parsed/derived JSON, source copies, exact
Git-blob verification, runtime identity/configuration and preservation records.
Seventeen native jq consistency assertions verify raw parsing, arithmetic,
receipt equality, prior logical-inventory scope and the unresolved gate. The
command record retains the complete invocation. To verify custody, in Bash:

```bash
cd /data/scratch/tmp/opencode/chaosbox-s01b3-20261019
sha256sum --check evidence.sha256
```

The manifest digest is recorded in the adjacent receipt. Checksums establish
custody of supplied evidence, not an independently timed privileged capture.
