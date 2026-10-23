# S03a — keyed build and membership read audit

**The production TypeDB readers already satisfy S03a's materialization invariant.**
No production reader rewrite is warranted. This slice strengthens the existing
shared conformance check and records its real-backend result. It does not establish
whole-stack resource acceptance or bounded reads across the entire MCP surface.

See the [receipt](s03a-keyed-membership-audit.json),
[S02](s02-bounded-navigation.md) and [workcards](../tasks/session-efficiency/todo.md).

## Read contracts and actual callers

The audit uses canonical HEAD `42f0e64fddd50f612516e39e82d5c2ea4d2ead45` plus
its retained dirty patch. Only `crates/chaosbox-store/src/conformance.rs` changes
code in this slice. Copied sources, the tested patch and binary hashes are in the
evidence bundle.

| Read | Existing implementation | Materialization/query bound |
| --- | --- | --- |
| Active header | `chaosbox-typedb/src/reader.rs:296`; schema keys the pointer by repository and build by build ID | One header query; a successful lookup adds one exact-build snapshot query |
| Historical header | `reader.rs:303` binds repository, build and published status | Missing/foreign/unpublished headers reveal no members; successful lookup includes all snapshots of that build |
| Entity lookup | `reader.rs:349` binds build membership and entity ID | One keyed member query; no complete build projection |
| Entity prefix | `reader.rs:216,381` | TypeQL sorts by qualified name then ID and applies `max(0, limit)` before returning rows |
| Relationship prefix | `reader.rs:235,389` | TypeQL sorts by relationship ID and applies the limit before returning rows |
| Schema probe | `reader.rs:182` | One query projects at most one pointer row (`limit 1`); no member collection |

The counts above are **source-derived**, not measured wire-query counters. Header
snapshot materialization is proportional to the selected build's snapshot list;
truncating it would weaken freshness/authorization checks. Coverage metadata also
remains complete. Existing prefix APIs are not cursor-pagination APIs.

All production callers of these methods were traced:

- Ordinary CLI/MCP uses `AnyReader::connect` in `chaosbox/src/backend_cmd.rs`,
  then `GraphReader::pinned`; lookups/status/context use that pinned identity.
- `GraphReader::export` requests node/edge caps plus one sentinel row. `diff`
  validates both historical headers before either membership projection and
  preserves its explicit overflow error (`chaosbox/src/reader.rs:231,314`).
- Run-bound `ScopedReader::admit/load` validates the exact published header,
  evidence seal and snapshots, then requests its declared caps plus one before
  byte checks and source filtering (`chaosbox/src/read_view/mod.rs:105`). This
  intentionally materializes an admitted bounded view, not an arbitrary database.
- Publication-chain selection and backend readiness in `backend_cmd.rs`, and
  PostgreSQL publication in `postgres_cmd.rs`, use keyed headers. The legacy
  relation-presence gate requests only one relationship.
- Schema readiness in `backend_cmd.rs:172` calls `TypeDbReader::probe`. Its
  schema-presence query matches several types without repository pinning, but
  returns at most one row and exposes no graph members. This is a source audit;
  the focused conformance run does not separately exercise the probe. Its server
  execution cost remains unmeasured.
- Remaining direct callers are conformance/recovery tests or test forwards.
  `GraphQueries` explicitly requires trusted callers to establish ownership
  before raw member reads; the raw methods are not an authorization API.

## Checks and measurements

The shared conformance fixture already checked current/historical ownership
and missing membership. Its live seed also checks another repository's snapshot
isolation. Both seeds supply source spans; this helper does not assert span equality.
The extension adds current/historical snapshot assertions and **10 prefix cases**:
limits `-1, 0, 1, 2, 3` against each two-node/one-edge build, plus absent-build
relationship membership. It checks ordered names, snapshots and exact lengths.
Each case records returned counts, serialized JSON bytes and elapsed query time.

A deliberate negative control changed the memory reader to return one extra
entity. The new check failed at limit `-1` (`Alpha` instead of an empty result).
That mutation was restored byte-for-byte before qualification.

| Gate | Result |
| --- | --- |
| Negative control | Expected failure, exit 101 |
| Focused memory conformance | 1 passed, 0 ignored |
| Required disposable TypeDB conformance | 1 passed, 0 ignored; `CHAOSBOX_REQUIRE_TYPEDB=1` |
| Store library suite | 9 passed; one pre-existing isolated RSS test ignored, not counted as audit evidence |
| Warnings-denied store Clippy | Passed |
| Scoped treefmt and diff check | Passed |

The selected TypeDB 3.13.0 test executable completed in **0.42 s**, with
**17,288 KiB** peak client RSS. This includes migration, fixture publication and
driver setup, not just reads. The two selected builds each have two nodes and one
edge; another repository supplies an isolation check. The largest serialized
node/edge projection is **627 bytes**. Reference/live **boundary summaries** match
in build, limit, row counts and serialized byte lengths after removing timing.
This comparison does not establish equality of the row payloads. Serialized bytes
are not heap-allocation bytes.

The separate loopback server used `127.0.0.1:22729`, scratch-only data and a
600-second runtime ceiling. Its cgroup had a 4 GiB hard limit, no swap and two
CPU cores of quota; sampled peak RAM was 187,858,944 bytes. It was stopped cleanly,
with its fixture database and logs retained. These are test bounds, not adopted
production policy.

## Cost boundaries that remain open

`common.rs:191` collects raw driver rows, clones attributes into owned maps and
then decodes typed rows. Consequently multiple representations can coexist, but
for these TypeDB membership queries their row count follows the requested prefix,
not the complete database. This audit does not measure allocator-level ownership.

The memory reference backend deliberately materializes whole-build rows before
selecting prefixes. Its output is a semantic oracle, not a TypeDB performance
model. TypeDB scan/sort work behind a keyed query or `LIMIT` was not profiled;
small returned results do not prove constant server CPU, I/O or memory.

The tiny fixture is not a large-corpus benchmark. S01b attribution remains open,
and no numeric production RAM/latency/swap/context target is adopted. Future
neighborhood/path work must account for aggregate rows, bytes, query count,
whole-request deadline/cancellation and persistent-MCP retention.

## Evidence and handoff

Evidence is retained at
`/data/scratch/tmp/opencode/chaosbox-s03a-20261016/`. The receipt binds its checksum
manifest; the bundle includes before/tested source identities, source copies,
raw logs, boundary projections, native process metrics and stopped disposable
data. Verify with `sha256sum --check SHA256SUMS` in that directory.

S02a review clarification: the frozen bundle retains its original
`reference-live-projection-parity` gate label. The comparison is boundary-summary
parity as described above; the current receipt records that narrower claim. The
original report and receipt are also preserved in the S02a evidence bundle.

Reproduce the focused checks in the approved cached direnv with bounded Cargo
jobs. For live conformance, start the retained disposable configuration on a free
loopback port, use its disposable credential file and select the required gate:

```bash
direnv exec . cargo test --locked --offline -p chaosbox-store --lib \
  tests::memory_reader_passes_conformance --jobs 2 -- --exact --nocapture --test-threads=1
direnv exec . env TOKIO_WORKER_THREADS=2 CHAOSBOX_REQUIRE_TYPEDB=1 \
  CHAOSBOX_TYPEDB_ADDR=127.0.0.1:22729 CHAOSBOX_TYPEDB_USER=admin \
  CHAOSBOX_TYPEDB_PASSWORD_FILE=/data/scratch/tmp/opencode/chaosbox-s03a-20261016/disposable-password \
  cargo test --locked --offline -p chaosbox-typedb --test live \
  reader_passes_reference_conformance_against_live_backend --jobs 2 \
  -- --exact --nocapture --test-threads=1
```

The foreign Chaosbox tracked diff compares byte-identically with entry. Canix's
index matched entry after the tests; during final documentation another session
staged five Harbor DB paths: `flake.nix`, `flake.lock`,
`flake/checks/harbor_db.nix` and two database operations guides. All original
staging still compares byte-identically after excluding those added paths. Both
index snapshots and their delta are retained; this session never writes the index.
The production TypeDB PID/start identity is unchanged. This slice makes no
production database writes, activation, service restart, pin/index write or commit.

**Next workcard: S02a, exact count-only explanations shared by CLI/MCP.** Reviewed
follow-ups now distinguish fully enumerable neighborhoods, budgeted path search,
knowledge contract investigation and independent graph-read reuse. S25a source
attribution is resolved; actual consumer qualification remains a separate step.
