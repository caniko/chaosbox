# Selective session intelligence

Chaosbox can now extract bounded proposals from normalized session JSONL,
assess them with its existing pinned Jev client, and serve sparse, cited
knowledge through the CLI and MCP. This is the first session-intelligence
slice, not a replacement for the session archive or an automatic migration.

## Contracts

- Source: one normalized OpenCode JSON object per line, with native `id` and
  `type`. User text, assistant text and textual tool results are eligible.
  Compaction, synthetic and system records are excluded as independent sources.
- Candidate wording is a verbatim source line, with native source/session/message
  lineage, JSON pointer, line number, source timestamp where present, complete
  input digest and adjacent context. A bounded window of neighboring records
  retains tool names, command/input, structured status and exit code. Prefixes
  are explicitly partial; missing metadata is never inferred as success.
  No generated labels or invented evidence.
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

Current policy `session-intelligence-v5` requires support/atomicity/scope Noul
values >= 0.95 and durability/usefulness >= 0.9. Consolidation requires
both confidence and chosen probability >= 0.9. Taxonomic uncertainty between
meaningful kinds is retained in the receipt rather than being mistaken for
uncertainty about usefulness; their combined probability must be >= 0.9.
Utility cannot compensate for
weak support. These are deliberately conservative initial thresholds, **not a
measured accuracy claim**; calibrate against labelled examples before changing
them. Rejection and abstention are successful outcomes, not retry opportunities.

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
Use the actual returned id for evidence lookup; the placeholder above is not a
valid stored record.

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

Bundle manifests are immutable private JSON artifacts (0600). Full receipts are
content-addressed, written once under the sibling `intelligence-receipts/`
directory (0700), and referenced by manifests instead of copied into each one.
Consumers load only receipts needed for the stored knowledge; operators can
reload the complete index for reassessment. Move the receipt directory with its
manifests. Both writer and reader enforce the 32 MiB per-file ceiling.
Publication fails if the
output exists; no last-good artifact is overwritten. Successful validated Jev
responses are cached under the output's `.decisions/` sibling (0700), keyed by
the complete candidate state, neighbor state, questions, rubric and pinned model.
Rerunning an interrupted assessment with the same inputs replays cached responses
through validation. API failures do not produce accepted knowledge or replace a
previous bundle. Existing receipts prevent repeated assessment of unchanged
source occurrences under the same rubric.

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
the current policy still misses both calibration positives. These changes did
not lower the numeric support/scope/atomicity thresholds. Corpus-wide semantic
promotion remains blocked pending measured calibration. No rejected source data
is deleted. Protocol/integrity tests passing does not establish semantic quality.

## Other sessions: read-only MCP

```sh
chaosbox mcp --intelligence /private/path/knowledge-1.json
```

This pins one validated bundle on startup and adds two tools to the existing
read-only server:

- `intelligence_context`: explicit repository and task terms, bounded item and
  character budgets. Returns historical context, not fresh instructions. An empty
  result does not prove that no relevant knowledge exists.
- `intelligence_evidence`: source occurrences, contradictions/supersession and
  bounded typed receipts for a record. Omitted evidence/receipt counts are explicit.

Tools cannot choose another bundle path/scope, invoke Jev, write data or access
the session archive. The configured bundle defines the visibility boundary;
scope labels are not authentication or a multi-tenant ACL. Only expose a bundle
to callers authorized to read its scope. Restart the consumer to select a newer
artifact. The existing repository graph tools are unchanged.

## Current boundaries and next migration slice

This code reuses core provenance, typed Jev answers/budgets and the CLI/MCP
consumer infrastructure. The bundle is a staging/export artifact, **not a new
authoritative database backend**. No private session data is inserted into the
shared TypeDB graph automatically.

Next: adopt the snapshot/reconciliation manifest, model visibility and session
source references in TypeDB, publish through the existing generation-guarded
transaction, and connect native session transfer/checkpoint generation. Spark
Contributor/xhigh compaction remains a separate generative operation: its
summary is a derived view, never new supporting evidence for itself.

Tests cover strict rejection, abstention, malformed answers, model substitution,
duplicate consolidation, preserved contradictions, source/chronology rules,
scope isolation, bounded retrieval, immutable private writes and a real MCP
process that serves intelligence with deliberately unusable DB/model credentials.
Live Jev quality calibration is distinct from these protocol/invariant tests.
