# First delivery: session migration

Status: proposed implementation scope. No `chaosbox sessions` commands exist
yet. The goal is to replace migration scratch scripts with one bounded Rust
workflow inside Chaosbox, while implementing the [selective intelligence
lifecycle](intelligence.md).

## Capability boundaries

These are logical modules, not six new crates:

| Module | Responsibility | Existing foundation |
| --- | --- | --- |
| `session-ingest` | Adopt verified snapshots and capture source records | Snapshot identities and validation |
| `session-reconcile` | Deduplicate, recover verified truncation, retain variants | Deterministic evidence handling |
| `session-knowledge` | Candidate admission, consolidation and repository links | Core claims, Jev decisions, TypeDB |
| `session-checkpoint` | Validated continuation views | Budgets, identities and resumable tasks |
| `session-transfer` | Runtime conversion/export and completeness accounting | Native OpenCode facilities plus validation |
| `session-query` | Bounded continuation/context and evidence retrieval | Shared read-only CLI/MCP consumers |

Start with ingestion and reconciliation. Establish knowledge admission on a
representative pilot before scaling checkpoint generation and runtime transfer.
Add at most one focused session-processing crate if needed to isolate SQLite and
runtime-adapter dependencies from the graph core. No separate service, provider
framework, embeddings platform or UI is required for this delivery.

## Adopt evidence already collected

The OpenCode migration has consistent database snapshots, native-conversion
working copies, reconciliation records and partial compaction results outside
this repository. Treat them as inputs to verify, not as evidence that the
migration finished. Validate source hashes, schema versions, parent relationships
and artifact identities before reusing them.

Do not copy private transcripts, credentials, database files or campaign outputs
into Git. Commit only sanitized fixtures and reproducible validation logic.
Avoid recopying the corpus or re-requesting successful inference when input and
transform identities still match.

## Reconciliation before interpretation

- Exact duplicates collapse to one canonical representation.
- A verified historical prefix is redundant only relative to a complete,
  compatible continuation; timestamps alone do not establish this.
- Restore truncated text only when the fuller archived value matches the
  retained prefix and the truncation is explicit. Record both source identities.
- Preserve genuinely divergent histories with provenance rather than choosing
  one silently or blending contradictory conversations.
- Preserve and account for interrupted/unsettled operations. Never turn them
  into completed work to satisfy an import schema.

Native runtime conversion remains responsible for runtime-specific semantics.
Chaosbox owns isolation, reconciliation, provenance, orchestration and
before/after completeness verification. Do not implement a second OpenCode
migrator where a supported native facility already covers the operation.

## Intelligence and continuation are separate outputs

Before reducing a session's context, extract candidates from original source
records and pass them through Jev's bounded assessment/admission policy. Link
accepted items to repository entities, while retaining time, scope, contrary
evidence and unresolved questions. A high-value finding can then help a different
session without importing the original conversation wholesale.

For this migration, the requested continuation model is
`muse-code/muse-spark-1.3-contributor` at `xhigh`, through the Muse Code subscription
route. Verify the exact model and account entitlement; never silently substitute
another provider or a pay-as-you-go credential. This is a campaign choice, not a
replacement for Chaosbox's pinned Jev decision model.

Checkpoint generation must:

1. Preserve the latest user instructions, decisions and rationale, findings,
   unresolved work, approvals, execution state and relevant evidence references.
2. Distinguish observed results from unsupported assistant success claims.
3. Remove repetition and resolved chatter without losing unresolved facts.
4. Keep recent user context and coherent tool-call/result boundaries available.
5. Reference recoverable attachments without pretending they were interpreted.
6. Record source, normalization, prompt/schema, model and effort identities.
7. Validate both checkpoint and audit schemas with the same fail-closed predicate
   used to authorize publication. `pass: true` alone is not a valid audit.
8. Stream bounded chunks, persist accepted progress, and avoid repeating work
   after a restart. Validate end-to-end continuity as well as local chunks.

Incomplete responses, malformed JSON, transport termination and failed audits
retain the original history and remain explicit failures. An abstention on
intelligence admission is not a failed checkpoint request; keep those outcomes
separate. A generated checkpoint never counts as fresh corroborating evidence.

## Operator surface

Extend the existing Rust CLI. A proposed command family is `chaosbox sessions`
with ingest, reconcile, compact, status, verify, export and resume operations.
Names and arguments must be finalized against the implementation before they
are presented as runnable examples.

Mutating operations need a campaign identity, bounded budgets, restartable state
and machine-readable reports. Reuse the existing task/publication machinery
where its storage contracts apply. Resume produces a cited continuation packet;
it does not execute commands copied from history. MCP remains read-only.

## First-release gates

- Every source session and divergent variant is accounted for in a canonical
  destination or an explicit recoverable exception.
- Original snapshots remain intact, and every deduplication/recovery is
  explainable from source evidence.
- Admission rejects unsupported, duplicate and inapplicable candidates on a
  labelled pilot; conflicting evidence survives consolidation.
- Another session can retrieve relevant intelligence and inspect its sources
  without an inference call or exposure of private data outside its scope.
- Completed checkpoints pass structural and continuity checks and can be
  resumed by the actual target runtime. Failed compaction never deletes context.
- Runtime migration is rehearsed on copies, with a final delta/freeze step and
  executable-and-data rollback before any production switch.

The first delivery is not complete merely because an archive exists, a graph is
large, summaries look fluent, or a destination database opens. It is complete
when retained knowledge is grounded, selectively admitted, retrievable and useful
for reliable continuation.
