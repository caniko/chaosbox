# Session-derived feature research pilot

## Candidate review and reproduction receipts

`python3 scripts/session_research.py ledger --work WORK --out NEW_FILE` creates
a private, write-once inventory of every extracted feature, unfinished task and
friction signal. It retains complete exported conversations alongside stable
candidate IDs. The top-eight report limit does not apply to this inventory.
Historical cwd remains `sourceCwdHint`; ownership starts unresolved.

An optional `--decisions FILE` supplies a JSON object keyed by candidate ID.
Each decision requires a rationale. Resolving `targetRepository` also requires
`targetComponent` and exact `ownershipEvidence` citations. Targets can belong to
repositories outside the sampled three. Intent, implementation, deployment,
action and disposition are separate fields; duplicate links must be acyclic.
These are review annotations, not proof that a system was deployed. Version 1
keeps deployment unknown pending an independent deployment-verification path.

New synthesis requires `--decisions FILE`. Only candidates marked `selected`
with resolved ownership and requested/accepted intent enter repository views.
Every opportunity carries its candidate IDs and the reviewed action,
implementation and deployment fields; validators reject changed statuses or
invented IDs. Ownership can cross historical cwd boundaries. Out-of-cohort
owners and all unselected candidates remain in `candidate-ledger.json`.
Synthesis retries require the same ledger; changed review decisions belong in
a new work directory. Review annotations guide synthesis but do not establish
that cited text semantically proves a claim: that remains the reviewer's job.

A new synthesis may use `confirmed-gap` only with an independently supplied
`WORK/reproductions.json` entry and the corresponding `reproductionRef` in the
opportunity. Each receipt records `sourceRevision` (the same full Git revision
as the code evidence), `sourceState: "clean"`, `command` (argument array),
`expected`, `observed`, `outputSha256`, and `result: "reproduced-defect"`.
Receipts are operator-reviewed evidence; the model cannot create them. Dirty
checkout results cannot establish failure of a pinned clean revision. Code
inspection without a receipt remains partial or unverified. Existing historical
reports remain evidence of their original run, rather than being rewritten to
meet the stronger validation contract.

`scripts/session_research.py` is a disposable research workflow over the frozen
OpenCode v1 archive. Its output is a set of **reviewable feature proposals**, not
a session migration or a write to the Chaosbox knowledge graph.

The pilot uses the pinned `manifest.json` and `reconciliation.json`: canonical
`primary` root sessions whose working directory ends in `canix`, `SynDB`, or
`pink-raven`. It reads SQLite with `mode=ro&immutable=1`, verifies the full
snapshot SHA-256 and size, and refuses a nonempty snapshot WAL. Each exported
text slice retains its native session, message and part IDs plus character
offsets, original speaker, and message/part creation times. Derived compactions,
synthetic records and tool payloads are excluded
from independent evidence. Assistant assertions are labelled as such; they do
not establish the current repository state.
Extraction and synthesis recheck every exported slice against its pinned native
part before invoking a model.

This is intentionally **pilot coverage**: child sessions, divergent variants,
other source databases, image contents, tool-output text and post-freeze deltas
are not analyzed. Roots with up to 500 messages are eligible; a selected thread
above 120,000 exported text characters is refused rather than silently
truncated. Normalized root title and first user request identify repeated fork
lineages; only one root from a lineage enters a pilot. The cwd is a sampling
hint, not an inferred repo identity.
Metrics distinguish `replayedRoots` (same title/request family among primary
roots) from `reconciliationVariants` (recorded alternate snapshot copies).
No frequency claim should be made from the pilot alone.

## Run a disposable pilot

Create a new private work directory outside the repository; it must not exist
before `prepare`. `--session` selects operator-curated canonical roots; without
it, `--per-repo` draws a deterministic, title-stratified sample. Review the
resulting `plan.json` for unsuitable sessions before exporting. If the sample
is poor, choose a **new** work directory; files are never overwritten.

```sh
ARCHIVE=/data/nvme0/can/ProjectState/opencode-migration/2026-09-22-v2
WORK=/data/scratch/tmp/opencode/feature-research-pilot
python3 scripts/session_research.py prepare --archive "$ARCHIVE" --work "$WORK" --per-repo 12
python3 scripts/session_research.py export --work "$WORK"
```

`bundles.json` contains original user/assistant text and must remain private.
Review it before transmitting it to models. Extraction blocks recognizable
credential patterns, but the local check is not a comprehensive privacy review.
`--privacy-reviewed` is the operator's explicit attestation for that batch.

### Typed Jev decisions

All three inference stages use pinned Typesafe `jev-1.13.0` Choice questions:
source-span classification, reviewed-candidate priority/status, and shared-work
comparison. `scripts/session_research_jev.py` builds finite choices and renders
the results. `chaosbox jev evaluate` reuses the existing Rust HTTP client,
credentials, deadlines and context checks. It sends at most 16 independent
questions in one request, with 2–255 options per question. The model never
generates quotations, candidate IDs, new plans or report prose.

The contract follows the official [API](https://docs.typesafe.ai/api) and
[models](https://docs.typesafe.ai/models) documentation, checked 2026-09-30.
Question keys are not sent to the model: source/candidate selectors are included
in the instructions and option descriptions. Dependent decisions run in separate
stages: select exact repository evidence first, then assess status against it.

Build the current CLI or select an installed binary that supports `jev evaluate`.
Credentials come from `CHAOSBOX_JEV_API_KEY_FILE`, falling back to the explicit
operator `TYPESAFE_API_KEY`. No OpenCode server, provider configuration, model
session database or copied provider credentials is needed.

```sh
cargo build -p chaosbox
export SESSION_RESEARCH_CHAOSBOX_BIN="$PWD/target/debug/chaosbox"
# Supply CHAOSBOX_JEV_API_KEY_FILE or TYPESAFE_API_KEY through the operator environment.
python3 scripts/session_research.py extract --work "$WORK" --privacy-reviewed
python3 scripts/session_research.py collect --work "$WORK"
python3 scripts/session_research.py briefs --work "$WORK"
python3 scripts/session_research.py metrics --work "$WORK"
```

Extraction enumerates exact sentence/line spans, then Jev chooses a signal kind,
`none`, or `uncertain`. A short user reply (for example, `commit`) is citable only
as the entire exported record. Shorter fragments of longer records are excluded
by the citation gate. Ambiguous or multi-topic spans should abstain; this finite
candidate strategy can miss requests and requires recall evaluation. At most 12
signals enter each chunk view; every evaluated span retains its answer and
admitted/rejected/abstained/omitted disposition. `briefs.json` is a deterministic
view of selected original statements with a speaker-labelled timeline.

Choice answers require the pinned returned model, exact answer/option sets,
finite probabilities summing to one (within 0.01), and a selected maximum.
Materialization requires selected probability ≥0.8 and confidence ≥0.6.
These are versioned conservative thresholds, **not calibrated accuracy claims**.
Uncertain priority stays `unverified`; it does not discard the reviewed candidate.

Each request is content-addressed under `WORK/jev/`, including source-plan hash,
state, questions, model, rubric and thresholds. Files are private and create-only.
A process lock serializes calls; an intent is persisted before launching the CLI.
Successes, including negatives and abstentions, are reused even with `--retry`.
Failures/interrupted attempts require explicit `--retry` and still consume the
work-wide request allowance. No automatic retries or general-purpose fallback run.

`extract` and `synthesize` share persistent budgets: `--max-requests` defaults to
100 and `--max-input-tokens` to 1,000,000. Each dispatch reserves serialized input
bytes plus overhead; accounting charges the larger of that reservation and
reported usage. Unknown timeout usage is covered conservatively by the reservation,
not treated as a zero-cost success. This is a spending guard, not an exact billing
estimate. `metrics` separates reservations, reported tokens and failed/interrupted
attempts. Increase the explicit total allowance when resuming a larger reviewed
batch. The CLI enforces Jev's 64k total / 32k state-plus-longest context ceilings;
oversized inputs fail rather than silently losing context.

To generate reports, supply bounded **tracked HEAD** excerpts for
candidate current-capability checks. Use explicit line ranges for long files
and at most 254 excerpt lines for one evidence-selection Choice.
The synthesis payload carries the full exported conversations associated with
selected candidates and their ownership citations. This retains assistant
proposals alongside brief user approvals and later reversals. Validators check
quotes against exported parts and the pinned SQLite source. An oversized
payload is refused rather than dropping decisions. Choose a smaller reviewed
cohort for a new work directory. The complete extracted candidate inventory
remains in `candidate-ledger.json`; the top-eight report is a view of that
inventory, not a transcript census. Earlier export and extraction limits apply.
Present/partial/rejected/confirmed-gap status requires a matching repository excerpt;
confirmed-gap also requires the separate reproduction receipt described above; an
absence claim cannot be established by a bounded context sample and remains
`unverified`.

```sh
python3 scripts/session_research.py ledger --work "$WORK" --out "$WORK/review-inventory.json"
# Review the inventory and create $WORK/decisions.json keyed by candidate ID.
python3 scripts/session_research.py synthesize --work "$WORK" --privacy-reviewed \
  --decisions "$WORK/decisions.json" \
  --repo-root canix=/path/to/canix --repo-file 'canix=.envrc#L1-L25' \
  --repo-root SynDB=/path/to/SynDB --repo-file 'SynDB=README.md#L1-L80' \
  --repo-root pink-raven=/path/to/pink-raven \
  --repo-file 'pink-raven=crates/pink-raven-dioxus/src/lib_parts/catalog_entities.rs#L1453-L1497'
python3 scripts/session_research.py render --work "$WORK"
python3 scripts/session_research.py audit --work "$WORK"
```

The final render rechecks every source slice against the original pinned part
and validates both session and repository citations. Generated Markdown and
machine-readable reports live in `$WORK/reports/`. Portfolio opportunities
require citations associated with at least two reviewed target repositories.
Pairs from overlapping historical lineages are excluded before inference.
Jev chooses commonality and ownership only from supplied candidates; report
proposals and first steps use fixed action templates. Report coverage includes
omitted candidate/pair IDs. Source text and all decision receipts stay in the
private work directory; do not commit it.

### Migrating an earlier pilot

Start a new work directory for Jev extraction/synthesis. `seed-auth`,
`SESSION_RESEARCH_OPENCODE_BIN` and the generative model runner have been removed.
Earlier artifacts remain readable through `collect`, `briefs`, `render`, `audit`
and legacy usage metrics; they are historical results, not Jev receipts. New
Jev synthesis may consume reviewed legacy findings, but cannot overwrite earlier
reports or reinterpret their model events as typed answers. Candidate IDs can
change when new extraction uses verbatim statements instead of generated claims;
review a new ledger rather than reusing old decisions by position.

For source drill-down, copy a report citation verbatim into
`python3 scripts/session_research.py inspect --work "$WORK" --ref 'primary/ses_.../msg_.../prt_...@0:2500'`.
It rechecks the pinned original parts and prints a bounded neighborhood of
source text; treat that terminal output as private.

`audit` writes `reports/audit.json`, flagging assistant-only recommendations,
repeat-lineage citations, obvious pasted tool transcripts and skill templates
inside user-role text, and distinct historical root counts. The flags are conservative;
an unflagged user-role quote may still contain an embedded template. Review each
flag against its original context before considering the reports for tasks.

## Pilot evaluation

Inspect a sample of findings against their complete conversations, and check
each recommended capability against current source and tests before opening a
feature task. Record whether an item is new, already delivered, wrong-scope or
unsupported. Measure missed requests, wrong outcomes, cross-repo attribution,
failed calls, and citation validity before increasing the sample. Do not treat
multiple assistant repetitions as independent users or evidence.

Tests: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests` and
`cargo test -p chaosbox --bin chaosbox --test jev_cli`.

A 2026-09-30 synthetic-only live smoke exercised extraction, report decisions and
portfolio comparison through the actual Rust CLI: five Jev requests, 4,608 reported
input tokens and 396 output tokens. Each of two invented feature requests yielded
one signal; weak code-status/ownership evidence remained unverified or unselected.
Replaying the portfolio added zero requests. This validates the transport and
receipt path, not recall, calibration, cost savings against a baseline, or improved
agent outcomes on real sessions.

Local verification artifacts: the synthetic smoke is
`/data/scratch/tmp/opencode/chaosbox-jev-synthetic-75tdtb3t/smoke.json`; the
workspace-test log is `/data/scratch/tmp/opencode/chaosbox-jev-workspace-tests.log`
(250 passed, three opt-in tests ignored). All 53 Python tests, strict Clippy and
mdBook also passed. Production-package realization remains blocked by another
session's Canix evaluation lock, as recorded in `WORKSPACE_IMPACT.md`.
