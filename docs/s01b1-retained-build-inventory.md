# S01b.1 — native retained-build inventory

The bounded, credential-file-backed inventory prerequisite is locally qualified
in signed commit `4704623025ee23a977ee5759043bc73e9c209836`.
**Parent S01b remains open for allocation and physical-storage attribution.**
See the [receipt](s01b1-retained-build-inventory.json) and the
[resource-attribution report](s01b-resource-attribution.md).

## Interface and guarantees

`chaosbox db inventory --repo REPO [--limit N] [--after BUILD_ID]` reuses the
TypeDB driver, credential-file loader and lifecycle-v2 error envelope under page
contract `chaosbox.build-inventory.v1`.

- Default page size is 20 (range 1–50). Repository and boundary labels must be
  nonblank, control-free and at most 256 bytes. Validation runs before
  credentials and connection acquisition; TypeQL literals use the shared encoder.
- One read transaction reads the repository pointer, ordered headers and three
  scalar membership counts per returned header; one lookahead header sets
  `complete` and the exclusive `next_after` boundary. Later pages take fresh
  read snapshots, so concurrent publication is not frozen.
- The ten-second CLI deadline covers credential loading, connection acquisition
  and backend queries; the read transaction also has a ten-second lifetime.
- `is_current` compares the pointer with the header ID. Historical headers can
  still carry `status = "active"`, so stored status alone does not prove
  currentness or publication admission. Unpublished `staging` headers remain
  inspectable.
- Counts cover node/edge memberships and edge memberships with a sealed payload
  attribute. They do not fetch payloads, validate seals, count unique physical
  objects across builds, or cover knowledge-envelope/MVCC history.
  `physical_bytes` is `null`.

The CLI issues at most 152 queries per transaction (pointer, header page, plus
three aggregates for up to 50 headers). The `ponytail:` comment records that
round-trip ceiling and the measured-work trigger for grouped aggregates. Scalar
output and a deadline do not set a production scan/RAM budget.

## Live read-only observation

The retained candidate queried repository `canix` at `127.0.0.1:1729` using the
`chaosbox-app` credential file. One complete page returned:

| Generation | Build | Current | Nodes | Edges | With sealed attribute |
| --- | --- | --- | ---: | ---: | ---: |
| 1 | `build:92d8228e6816` | no | 43,789 | 33,656 | 0 |
| 2 | `build:e52e3a65bbb8` | no | 44,842 | 34,218 | 34,218 |
| 3 | `build:8ac2e28136c3` | yes | 65,727 | 45,815 | 0 |

All three stored statuses were `active`. This independently confirms three
retained headers, which generation 3 alone did not establish. Sealed-attribute
presence is not an evidence-loss or admission verdict.

GNU time recorded **0.91 seconds** elapsed and **25,724 KiB** peak client RSS.
The before/after server counters are retained, but background work was not isolated
and the server already held swap, so those counters cannot attribute server
memory or I/O to the read. Production PID `4144752` and monotonic start
`158470931838` stayed unchanged.

The installed wrapper still invokes its existing store executable; use the
retained candidate for this command, as consumer promotion remains separate.
Example in Bash:

```bash
env CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 \
  CHAOSBOX_TYPEDB_USER=chaosbox-app CHAOSBOX_TYPEDB_DATABASE=chaosbox \
  CHAOSBOX_TYPEDB_PASSWORD_FILE=/var/lib/typedb-auth/app-password \
  /data/scratch/tmp/opencode/chaosbox-s01b1-s05a-20261019/bin/chaosbox-inventory \
  db inventory --repo canix --limit 20
```

## Qualification and provenance

The affected native suite passed **94 tests**, ignoring four opt-in cases in the
aggregate run; the inventory case, unpublished-header case and backend reader
conformance then passed explicitly against the disposable server (**three
required native cases, zero ignored**). The other three aggregate opt-ins were
not qualified here.

The native inventory fixture checks two published generations, exact counts,
exclusive pagination, foreign/absent repositories, escaped labels and stable
readback. The unpublished-header case verifies inspection while ordinary
published reads keep rejecting that header. Dedicated CLI regressions cover
credential-free malformed requests and a stalled-acquisition deadline; the
pre-change binary failed the new command regression. Scoped warnings-denied
Clippy, treefmt CI and whitespace checks pass. An initial duplicate-`--lib`
invocation failed before running tests; only `affected-inventory-final.log`
establishes the 94-test pass.

Evidence is retained at
`/data/scratch/tmp/opencode/chaosbox-s01b1-s05a-20261019/`, including the exact
CLI, `source-inventory.tar`, source checksums, command record and native logs. The
source archive retains dirty/untracked code and excludes `docs/` and `tasks/`
(scoped qualification, not a clean release). Staged and committed patch bytes
match, and the pre-existing tracked diff is preserved.

Workers ran with two Cargo jobs, two Tokio workers, 200% CPU quota, 2 GiB
memory-high, 4 GiB memory-max and zero allowed swap. Disposable TypeDB peaked at
249,159,680 bytes with zero swap, ended with zero active transactions, and
exited after SIGTERM with its listeners gone (worker/fixture observations).

## Remaining S01b gate

Unprivileged current-PID `smaps_rollup` reads and service-owned storage
traversal remain denied; the tool layer rejected the privileged read before
execution (`access-limit.md` records the exact command). The protected resource
is the running TypeDB process/storage owned by `typedb`. Collecting that
evidence requires explicit read-only tool authorization and an allocator/storage
surface that separates live allocations, allocator retention, caches, retained
graph/knowledge payloads and engine overhead; smaps alone is insufficient.

The operator's deferred resource-policy decision stays in force. Substantial S05
schema/storage work and representation-dependent S03b.2/S04b remain behind S01b.
