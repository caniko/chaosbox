# First delivery: session migration

Status: partially implemented. `chaosbox sessions` offers `status`, `verify`,
`adopt`, `install` and `rollback` (`crates/chaosbox/src/sessions/cli.rs`).
Ingestion, reconciliation, checkpoint generation and runtime transfer remain
unimplemented in this scope. The goal is one bounded Rust workflow in Chaosbox
that replaces migration scratch scripts and implements the [selective intelligence
lifecycle](intelligence.md).

## Capability boundaries

These are logical modules, not six new crates:

| Module | Responsibility | Existing foundation |
| --- | --- | --- |
| `session-ingest` | Adopt verified snapshots and capture source records | Snapshot identities and validation |
| `session-reconcile` | Deduplicate, recover verified truncation, retain variants | Deterministic evidence handling |
| `session-knowledge` | Candidate admission, consolidation and repository links | Core claims, Jev decisions, PostgreSQL |
| `session-checkpoint` | Validated continuation views | Budgets, identities and resumable tasks |
| `session-transfer` | Runtime conversion/export and completeness accounting | Native OpenCode facilities plus validation |
| `session-query` | Bounded continuation/context and evidence retrieval | Shared read-only CLI/MCP consumers |

Start with ingestion and reconciliation. Establish knowledge admission on a
representative pilot before scaling checkpoint generation and runtime transfer.
Add at most one focused session-processing crate if needed to isolate SQLite and
runtime-adapter dependencies from the graph core. This delivery needs no
separate service, provider framework or UI. Before 0.2.0 it also excluded an
embeddings platform. In 0.2.0, PostgreSQL + pgvector replaced TypeDB and stores
vectors from an existing loopback embedding endpoint for retrieval ranking.
Delivery does not depend on embeddings: retrieval falls back to lexical.

## Adopt evidence already collected

Outside this repository, the OpenCode migration has consistent database snapshots,
native-conversion working copies, reconciliation records and partial compaction
results. These are inputs to verify, not proof of completion. Before reusing
them, validate source hashes, schema versions, parent relationships and artifact
identities.

Do not copy private transcripts, credentials, database files or campaign outputs
into Git. Commit only sanitized fixtures and reproducible validation logic.
If input and transform identities still match, avoid recopying the corpus or
repeating successful inference.

## Reconciliation before interpretation

- Exact duplicates collapse to one canonical representation.
- A verified historical prefix is redundant only relative to a complete,
  compatible continuation; timestamps alone do not establish this.
- Restore truncated text only when the fuller archived value matches the
  retained prefix and the truncation is explicit. Record both source identities.
- Keep divergent histories with provenance. Don't silently pick one or blend
  contradictory conversations.
- Keep and account for interrupted/unsettled operations. Never mark them complete
  to satisfy an import schema.

Native conversion handles runtime-specific semantics.
Chaosbox owns isolation, reconciliation, provenance, orchestration and
before/after completeness verification. Use a supported native facility when it
covers the operation instead of writing a second OpenCode migrator.

## Intelligence and continuation are separate outputs

Before reducing a session's context, extract candidates from original source
records and pass them through Jev's bounded assessment/admission policy. Link
accepted items to repository entities and retain time, scope, contrary evidence
and unresolved questions. Other sessions can use high-value findings without
importing the whole conversation.

Continuation uses the same Jev-only policy as graph decisions, intelligence
admission and research. Deterministic processing lists source records and
coherent tool-call/result groups. Pinned Jev choices classify, select, prioritize
and relate those records. Rust uses deterministic templates to assemble a
fixed-schema packet and cited text. No model writes checkpoint prose or invents
tasks, quotations or evidence references. For ambiguous decisions, keep the
source and an explicit unresolved state. This replaces the earlier generative-model
campaign choice. The bounded `checkpoint prepare` and `checkpoint assemble`
commands implement source-selected rendering and persistent spending; see
[Source-selected continuation](checkpoints.md). Automatic native transfer and
the full continuity/audit workflow remain separate implementation milestones.

Checkpoint generation must:

1. Preserve the latest user instructions, decisions and rationale, findings,
   unresolved work, approvals, execution state and relevant evidence references.
2. Distinguish observed results from unsupported assistant success claims.
3. Select cited records and record omissions; unresolved instructions and facts
   cannot be dropped by a low-confidence selection or a presentation budget.
4. Keep recent user context and coherent tool-call/result boundaries available.
5. Reference recoverable attachments without pretending they were interpreted.
6. Record source, normalization, schema/template, Jev model/rubric and decision
   receipt identities. Preserve quotations and structured result fields exactly.
7. Validate both checkpoint and audit schemas with the same fail-closed predicate
   used to authorize publication. `pass: true` alone is not a valid audit.
8. Stream bounded chunks, persist accepted progress, and avoid repeating work
   after a restart. Validate end-to-end continuity as well as local chunks.

Keep the original history and record incomplete decisions, malformed JSON,
transport termination and failed audits as explicit failures. Keep intelligence
admission abstentions separate from checkpoint failures. A rendered checkpoint
never counts as fresh corroborating evidence.

## Operator surface

Extend the existing Rust CLI. A proposed command family is `chaosbox sessions`
with ingest, reconcile, compact, status, verify, export and resume operations.
Finalize names and arguments against the implementation before publishing
runnable examples.

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

The first delivery is complete when retained knowledge is grounded, selectively
admitted, retrievable and useful for reliable continuation. An archive, a large
graph, fluent summaries or a destination database that opens do not suffice.
