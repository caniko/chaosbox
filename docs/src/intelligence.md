# Selective intelligence

This proposed extension of Chaosbox's evidence pipeline hasn't shipped. Its
first application is
[preserving session knowledge during migration](session-migration.md).

## Objective

Extract intelligence when meaningful new evidence appears and make it available
to other LLM sessions. Intelligence is scarce; judge it by whether it improves
decisions, not by graph size or a catalogue of every utterance.

An intelligence item is a supported, scoped proposition or decision that reduces
uncertainty, prevents repeated mistakes or preserves a consequential insight.
Record enough provenance and applicability to use it safely outside its source
session.

Keep three artifacts distinct:

| Artifact | Purpose | Authority |
| --- | --- | --- |
| Source archive | Recover transcripts, repository snapshots, outputs and attachments | What was actually captured |
| Admitted intelligence | Reusable findings, decisions, constraints and relationships | Supporting evidence plus explicit admission policy |
| Continuation checkpoint | Help an agent resume a particular session | A generated view of a specified source range |

A checkpoint isn't new evidence for its own claims. Repeated summaries don't
corroborate an assertion independently. Preserve sources conservatively during
migration even if little qualifies for reusable intelligence.

## Pipeline and ownership

```text
repository snapshot / session records / tool results / user correction
                              |
               stable, source-addressed evidence
                              |
                  bounded atomic candidates
                              |
                  typed Jev assessments
                              |
        deterministic admission, consolidation and publication
                              |
              task-scoped retrieval with citations
```

Rust owns identity, validation, scope, budget enforcement and publication. Jev
adjudicates supplied candidates using the existing typed API. Every
Chaosbox-owned live inference uses the pinned Typesafe Jev model and endpoint;
model aliases, endpoint overrides, returned-model substitutions and redirects
are rejected. There is no general-purpose generation or fallback provider.
Candidates use exact source spans or deterministic templates. Jev selects and
classifies supplied records; Rust renders explanations and continuation packets
from those selections, with citations and explicit unresolved states.

Keep the code-extraction contract: copy or deterministically construct source
labels and evidence text. Classify rendered explanations separately as derived
artifacts. Don't loosen the contract to pass session summaries off as extracted
code facts.

## Evidence and candidate construction

Address repository evidence by repository, snapshot/revision and source span.
Session evidence needs a source snapshot, native session ID, message/part ID
and an exact excerpt or structured result field. Add a session source-reference
type instead of treating messages as repository files.

Sessions may span several repositories. Their working directory is an observed
location, not proof of sole ownership. Links to files, symbols and components
must preserve repository identity and the applicable revision or artifact hash.
Never merge entities merely because labels or paths look alike. Respect the
existing graph's build-membership guards when adding cross-repository links.

Start with cheap deterministic extraction: source spans, explicit user
decisions, command/result metadata, file/revision references and known
repository graph relationships. Language interpretation can add candidates,
but their evidence anchors must resolve before Jev assessment. For missing or
oversized context, abstain explicitly or request bounded expansion. Never
truncate silently.

"The build passed" from an assistant establishes only that the assistant made
the claim. To establish success, require a relevant tool result, its exit
status, the exact command and the tested source identity. A successful output
filter at the end of a pipeline doesn't by itself prove the build passed.

## Jev: standardize and dissect, not generate an archive

Reuse `chaosbox-jev` and its Noul, Choice and Score contracts:

| Assessment | Bounded question |
| --- | --- |
| Support | Does the supplied evidence support this proposition? |
| Atomicity | One coherent proposition, compound proposition, or insufficient context? |
| Kind | Observation, decision, constraint, hypothesis, reported claim, or noise? |
| Scope | Revision, repository, environment, cross-repository principle, or unknown? |
| Consolidation | Duplicate, refinement, supersession, contradiction, unrelated, or abstain? |
| Value | Explicit scored criteria for usefulness and likely durability |

These rubrics are proposed; this document does not claim the current
code-relation catalog implements them. Split compound candidates into
independently grounded propositions and reassess them. Give Choice options
descriptions, not bare opaque IDs. Compare against a bounded set of relevant
existing items, never every historical pair.

Questions within a Jev request are independent. Assessments that depend on an
earlier answer belong in a later stage. Noul supplies a probability; Choice and
Score have their own distribution/confidence fields. Do not conflate them.

The current client pins `jev-1.13.0`, checks requested and returned identity,
and enforces context and response limits plus deadline, concurrency, token and
request budgets. Reuse its retries and abstentions. Don't retry abstentions
until accepted or quietly switch models.

Deterministic processing makes decisions reproducible, not necessarily true.
Persist exact inputs, rubric/version, model identities and validated raw answers.
Publish deterministically from those records without relying on a fresh remote
request for bit-identical output. Confidence never upgrades INFERRED or AMBIGUOUS
evidence to EXTRACTED.

## Admission: high precision by default

Durable admission normally requires all of the following:

1. **Grounded:** the supporting records exist and actually address the claim.
2. **Atomic:** one proposition with understandable conditions and exceptions.
3. **Scoped:** where and when it applies are known.
4. **Useful:** it changes a future decision or avoids consequential repeated work.
5. **Novel:** it adds information beyond existing knowledge or readily available
   documentation; a precise applicability link may itself provide that value.
6. **Durable:** it is more than transient progress or current machine status.
7. **Qualified:** uncertainty and contradictory evidence are retained.

These are gates, not one weighted score: high usefulness cannot compensate for
unsupported evidence. Do not reuse the existing code-relation thresholds as an
intelligence-admission policy. Calibrate the new policy on labelled examples.

The outcome vocabulary should distinguish:

- admit;
- merge with existing intelligence;
- supersede;
- record a contradiction;
- retain as session-local working state;
- abstain pending evidence;
- reject as noise.

Rejected/abstained candidates keep a compact decision fingerprint and reason
for reproducibility and cache reuse, not another permanent copy of the paragraph.
Keep source custody and its retention policy independent of candidate
disposition. Initial migration never deletes originals just because a candidate
was rejected.

## What an admitted item contains

- Source-grounded proposition or decision; generated display text, if any,
  explicitly marked as derived.
- Kind, applicability, exceptions, owner/visibility scope.
- Supporting and contradicting evidence references.
- Repository/revision/environment context and observation time.
- Lifecycle: current, disputed, superseded, or needs revalidation.
- Related repository entities and other intelligence items.
- Admission decision, rubric and model provenance.

Lifecycle is independent of evidence class and confidence. Record the user's
policy as a choice, not a universal engineering recommendation. New evidence can
supersede an item without erasing its old decision or rationale. A dependency
change flags applicability for revalidation; it doesn't make the historical
claim false by itself.

For example, distinguish:

- A revision-scoped observation that a particular shell hook lacks session
  identity, with a source/API reference.
- A reusable design finding that session-global environment replacement can
  race concurrent commands, supported by a reproduction and the tested fix.
- An operator preference for automatic direnv approval, scoped to that
  operator's trusted project policy.

## Extract at meaningful boundaries

Useful triggers include newly ingested records, settled investigations, user
corrections, consequential code changes and their verification, compaction,
pruning, and evidence that affects an existing item.

Extract before compaction removes material the model can see. Use original
records, including counterevidence. A generated checkpoint can point back to
sources, but isn't enough on its own.

"Every opportunity" does not mean a request per streamed token. Deduplicate
unchanged source ranges, batch bounded candidates, and process settled evidence
asynchronously. Use existing worker leases, per-attempt accounting and guarded
publication. Failed batches must leave the last good knowledge build active.

## Storage and privacy

Large transcripts, outputs, attachments and checkpoints belong in a private
content-addressed artifact archive. PostgreSQL holds identities, provenance,
applicability, decisions and relationships; it should not become a dump of
hundreds of gigabytes of logs. Full digests identify archived content and
transform inputs; preserve native runtime IDs as namespaced aliases.

Require explicit user/workspace visibility scope before publishing session
knowledge. Repository membership alone doesn't authorize sharing private
conversation content. Preserve that boundary across repositories. Keep
credentials out of prompts, logs, summaries and published evidence. Don't retain
reusable items whose use requires disclosing a credential value.

Do not revive a second authoritative backend for this feature. Reuse the
PostgreSQL backend and the existing publication contracts, with explicit schema
changes for session provenance and scoped knowledge.

Superseded in 0.2.0: this section previously named TypeDB as the backend to
reuse. TypeDB provided only keyed upserts, joins, a compare-and-swap pointer and
one-hop reads while costing 13–14 G RSS plus 23–25 G swap, about 1 TB written
per 43 h and REX1 read failures, so the operator replaced it with PostgreSQL +
pgvector. Statement embeddings now rank knowledge retrieval (MCP
current-snapshot `intelligence_context`, `compaction context`) alongside word
overlap; they are derived data and never admit, evidence or publish knowledge.

## Useful during an LLM session

Extend the existing read-only CLI/MCP query path with two capabilities:

1. **Task context:** accept repository/revision, relevant paths and task intent;
   return a small token-budgeted set of applicable items with citations,
   exceptions, conflicts and freshness. Returning nothing is preferable to
   injecting weakly related advice.
2. **Evidence drill-down:** show source records, contradictions, supersession
   history and the exact code snapshot behind an item.

These capabilities are proposals, not current tool names. Don't inject the
whole store into every prompt. Retrieved content is historical evidence and has
no authority to override current user instructions or execution permissions.
Retrieval never triggers inference, ingestion or writes; operators explicitly
run those.

## How to judge quality

Label examples for support, duplication, contradiction and applicability,
including negative and uncertain cases. Hold some out when tuning admission
thresholds. Measure unsupported and duplicate admissions, stale recommendations
and whether retrieval improves continuation. Track important rejections so
high precision doesn't hide systematic loss of valuable knowledge. Item count
and graph size aren't success metrics.

Publish only when evidence resolution, schemas, visibility and the selected
materialization policy validate. Keep enough provenance to explain admission,
rejection and later correction without another model call.
