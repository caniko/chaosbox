# Selective session intelligence

Chaosbox extracts bounded proposals from normalized session JSONL, assesses
them with its pinned Jev client, and serves sparse, cited knowledge through the
CLI and MCP. This first session-intelligence slice neither replaces the session
archive nor migrates it automatically.

## Contracts

- Source: one normalized OpenCode JSON object per line, with native `id` and
  `type`. User text, assistant text and textual tool results are eligible.
  Compaction, synthetic and system records are excluded as independent sources.
- Candidate wording is a verbatim source line, with native source/session/message
  lineage, JSON pointer, line number, source timestamp where present, complete
  input digest and adjacent context. A bounded window of neighboring records
  retains tool names, command/input, structured status and exit code. Prefixes
  are marked partial; missing metadata never implies success. No generated
  labels or invented evidence.
- `--scope` is operator-selected visibility. `--repo` declares associations;
  neither associations nor scope are guessed or broadened by Jev.
- Semantic interpretation is always `INFERRED`, even when the underlying quote
  is extracted. Admission is not proof of current repository state. Repeated
  summaries and duplicate occurrences are never independent corroboration.
- Input JSONL should come from the privacy-reviewed, normalized migration
  artifact. This command is not a credential scanner or a raw database importer.

## Admission

Seven independent questions classify support, atomicity, applicability,
durability, kind, reusable value, and relation to at most four same-scope,
same-repository neighboring items. Jev receives bounded source context and
closed answer vocabularies, not an entire archive or executable instructions.

Current policy `session-intelligence-v6` requires support/atomicity/scope Noul
values >= 0.95 and durability/usefulness >= 0.9. Consolidation requires
both confidence and chosen probability >= 0.9. The receipt keeps uncertainty
between meaningful kinds separate from uncertainty about usefulness; the kinds'
combined probability must be >= 0.9. Utility cannot compensate for weak support.
These conservative initial thresholds are **not a measured accuracy claim**.
Calibrate against labelled examples before changing them. Rejection and
abstention are successful outcomes, not retry opportunities.
A gate value <= 0.1 or confidently classified noise is a rejection; the region
below automatic admission but above decisive negative evidence is an abstention.
Abstained candidates remain recoverable, require no mandatory human queue, and
do not block lossless session transfer. Neither outcome deletes source history.

Consolidation admits a new item, merges duplicate occurrences, retains both
sides of a contradiction, or supersedes an older item. Supersession requires
explicit user evidence with a later known source timestamp; missing chronology
abstains. Source snapshots remain recoverable and are never deleted here.

## Operator workflow

From an existing normalized migration transcript:

```sh
chaosbox intelligence extract transcript.ndjson \
  --source opencode --session ses_example --scope private:can \
  --repo canix --max-candidates 100 --output candidates-0.json

chaosbox intelligence assess candidates-0.json --live-jev \
  --source-jsonl transcript.ndjson \
  --max-requests 100 --max-input-tokens 1000000 --output knowledge-0.json

chaosbox intelligence context knowledge-0.json --scope private:can \
  --repo canix 'direnv approval' --limit 5 --max-chars 12000

chaosbox intelligence evidence knowledge-0.json --scope private:can \
  --repo canix intel:IDENTIFIER
```

Assessment uses the existing `CHAOSBOX_JEV_API_KEY_FILE` or explicitly configured
operator `TYPESAFE_API_KEY`. Query commands and MCP never load model credentials.
For evidence lookup, use the returned id; the placeholder is not a stored record.

Candidate schema v2 records source/session/repository identity explicitly.
Assessment requires the original `--source-jsonl` and regenerates the candidate
window before any inference: altered quotes, pointers, context, execution
metadata or snapshot identities fail closed. Re-extract legacy catalogs.

Extraction reports `has_more` and `next_offset`. Continue the same immutable
input with `--skip-candidates OFFSET`, then assess using `--previous
knowledge-0.json --output knowledge-1.json`. Coverage is stored in the bundle.
Oversize contexts and derived records are accounted separately from candidates;
lexical proposal generation is not exhaustive semantic recall. Inputs over
32 MiB are refused, never silently truncated: use explicit source shards.

Bundle manifests are immutable private JSON artifacts (0600). They reference
full, content-addressed receipts written once in the sibling `intelligence-receipts/`
directory (0700), rather than copying them. Consumers load only receipts needed
for stored knowledge; operators can reload the full index for reassessment.
Move the receipt directory with its manifests. Both writer and reader enforce the
32 MiB per-file ceiling.
Publication refuses existing output; it never overwrites a last-good artifact.
The cache stores successful, validated Jev responses in the output's `.decisions/`
sibling (0700), keyed by complete candidate state, neighbor state, questions,
rubric and pinned model.
Rerunning an interrupted assessment with the same inputs replays cached responses
through validation. API failures do not produce accepted knowledge or replace a
previous bundle. Existing receipts prevent repeated assessment only when the
full current state/rubric/model cache identity matches. The item's own
materialization is excluded from comparison neighbors so it does not invalidate
its own cache; new relevant knowledge does. Source-local and consolidation
answers still share one bounded request, so a changed neighbor set currently
reassesses that request rather than claiming unverified source-local reuse.

Loading manifests and receipts adds a 64 MiB aggregate budget to the 32 MiB
per-file ceiling. Oversize artifacts fail explicitly; partition rather than
truncate them. Evidence windows record total/window/omitted source-record
counts and omitted text fields separately from excerpt truncation flags.

Reevaluation retains the original admission and evidence but marks the item
`withheld` when the current assessment rejects or abstains. Default retrieval
excludes withheld and superseded items. A later supported assessment can readmit
the same item without duplicating it. New contradictions do not resurrect a
withheld claim. Retrieval projects bounded citations and counts rather than an
ever-growing evidence array; it marks records requiring policy revalidation.

## Labelled quality gate

`fixtures/intelligence/labels.json` records sanitized positive/negative labels
separately from the source records sent to Jev. Run the explicit live test with
`CHAOSBOX_INTELLIGENCE_PILOT_OUT` set to a new report path and
`cargo test -p chaosbox --test intelligence labelled_live_intelligence_pilot --
--ignored --nocapture`. `CHAOSBOX_INTELLIGENCE_PILOT_SPLIT=calibration` selects
the calibration subset. A failed gate is a failed gate; never count the normal
suite's intentional live-test skip as a model-quality pass.

The initial multi-record policy rejected all four labelled positives and all
four negatives. After clarifying normative-policy evidence, separating binary
usefulness from overlapping category labels, and projecting simpler model state,
v5 still missed both calibration positives. V6 changes their recorded-score
classification from rejection to abstention; it does not lower the automatic
admission thresholds or claim a new live calibration pass. Corpus-wide semantic
promotion remains blocked pending measured calibration, but lossless history
transfer proceeds independently. No source data is deleted for uncertainty.
Protocol/integrity tests passing does not establish semantic quality.

## Other sessions: read-only MCP

[Same-user peer sync](PEER_SYNC.md) adds signed intelligence/evidence exchange,
Jev-assisted reconciliation, and a live TypeDB-backed reader. Use `chaosbox mcp
--intelligence-current /private/path/sync` to refresh the current local snapshot
between requests; the artifact mode below pins its bundle at startup.

```sh
chaosbox mcp --intelligence /private/path/knowledge-1.json
```

At startup, this pins one validated bundle and adds two read-only server tools:

- `intelligence_context`: explicit repository and task terms, bounded item and
  character budgets. Returns historical context, not fresh instructions. An empty
  result does not prove that no relevant knowledge exists.
- `intelligence_evidence`: source occurrences, contradictions/supersession and
  bounded typed receipts for a record. Omitted evidence/receipt counts are explicit.

Tools cannot change bundle path/scope, invoke Jev, write data or access the
session archive. The configured bundle bounds visibility; scope labels provide
neither authentication nor a multi-tenant ACL. Expose bundles only to callers
authorized for their scope. Restart the consumer to select a newer artifact.
Repository graph tools are unchanged.

### OpenCode adapter (first slice)

`plugins/chaosbox-intelligence/` is an OpenCode V2 plugin over the same
read-only surface: selective injection of bounded context packets into the
agent loop, evidence drill-down, structured challenges that stay proposals
pending explicit human resolution, and opt-in shadow-mode capture for later
explicit `extract|assess` runs. It never calls Jev, holds no model
credentials, and never writes knowledge. See its README for setup and the
pilot metrics to collect before widening automatic injection.

## Persistence and continuation

This code reuses core provenance, typed Jev answers/budgets and CLI/MCP
consumers. The bundle is a staging/export artifact, **not an authoritative
database backend**. This workflow does not automatically insert private session
data into the shared repository graph.

The [source-custodied coordinator](SESSION_COMPACTION.md) keeps native records
under durable custody and publishes private knowledge through predecessor-guarded
TypeDB transactions. [Peer sync](PEER_SYNC.md) adds an owner/scope-isolated signed
event ledger and live immutable read snapshots, including Jev reconciliation
receipts. Continuation uses pinned Jev selections over supplied records and
deterministic Rust templates with exact source references. Its rendered view
never supplies new evidence for itself.

Tests cover strict rejection, abstention, malformed answers, model substitution,
duplicate consolidation, preserved contradictions, source/chronology rules,
scope isolation, bounded retrieval, immutable private writes and a real MCP
process that serves intelligence with deliberately unusable DB/model credentials.
Live Jev quality calibration is distinct from these protocol/invariant tests.
