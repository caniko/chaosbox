# Indexing memory verification

The allocation fixes preserve graph contents, candidate ordering/accounting,
publication identities and sealed historical evidence. They remove unused
indexes, redundant entity/build copies, full evidence/JSON copies during
publication hashing, and full JSON materialization during CLI export. Claims
are indexed by relationship instead of scanned for every edge. TypeDB flushes
borrow validated staging rows rather than cloning them again.

Structural validation still completes before any observations are persisted.
The batch retains source references and relationship ids, not another copy of
every relationship and evidence body. Duplicate facts fail before writes.

## Isolated hashing regression

Run in a separate process, with no other tests sharing its RSS high-water mark:

```sh
direnv exec . cargo test --locked -p chaosbox-store --lib -j 2 publication_digest_memory_budget -- --ignored --nocapture --test-threads=1
```

On Atlas, the same 12,000-relationship fixture with 2 KiB evidence bodies gave:

| Implementation | Additional peak RSS | Hashing time | Publication id |
| --- | ---: | ---: | --- |
| Original | 69,268 KiB | 5.69 s | `publication:7ef23cd91807` |
| Streaming/indexed | 2,088 KiB | 1.08 s | `publication:7ef23cd91807` |

These are debug-build, stage-local measurements, **not** whole-indexer or
TypeDB server memory claims. The Linux regression permits 16 MiB of growth.

## Offline pipeline comparison

The explicit benchmark generates 250 files with 50,000 declarations and runs
both CLIs on the identical corpus. It measures child peak RSS using GNU time
and compares SHA-256 hashes of the complete JSON exports. Both children use
the disposable memory backend, no inference and no production database.

```sh
CHAOSBOX_BASELINE_BIN=/absolute/path/to/original/chaosbox direnv exec . cargo test --locked -p chaosbox --test pipeline_memory -j 2 -- --ignored --nocapture --test-threads=1
```

Set `CHAOSBOX_MEMORY_CORPUS` to an existing checkout to benchmark that tree
read-only instead of generating the fixture. Omit `CHAOSBOX_BASELINE_BIN` to
measure only the current CLI.

Measured on Atlas on 2026-10-15:

| Corpus | Files | Facts | Original peak RSS | Optimized peak RSS | Reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| Generated declarations | 250 | 50,250 | 561,760 KiB | 296,656 KiB | 47.2% |
| Canix checkout | 2,132 | 49,216 | 947,540 KiB | 420,304 KiB | 55.6% |

Complete export SHA-256 values matched between the two implementations:

- Generated: `58941210345a089f6a57746a7648b22b8628c0c832023f975f95581bcbc4d535`
- Canix: `5e611a5fa676622d863a3b4a62f2094d8a43668605d51d1789286f95302c0e47`

The baseline was the installed release binary at
`/nix/store/aamrc152yl781cs8fy5nrk1nzgip19a7-chaosbox-0.1.0/bin/chaosbox`;
the optimized CLI was a debug build of the working tree. **Timings are not
comparable across those profiles.** The Canix run used a transient systemd
scope with `MemoryHigh=8G`, `MemoryMax=12G`, and `MemorySwapMax=0`; neither
child approached those limits.

## Qualification and remaining work

The workspace tests, warnings-denied workspace/all-targets Clippy, isolated
hashing regression, and disposable real-TypeDB integration gate pass. The
latter uses a separate loopback port and database directory, requires live
conformance tests rather than accepting skips, and verifies fresh-process
readback, publication guards, and sealed historical evidence. No production
service was restarted or reconfigured.

These measurements do **not** establish a RAM budget for the original
1,186,553-fact workload or resolve the TypeDB server's separate footprint.
A read-only observation of the live service on 2026-10-15 showed approximately
7.8 GiB cgroup RAM, 3.8 GiB swap, and a 10.3 GiB historical cgroup peak. Cgroup
RAM includes file cache and compressed swap, unlike process RSS.

The deployed TypeDB 3.13.0 configuration retains its default 1 GiB RocksDB
cache and 512 MiB write-buffer limits. These are shared storage-resource
settings, not a whole-process memory cap; the upstream implementation also
documents the cache limit as soft:

- <https://github.com/typedb/typedb/blob/3.13.0/storage/keyspace/rocks_resources.rs>
- <https://github.com/typedb/typedb/blob/3.13.0/server/parameters/config.rs>

Server allocation profiling and a representative large publication remain
necessary before choosing allocator/cache changes or production service
limits. Full snapshots, extracted facts, stored graphs, sealed evidence, and
TypeDB membership validation still scale with corpus size. No facts or
repositories were excluded to produce the reductions above.
