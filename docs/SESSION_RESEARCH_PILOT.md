# Session-derived feature research pilot

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

This is intentionally **pilot coverage**: child sessions, divergent variants,
other source databases, image contents, tool-output text and post-freeze deltas
are not analyzed. Roots with up to 500 messages are eligible; a selected thread
above 120,000 exported text characters is refused rather than silently
truncated. Normalized root title and first user request identify repeated fork
lineages; only one root from a lineage enters a pilot. The cwd is a sampling
hint, not an inferred repo identity.
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

### Isolate model sessions

OpenCode's `--standalone` isolates the server, **not automatically its DB**.
This host's Nix `opencode` launcher explicitly unsets `OPENCODE_DB`. Point
`SESSION_RESEARCH_OPENCODE_BIN` at the underlying OpenCode binary, not that
launcher, and carry over the host's provider configuration and XDG environment.
The runner checks that `debug paths db` resolves to `$WORK/model.sqlite` before
each call. A launcher that clears the override is rejected before inference.
The standalone server may not load an agent from the scratch directory even if
`debug config` lists that file. The runner explicitly pins a private merge of
the host's `OPENCODE_CONFIG` and a `session-research` primary agent denying
**all** tool actions (including MCP tools). A previous exact-match pilot config
is preserved as `opencode.previous.json` before upgrade; unknown config changes
are refused. Model event streams with any tool or error event are rejected.

An empty isolated DB may not have the required provider credentials and model
catalog. `seed-auth` initializes it and copies **only the active Muse/OpenAI
credential rows and model catalog** from a read-only credential DB. The new
private DB contains credentials and must be treated accordingly. It never
alters the source credential DB or the frozen session snapshot.

```sh
export SESSION_RESEARCH_OPENCODE_BIN=/absolute/path/to/underlying/opencode
python3 scripts/session_research.py seed-auth --work "$WORK" \
  --credential-db /path/to/existing/opencode.db
python3 scripts/session_research.py extract --work "$WORK" --privacy-reviewed
python3 scripts/session_research.py collect --work "$WORK"
python3 scripts/session_research.py briefs --work "$WORK"
python3 scripts/session_research.py metrics --work "$WORK"
```

Spark 1.3 Contributor xhigh produces a brief and at most 12 typed findings per
chunk. Every finding must cite an exact substring in a native source part. A
short user reply (for example, `commit`) is citable only as the entire exported
text record; shorter fragments of a longer record do not satisfy the gate. A
failed call retains private events and can be retried with `--retry`; completed
chunks are reused after validation. Calls refuse payloads above 200 KB;
`collect` refuses missing chunks.
`briefs.json` keeps each Spark chunk's prose in source order under its session;
the briefs also include a timestamped, speaker-labelled findings timeline.
They are generated views, never extra corroboration.

To generate reports, give Astra Max bounded **tracked HEAD** excerpts for
candidate current-capability checks. Use explicit line ranges for long files.
The synthesis payload carries independent source windows around cited quotes
rather than repeating whole 2,500-character parts; the final validators still
check every quote against the complete exported part and pinned SQLite source.
Present/partial/rejected status requires a matching repository excerpt; an
absence claim cannot be established by a bounded context sample and remains
`unverified`.

```sh
python3 scripts/session_research.py synthesize --work "$WORK" --privacy-reviewed \
  --repo-root canix=/path/to/canix --repo-file 'canix=.envrc#L1-L25' \
  --repo-root SynDB=/path/to/SynDB --repo-file 'SynDB=README.md#L1-L80' \
  --repo-root pink-raven=/path/to/pink-raven \
  --repo-file 'pink-raven=src/app/gallery.rs#L95-L195'
python3 scripts/session_research.py render --work "$WORK"
python3 scripts/session_research.py audit --work "$WORK"
```

The final render rechecks every source slice against the original pinned part
and validates both session and repository citations. Generated Markdown and
machine-readable reports live in `$WORK/reports/`. Portfolio opportunities
require support from at least two sampled repositories. Model calls are
tool-denied and run from the private work directory. Raw prompts, output events,
and the isolated model DB stay there; do not commit the work directory.

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

Tests: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -p test_session_research.py`.
