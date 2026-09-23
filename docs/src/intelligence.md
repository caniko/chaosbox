# Selective intelligence

Status: proposed direction. This extends Chaosbox's existing evidence pipeline;
it does not describe capabilities already shipped. The first application is
[preserving session knowledge during migration](session-migration.md).

## Objective

Extract intelligence whenever meaningful new evidence becomes available, and
make it accessible to other LLM sessions. Intelligence is scarce: the goal is
better future decisions, not a larger graph or a catalogue of every utterance.

An intelligence item is a supported, scoped proposition or decision that reduces
uncertainty, prevents repeated mistakes, or preserves a consequential insight.
It must carry enough provenance and applicability information to be used safely
outside the session that discovered it.

Three different artifacts must not be confused:

| Artifact | Purpose | Authority |
| --- | --- | --- |
| Source archive | Recover transcripts, repository snapshots, outputs and attachments | What was actually captured |
| Admitted intelligence | Reusable findings, decisions, constraints and relationships | Supporting evidence plus explicit admission policy |
| Continuation checkpoint | Help an agent resume a particular session | A generated view of a specified source range |

A checkpoint is not new evidence for its own claims. Repeating an assertion in
several summaries does not provide independent corroboration. Conservatively
preserving sources during migration is compatible with admitting very little
into the reusable intelligence store.

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
adjudicates supplied candidates using the existing typed API. A generative model
may propose a formulation or continuation checkpoint, but its prose is neither
a source excerpt nor a verified fact.

Keep the existing code-extraction contract: source labels and evidence text are
copied or deterministically constructed. Generated explanations belong to a
separate derived-artifact category. Do not loosen that contract to make session
summaries look like extracted code facts.

## Evidence and candidate construction

Repository evidence is addressed by repository, snapshot/revision and source
span. Session evidence needs a source snapshot, native session ID, message/part
ID and an exact excerpt or structured result field. Add an explicit session
source-reference type; do not disguise messages as repository files.

Sessions may span several repositories. Their working directory is an observed
location, not proof of sole ownership. Links to files, symbols and components
must preserve repository identity and the applicable revision or artifact hash.
Never merge entities merely because labels or paths look alike. Respect the
existing graph's build-membership guards when adding cross-repository links.

Cheap deterministic extraction comes first: source spans, explicit user
decisions, command/result metadata, file/revision references and relationships
already known to the repository graph. Language interpretation may produce
additional candidates, but each needs evidence anchors that resolve before Jev
assessment. Missing or oversized context causes an explicit abstention or a
bounded context-expansion request, not silent truncation.

An assistant saying "the build passed" supports the observation that the
assistant made that claim. It does not establish a successful build. A relevant
tool result, its exit status, exact command, and tested source identity are
needed for the stronger finding. A pipeline ending in a successful output
filter is not automatically evidence that the build itself succeeded.

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

These are proposed rubrics, not a claim that the current code-relation catalog
already implements them. A compound candidate must be split into independently
grounded propositions and reassessed. Choice options contain descriptions, not
bare opaque IDs. Compare against a bounded relevant set of existing items;
never compare every historical item with every other item.

Questions within a Jev request are independent. Assessments that depend on an
earlier answer belong in a later stage. Noul supplies a probability; Choice and
Score have their own distribution/confidence fields. Do not conflate them.

The current client pins `jev-1.13.0`, checks requested and returned identity,
enforces context and response limits, and has deadline, concurrency, token and
request budgets. Use its existing retry and abstention behavior. Do not retry an
abstention until it becomes an acceptance or switch to another model silently.

Deterministic processing makes a decision reproducible; it does not make the
interpretation true. Persist exact inputs, rubric/version, model identities and
validated raw answers. Publication from those records must be deterministic.
The system must not depend on a fresh remote request returning bit-identical
output. Confidence never upgrades INFERRED or AMBIGUOUS evidence to EXTRACTED.

## Admission: high precision by default

An item normally earns durable admission only when all of these hold:

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

Rejected/abstained candidates retain a compact decision fingerprint and reason
for reproducibility and cache reuse, not another permanent copy of the entire
paragraph. Source custody and its retention policy remain separate. The initial
migration never deletes originals merely because a candidate was rejected.

## What an admitted item contains

- Source-grounded proposition or decision; generated display text, if any,
  explicitly marked as derived.
- Kind, applicability, exceptions, owner/visibility scope.
- Supporting and contradicting evidence references.
- Repository/revision/environment context and observation time.
- Lifecycle: current, disputed, superseded, or needs revalidation.
- Related repository entities and other intelligence items.
- Admission decision, rubric and model provenance.

Lifecycle is independent of evidence class and confidence. A user policy choice
is recorded as a policy choice, not a universal engineering recommendation.
New evidence may supersede an item without erasing the old decision and its
rationale. A changed dependency marks applicability for revalidation; it does
not automatically make the historical claim false.

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

Extract before compaction removes material from the model-facing window. Use
the original records, including the counterevidence, not only a generated
checkpoint. Treat a checkpoint as a navigation aid back to sources.

"Every opportunity" does not mean a request per streamed token. Deduplicate
unchanged source ranges, batch bounded candidates, and process settled evidence
asynchronously. Use existing worker leases, per-attempt accounting and guarded
publication. Failed batches must leave the last good knowledge build active.

## Storage and privacy

Large transcripts, outputs, attachments and checkpoints belong in a private
content-addressed artifact archive. TypeDB holds identities, provenance,
applicability, decisions and relationships; it should not become a dump of
hundreds of gigabytes of logs. Full digests identify archived content and
transform inputs; preserve native runtime IDs as namespaced aliases.

Session-derived knowledge must have an explicit user/workspace visibility scope
before publication. Repository membership alone does not authorize disclosure
of private conversation content. Cross-repository retrieval must preserve that
boundary. Credentials remain out of prompts, logs, summaries and published
evidence. Do not retain a reusable intelligence item whose usefulness depends on
revealing a credential value.

Do not revive a second authoritative backend for this feature. Reuse TypeDB and
the existing publication contracts, with explicit schema changes for session
provenance and scoped knowledge.

## Useful during an LLM session

Extend the existing read-only CLI/MCP query path with two capabilities:

1. **Task context:** accept repository/revision, relevant paths and task intent;
   return a small token-budgeted set of applicable items with citations,
   exceptions, conflicts and freshness. Returning nothing is preferable to
   injecting weakly related advice.
2. **Evidence drill-down:** show source records, contradictions, supersession
   history and the exact code snapshot behind an item.

These are proposed capabilities, not current tool names. Do not inject the whole
knowledge store into every prompt. Retrieved content is historical evidence,
not authority to override current user instructions or execution permissions.
Consumers never perform inference, ingestion or writes as a side effect of
retrieval; those remain explicit operator operations.

## How to judge quality

Use labelled support, duplication, contradiction and applicability examples,
including negative and uncertain cases. Hold some examples out when tuning
admission thresholds. Measure unsupported admissions, duplicate admissions,
stale recommendations and whether the retrieved context improves continuation.
Track important rejected findings as well: high precision must not hide
systematic loss of valuable knowledge. Item count and graph size are not success
metrics.

Publish only when evidence resolution, schemas, visibility and the selected
materialization policy validate. Keep enough provenance to explain admission,
rejection and later correction without another model call.
