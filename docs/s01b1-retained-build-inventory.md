# S01b.1 — native retained-build inventory

The bounded, credential-file-backed inventory prerequisite is locally qualified
in signed commit `4704623025ee23a977ee5759043bc73e9c209836`.
**Parent S01b remains open for allocation and physical-storage attribution.**
See the [receipt](s01b1-retained-build-inventory.json) and the original
[resource-attribution report](s01b-resource-attribution.md).

## Interface and guarantees

`chaosbox db inventory --repo REPO [--limit N] [--after BUILD_ID]` uses the
existing TypeDB driver, credential-file loader and lifecycle-v2 error envelope.
Its page contract is `chaosbox.build-inventory.v1`.

- The default page size is 20; the accepted range is 1–50. Repository and boundary
  labels are nonblank, control-free and at most 256 bytes. Validation precedes
  credentials and connection acquisition; TypeQL literals use the shared encoder.
- One read transaction observes the repository pointer, ordered headers and three
  scalar membership counts per returned header. A single lookahead header
  determines `complete` and the exclusive `next_after` boundary. Following pages
  take fresh read snapshots, so concurrent publication is not frozen.
- The ten-second CLI deadline includes credential loading, connection acquisition
  and backend queries. The read transaction also has a ten-second lifetime.
- `is_current` compares the pointer with the header ID. Historical headers may
  still have `status = "active"`; stored status alone is not currentness or
  publication admission. Unpublished `staging` headers are inspectable.
- Counts describe node/edge memberships and edge memberships carrying a sealed
  payload attribute. They do not fetch payloads, validate seals, count unique
  physical objects across builds, or cover knowledge-envelope/MVCC history.
  `physical_bytes` is explicitly `null`.

The CLI requests at most 152 queries in one transaction (pointer, header page,
then three aggregates for each of 50 headers). The `ponytail:` comment records
this round-trip ceiling and the measured-work trigger for grouped aggregates.
Scalar output and a deadline do not establish a production scan/RAM budget.

## Live read-only observation

The retained candidate read repository `canix` at `127.0.0.1:1729` using the
existing `chaosbox-app` credential file. One complete page returned:

| Generation | Build | Current | Nodes | Edges | With sealed attribute |
| --- | --- | --- | ---: | ---: | ---: |
| 1 | `build:92d8228e6816` | no | 43,789 | 33,656 | 0 |
| 2 | `build:e52e3a65bbb8` | no | 44,842 | 34,218 | 34,218 |
| 3 | `build:8ac2e28136c3` | yes | 65,727 | 45,815 | 0 |

All three stored statuses were `active`. This query independently establishes
three retained headers; generation 3 by itself did not establish that count.
Sealed-attribute presence is not an evidence-loss or admission verdict.

GNU time recorded **0.91 seconds** elapsed and **25,724 KiB** peak client RSS.
The before/after server counters are retained, but background work was not
isolated and the server already carried swap. Those counters cannot attribute
the read's server memory or I/O. Production PID `4144752` and monotonic start
identity `158470931838` remained unchanged.

The installed wrapper still selects its existing store executable. Use the
retained candidate for this new command; consumer promotion remains separate.
Example, explicitly in Bash:

```bash
env CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 \
  CHAOSBOX_TYPEDB_USER=chaosbox-app CHAOSBOX_TYPEDB_DATABASE=chaosbox \
  CHAOSBOX_TYPEDB_PASSWORD_FILE=/var/lib/typedb-auth/app-password \
  /data/scratch/tmp/opencode/chaosbox-s01b1-s05a-20261019/bin/chaosbox-inventory \
  db inventory --repo canix --limit 20
```

## Qualification and provenance

The affected native suite passed **94 tests**. Four opt-in cases were ignored in
that aggregate; the inventory case was then explicitly required and passed on
the disposable server, alongside the unpublished-header case and backend reader
conformance: **three required native cases, zero ignored**. The remaining three
aggregate opt-ins were not qualified by this workcard.

The native inventory fixture checks two published generations, exact counts,
exclusive pagination, foreign/absent repositories, escaped labels and stable
readback. The unpublished-header case checks inspection while ordinary published
reads continue to reject that header. Credential-free malformed requests and a
stalled-acquisition deadline have dedicated CLI regressions. The pre-change
executable failed the new command regression. Scoped warnings-denied Clippy,
treefmt CI and whitespace checks pass. An initial duplicate-`--lib` invocation
failed before executing tests; only `affected-inventory-final.log` establishes
the 94-test result.

Evidence is retained at
`/data/scratch/tmp/opencode/chaosbox-s01b1-s05a-20261019/`, including the exact
CLI, `source-inventory.tar`, source checksums, command record and native logs.
The source archive includes preserved dirty/untracked code and excludes `docs/`
and `tasks/`; this is scoped qualification, not a clean release. The staged and
committed patch bytes match, and the pre-existing tracked diff is preserved.

Workers used two Cargo jobs, two Tokio workers, a 200% CPU quota, 2 GiB memory-high,
4 GiB memory-max and zero allowed swap. The disposable TypeDB peaked at
249,159,680 bytes with zero swap, ended with zero active transactions, and exited
after SIGTERM with its listeners gone. These are worker/fixture observations.

## Remaining S01b gate

The unprivileged current-PID `smaps_rollup` read and service-owned storage
traversal remain denied. The tool layer rejected the privileged read before
execution; `access-limit.md` records the exact command. The protected resource is
the running TypeDB process/storage, owned by the `typedb` service. Proceeding with
that evidence requires explicit read-only tool authorization and a selected
allocator/storage measurement surface capable of distinguishing live allocations,
allocator retention, caches, retained graph/knowledge payloads and engine overhead.
Smaps alone would still be insufficient for that attribution.

The operator's deferred resource-policy decision remains in force. Substantial
S05 schema/storage work and representation-dependent S03b.2/S04b stay behind S01b.
