# S03b.1 — Knowledge validation and current eligibility

Status: investigation complete, 2026-10-18. The [receipt](s03b1-knowledge-validation.json)
binds the source, focused checks and retained evidence. S03b.2 remains open.

The current and exact TypeDB queries already select one keyed generation. Their
large result is the complete knowledge envelope. The existing record/evidence
mirrors cannot replace that envelope while preserving its hash and complete
reference checks. Backend-targeted retrieval therefore needs S05's versioned
validation contract and the unresolved S01b gate. This investigation changes no
runtime code, schema, historical identity or publication guard.

## Read paths and identity boundaries

Here `H(parts...)` means `chaosbox_core::sha256_hex`: UTF-8 bytes followed by a NUL
for **each** part, including the last. It is not an ordinary file SHA-256.

| Entry point | Generation identity and validation owner |
|---|---|
| `TypeDbReader::knowledge` / `TypeDbStore::knowledge` | `knowledge::current` joins the scope-keyed pointer to the same-scope build and returns `(id, raw-envelope)` in one read transaction. It does not decode or validate a `Bundle`. |
| `knowledge_at(scope, id)` on either handle | `knowledge::at` selects the exact build and scope without consulting the current pointer. Missing/foreign scope returns `None`; there is no latest fallback. |
| Federation `Backend::Typedb` | Current/exact adapters reject envelopes above 64 MiB and require `id == H("session-knowledge-v1", scope, raw)` before decoding. `Reader::check` then validates the complete bundle, exact scope and ID shape. |
| Compaction CLI `pinned` | Reads the current envelope, verifies the same raw-envelope hash, decodes and validates the bundle, then applies the requested context/evidence projection. |
| Federation `Backend::Bundle` / startup-pinned MCP bundle | `load_bundle` loads and validates the artifact; its snapshot ID is `H(JSON(loaded Bundle))`. Artifact history checks this exact ID. It is a different identity domain from TypeDB. |
| Synchronized `CurrentReader` | Refreshes `replica_current(user, scope)` once per request, bounds/decodes the complete `View`, calls `View::validate`, and checks pointer ID plus bundle scope. Its digest identifies the signed event set, not serialized view bytes. |

Sources: `chaosbox-typedb/src/knowledge.rs:9–32`, `reader.rs:181–190`,
`store/memory.rs:28–105`; `chaosbox/src/federation/source.rs:16–126`,
`provider.rs:23–81`, `compaction/cli.rs:123–150`, `intelligence/cli.rs:130–177`,
`intelligence/mod.rs:48–55`, `sync/current.rs:29–64`, and
`chaosbox-core/src/lib.rs:19–28`. Paths are relative to `crates/`.

Two compatibility details matter:

- A version-2 artifact manifest deliberately loads only receipts referenced by its
  records for ordinary consumers; writer loading can request all receipts. The
  in-memory bundle becomes version 1 and its digest covers what was loaded. Do
  not equate the artifact manifest/index with TypeDB's full raw-envelope identity.
- An absent TypeDB knowledge pointer yields an empty unpublished `Bundle` in the
  federation adapter, with an artifact-style digest. A missing database is an
  error for this read-only adapter. The older compaction `TypeDbStore` path can
  create a missing database through `ensure_connected`; it still needs an explicit
  schema migration. These are distinct existing contracts.

`Snapshot::new` only computes a digest. `Bundle::digest` only serializes/hashes.
Neither is a validation certificate; the artifact loader/provider check owns
validation. The TypeDB adapter's raw hash check must also remain at its boundary.

## Complete-bundle checks

`Bundle::validate` (`intelligence/mod.rs:68–192`) validates all records and
assessments before context ranking or evidence selection:

- Bundle version/scope, unique record IDs and unique assessment IDs.
- Every assessment's identity, scope, pinned requested/returned model, accepted
  rubric version, question set and typed response validity. The assessment ID
  includes its decision fields and optional replay state.
- Record scope, nonempty evidence/repository associations, inferred evidence
  class and verbatim wording from its first evidence occurrence.
- Each evidence occurrence's serialized digest and repository list must bind to
  a referenced receipt. Every receipt reference must resolve.
- Record contradiction/supersession targets must exist in the complete generation;
  contradictions must be symmetric and non-self-referential.
- Record ID and primary receipt must agree on candidate, repositories, first
  evidence and an admitting outcome. Status is recomputed from retained receipts,
  including supersessions elsewhere in the generation and later withholding.

Filtering first could hide a dangling reference, invalid unrelated receipt or a
superseding assessment and admit a result the current reader rejects. An output
record/character limit bounds the projection, not this validation/materialization.

These checks establish internal bindings, not a fresh read of original session
bytes. They also do not perform the replica import's stronger source-capsule and
semantic replay checks. Legacy assessment rubrics v1–v6 remain readable; ordinary
projections report `needs_revalidation` for old policy rather than rejecting all
historical receipts. Preserve that compatibility.

## Historical federation evidence

`federation/provider.rs:33–69,185–244` enforces this sequence:

1. Validate request bounds and exact provider/owner/scope/project identity against
   deployment policy and the authenticated recipient.
2. Read and validate the current generation. The requested record must still be
   eligible there: same scope, mapped repository, admitted/disputed status, and
   owner access or a current selected/all-admitted grant.
3. Reuse that request's current snapshot when the handle matches it; otherwise
   read and validate the exact historical generation. Missing history is
   `SnapshotUnavailable`, never a latest-generation substitution.
4. Project the historical record and relationships only through the intersection
   of current and historical eligibility under the request's current policy.
   Deleted, withheld, superseded or unselected knowledge cannot be resurrected by
   an old handle. Hidden relationships contribute omission counts.

`FileProvider` reloads grants/backend settings on every request and verifies its
pinned provider identity. `Federator::evidence` routes only to the configured
origin and validates the returned handle/projection; it cannot independently
validate the owner's full bundle. See `policy.rs:125–153`,
`transport.rs:45–90` and `reader.rs:108–153`. This is request-observed policy and
current eligibility, not continuous revocation checking during output.

Exact storage lookup itself is not historical authorization. In particular,
`stage_knowledge` inserts the full build envelope **before** staging mirrors and
advancing the pointer. `knowledge_at` has no separate “was published” predicate.
Do not infer a graph-style publication seal or complete mirror admission from the
mere existence of `session-knowledge-build`.

## Synchronized replicas

The replica writer and consumer have separate responsibilities:

- `publication_from_sources` checks original normalized snapshot digests and
  reconstructs every retained candidate. `Publication::validate` requires exact
  candidate/receipt coverage, current-policy replayable state, source-capsule
  bindings, and classification/relationship metadata replay. The signed publisher
  attests capture; peers do not receive the complete private source archive.
- `SignedEvent::validate` verifies enrollment, exact user/scope, event identity,
  device signature, parent ordering and payload. `persist_event` validates before
  writing; `load_replica` pages all scoped history and revalidates each event.
- Projection requires complete acyclic causal history. Reconciliation receipts
  must match their causal source frontier and record versions; concurrent
  disagreement remains an explicit unresolved abstention. `publish_current`
  publishes by predecessor guard and retries by rereading/merging history.
- `Replica::digest` is `H(user, scope, JSON(sorted event IDs))`. The persistence
  layer checks opaque row bounds/isolation and rejects same-ID/different-byte
  collisions; it does not validate application JSON or event signatures.
- `View::validate` checks its full bundle, metadata, frontier shape, relationship
  endpoints/receipt references, unique pairs, scope and semantic receipt replay.
  It does **not** reconstruct the event-set digest or redo enrollment/signatures
  from the persisted `View`. `CurrentReader` trusts the locally published view
  boundary after these checks and pointer ID/scope comparison.
- A warmed consumer retains its last good view on backend/validation failure and
  marks responses degraded. A cold reader errors. It pins user/scope at startup,
  accepts no wire override, and clones the refreshed view before projecting.

Sources: `sync/sources.rs:9–74`, `mod.rs:44–123,166–255,293–345`,
`persistence.rs:5–95`, `projection.rs:43–182`, `reconcile.rs:86–139`,
`current.rs:29–152`, `chaosbox-store/src/replica.rs:56–96`, and
`chaosbox-typedb/src/store/replica.rs:116–230`.

The live test named `signed_history_and_live_consumers_survive_real_typedb_restarts`
reopens a store handle; it does not restart the TypeDB server. This session proves
fresh-handle readback, not server-crash recovery.

## Reuse and implementation gate

Reuse the existing keyed reads, `read_rows_in`, scope-aware adapters, receipt
identities and complete validators. Request-local borrowing/indexing after one
successful validation could remove repeated scans/serialization and full-bundle
clones (`Reader`'s equal-current historical branch and `CurrentReader::query`).
That can be investigated without changing persisted identities, but it does not
make the first read a small backend result. No performance improvement is claimed
by this no-code workcard.

S03b.2's small-result backend path requires S05 to define a versioned complete
generation/admission proof binding records, assessments, source references,
lifecycle dependencies, coverage and exact payloads. Existing mirrors already use
generation-scoped item keys `H(build, intelligence ID)` and scope-bound evidence
keys; they lack an independent complete-generation validation proof and separate
receipt representation. S01b must be resolved before substantial schema/storage
implementation. Preserve all legacy envelope/hash/export paths during migration.

S04b may reuse only validated immutable work. Its future contract must bound
bytes/entries/lifetime and distinguish backend, address/database/credentials,
owner/scope, exact generation, and validator/schema/model/rubric identity. It must
repin current state and recheck recipient/project/grants each request. A replica
event-set digest alone does not bind projected view bytes or a projection-code
version. Neither graph-driver reuse nor this report authorizes a knowledge cache.

Before replacing whole-envelope reads, require small-result/large-generation
parity, rejection of corruption outside the selected result, missing/asymmetric
references, scope/credential/backend isolation, pointer/policy movement, historical
denial and absent-history behavior, replica payload/version movement and degraded
fallback, and fresh-handle TypeDB readback. Source-backed original-byte validation
and signed import checks remain at their existing boundaries.

## Verification and handoff

**55 focused tests passed** across intelligence, federation, compaction, sync and
sync-current. Four opt-ins were ignored in that aggregate. The compaction and sync
TypeDB opt-ins subsequently passed explicitly, as did federation's TypeDB case
and two backend knowledge/replica cases: **five required live checks passed, zero
ignored**. The plugin/live-model opt-ins and 17 other backend live cases were not
run. Treefmt CI and whitespace checks pass. No runtime source changed, so Clippy
was not rerun.

The first compaction invocation omitted its documented migrated-schema
prerequisite and failed with `INF2` for `session-knowledge-pointer`. Its log is
retained. Explicit CLI migration of a fresh disposable database preceded the
successful run; no product code or test was changed to obtain the pass.

Evidence: `/data/scratch/tmp/opencode/chaosbox-s03b1-20261018/`, including
`commands.txt`, raw logs, caller/schema traces, exact source/test executables,
preservation patches and `qualification.sha256`. The dirty source archive matches
S04a.2 byte-for-byte: SHA-256
`e8c7e61e21d59d99f49525e2c415f7baf412955d227493ca3c8798b4d75b4287`.
Chaosbox baseline: `e89a768bcfdcbf02ff9a69f6989d80ffe841d97d`; Canix baseline:
`ae9628b083a27c433dc6ec1af8705901e508172f`.

Native workers used the existing approved project environment, two Cargo jobs,
two Tokio workers, 200% CPU, a 4 GiB ceiling and zero permitted swap. The disposable
TypeDB CE 3.13.0 server peaked at 235,544,576 bytes with zero swap; final active
transactions were zero. It stopped with SIGTERM, its listeners are gone, and the
production TypeDB PID/start identity is unchanged. These fixture observations do
not establish production RAM acceptance. Exact cumulative context telemetry was
unavailable.

Canonical checkout assertions passed. The fleet-wide workspace check failed on
the installed CLI's missing tracked `.gitmodules`/existing gitlinks contract; its
log is retained as a separate tooling finding. This session preserves foreign
Chaosbox work and makes no Canix source/index writes. Savepoint publication,
consumer promotion and activation remain separate scopes.

Next: resolve **S01b** and freeze **S05a**'s versioned multi-project corpus before
S05's schema contract. S03b.1 is complete; S03b.2 and S04b remain open behind their
applicable gates.
