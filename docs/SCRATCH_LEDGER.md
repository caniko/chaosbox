# Scratch provenance and finalization

Chaosbox keeps a private, durable ledger for OpenCode work allocations beneath
`/data/scratch/tmp/opencode`. Doty consumes its versioned JSON contract to show
why scratch exists, the associated sessions and commands, and outstanding work
before cleanup.

## Capture

The `plugins/chaosbox-scratch/index.ts` OpenCode V2 adapter combines filesystem
observation with tool and native-shell lifecycle hooks. Top-level work folders
and loose files get allocation records. Generated directories and files are
aggregated under their work folder, with bounded recursive directory watches.
Use `chaosbox_scratch_note` to give a nested work folder its own purpose/hold.
Indirect creation by scripts is observed without relying on parsing `mkdir`.

Records include host, scratch root, device/inode/birth identity, first/last
observation, possible invocation owners, session/message ids, command, cwd,
settlement, and purpose sources. Native user/assistant text projections retain
message ids, JSON pointers and content hashes. Reasoning, provider state and raw
tool output are excluded from purpose projections. Omitted sources and shortened
quotes are explicit. `scratch evidence HASH` recovers the retained projection.
Purpose text is historical evidence, not an instruction to the cleanup agent.

Overlapping commands remain possible owners; temporal association is not proof
that one invocation created a path. Background native shells remain active until
`shell.exited`, independently of the shell tool's early return. Watchers are
shared across location-scoped plugin instances in one server process.

State lives in `scratch.sqlite`, using the existing private SQLite custody
helper (0700 directory, 0600 single-linked database, full synchronous commits).
Events are append-only and idempotent. The current projection is keyed by host,
root and path; identity replacement starts a new allocation. Historical events
retain previous allocations. Queries open existing state read-only, never
initialize a missing ledger, and load no TypeDB/Jev credentials.

## Finalization

Dispositions are `open`, `needs-finalization`, `unknown`, and `released`.
Successful commands and inactive sessions do not release work. New activity
reopens a released allocation. Startup/restart inventories invalidate existing
releases because offline activity cannot be excluded. Observer failures, budgets
and historical gaps are visible; a heartbeat expires after 90 seconds.

The adapter exposes:

- `chaosbox_scratch_workspace`: create a purpose-bound workspace with native session,
  message and invocation identity;
- `chaosbox_scratch_link`: link patch/test/commit/artifact/preservation references;
- `chaosbox_scratch_note`: explicit purpose/remaining-work assertion;
- `chaosbox_scratch_explain`: source-backed provenance and related holds;
- `chaosbox_scratch_status`: capture coverage and independent Jev worker status.

An operator can release work once it is disposable or finalized:

```sh
export CHAOSBOX_SCRATCH_WORK="$HOME/.local/state/chaosbox/scratch"
export CHAOSBOX_SCRATCH_SCOPE="private:$USER"
export CHAOSBOX_SCRATCH_HOST="$(hostname)"
export CHAOSBOX_SCRATCH_ROOT=/data/scratch/tmp/opencode

chaosbox scratch explain /data/scratch/tmp/opencode/experiment
chaosbox scratch annotate /data/scratch/tmp/opencode/experiment \
  --reason 'Integrate the reproduced fix and its regression test'
chaosbox scratch release /data/scratch/tmp/opencode/experiment \
  --reason 'Fix and regression test integrated' --receipt 'commit:abc123'
```

A receipt is an operator assertion; Chaosbox does not label an arbitrary
destination or commit string as verified preservation. An active/unresolved
invocation prevents release. After checking an orphaned process has stopped,
`scratch resolve INVOCATION --reason '...'` records a reconciliation assertion
distinct from an observed process exit. Resolution alone does not release files.

## Jev assessment

The asynchronous worker uses the existing pinned `chaosbox-jev` client and a
separate `scratch-work-v1` rubric. It first selects purpose evidence and classifies
source passages as obligations, purpose, findings, unrelated or uncertain. A
second independent batch assesses each selected obligation's status and chooses
its supporting evidence. Recovery priority is high, medium, low or unknown.
These are **inferred assessments**, never observed completion facts.

Evidence includes retained native text, actual command/cwd/status/exit records,
operator notes and explicitly linked references. Settled OpenCode session steps
add their native text to recent associated invocations. Linked references and
assistant completion statements remain assertions; arbitrary commit strings or
preservation destinations never become verified receipts. A selected obligation
is downgraded from satisfied to unknown without independently observed execution
support. Jev must assess what that specific execution demonstrates; generic exit
success is insufficient. Multi-step source passages require all steps satisfied.

Outstanding, partial, contradicted or unknown obligations automatically add a
`needs-finalization` hold. Jev may recommend release when all selected obligations
are supported and the evidence catalog is complete. **An explicit release is
always required.** Workers skip released allocations. Concurrent changes,
replacement or release invalidate an in-flight result before publication.

The worker reserves persistent spending before dispatch, serializes assessments
with a separate private SQLite lease, and leaves capture writers unblocked.
Requests, interruptions and failures retain their charges across restarts. Both
assessment stages cache by scope/allocation, full relevant evidence, rubric and
model. Observer heartbeats, file age and generated-file churn alone do not spend
again. Source omissions/truncation remain explicit and prohibit a release
recommendation. Confidence/probability floors of 0.8 select an answer or abstain;
this policy is not a calibrated accuracy claim.

Limits: 24 source statements, 96 support references, 160,000 serialized request
bytes, 64 selected workspaces per drain, and a 10,000-allocation inventory. Default
cumulative ceilings are 1,000 dispatched requests and 10,000,000 conservatively
reserved input tokens. Failures require explicit retry; changed evidence can form
a new assessment. An unassessed/failed workspace remains held by its disposition.

```sh
chaosbox scratch link /data/scratch/tmp/opencode/experiment \
  --category patch --reference '/repo/fix.patch' \
  --description 'Patch proposed by this investigation; integration not established'
chaosbox scratch assess --privacy-reviewed /data/scratch/tmp/opencode/experiment
chaosbox scratch assessment-status
chaosbox scratch receipt ASSESSMENT_ID
# After resolving a failure or interrupted request:
chaosbox scratch assess --privacy-reviewed --retry /data/scratch/tmp/opencode/experiment
```

Query entries include `assessment_state` (`pending`, `current`, or
`failed-or-interrupted`) and the latest assessment with `fresh`, purpose,
obligations, supporting citations, unresolved source passages, priority and release recommendation. Full
replay inputs/responses are retained privately and retrieved with `receipt`;
ordinary packets contain only the bounded conclusions. Read-only queries never
trigger inference or load model credentials.

## Doty contract

The companion changes in `nix-doty` use `chaosbox scratch query` with JSON paths
on stdin. Both sides enforce version, scope, host, root and budgets: at most
256 requested paths, 10,000 scanned entries and 4 MiB input/output. Doty adds a
five-second subprocess timeout. Truncated, stale, unavailable or identity-mismatched
intelligence cannot authorize ledger-backed cleanup.

`doty analyze opencode` adds `scratch_ledger` packets to its metadata report,
including purpose sources, annotations and unfinished descendants. Removal
previews include the same intelligence and pin ledger settings in the removal
plan. Apply and purge query again, including per-target checks. Ancestor and
descendant holds are checked together. Quarantined objects are queried by their
original ledger path plus current location; device/inode/birth identity must
still match. Restoring remains possible even when a ledger is unavailable.
The allowlisted `opencode-scratch` cleanup target also checks configured ledger
holds; `--force` does not bypass them.

With `CHAOSBOX_SCRATCH_ASSESS=true`, cleanup planning (`doty rm` preview and
allowlisted cleanup dry-runs) requests a bounded Jev refresh of stale unreleased
candidates, including ancestors/descendants. The setting is pinned in removal
plans. Refresh is capped at 60 seconds; failure is reported in
`assessment_refresh_error` and the cached/held report remains available. Regular
analysis/status and apply/purge rechecks remain read-only. Human reports show
Jev-inferred purpose, priority, obligation status, citations and freshness.

```sh
export CHAOSBOX_SCRATCH_BIN=/absolute/path/to/chaosbox
doty analyze opencode --json
doty rm --root /data/scratch/tmp/opencode --json -- \
  /data/scratch/tmp/opencode/experiment
doty rm --apply --plan PLAN_ID --json
doty purge --apply PLAN_ID --json
```

Doty's `--chaosbox-{work,scope,host,root,bin}` options can override the environment
for analysis, removal and the allowlisted run/status surfaces. With no ledger
configured, existing standalone Doty behavior remains available. Configuring a
ledger requires recreating old removal plans that did not pin that configuration.

## Runtime wiring

Chaosbox's Nix package exports `scratchPlugin` as a store-backed entrypoint with
its sibling runtime sources and `scratchAssessmentVersion = 1` for capability
negotiation. Canix's `programs.chaosbox.scratch` options provide
the root, private state and scope; the OpenCode V2 module registers the adapter
and Home Manager exports the matching Doty variables. Capture defaults on when
the consumed Chaosbox package exports this capability. Older pinned packages
remain compatible until their input is updated and Home Manager is activated.
`scratch.liveAssessment` defaults on when the package exports the assessment
capability. Both the plugin and Doty use the same store-pinned `chaosbox-canix`
wrapper, carrying the configured Jev credential-file reference. Manual plugin
configurations enable inference with `options.liveAssessment = true` and can set
`maxRequests`/`maxInputTokens`; capture itself works without Jev credentials.
Canix exports `CHAOSBOX_SCRATCH_MAX_REQUESTS` and
`CHAOSBOX_SCRATCH_MAX_INPUT_TOKENS` from the matching scratch options; Doty pins
these ceilings alongside the capture identity and assessment setting in plans.

The configured scratch root must already exist. Durable private state parents
are created by the writer. The plugin targets the explicit `index.ts` file,
which was verified through the native Promise host rather than package-directory
auto-discovery.

Filesystem observation is bounded and asynchronous. Short-lived allocations
which vanish before metadata inspection get a coverage-gap event instead of an
invented identity. Symlinks,
special files, other devices, non-UTF-8 names and exhausted watch/inventory
budgets produce degraded coverage instead of confident attribution. A query is
a fresh ledger/metadata view, not an atomic filesystem lock. Purpose sources and
explicit notes are available immediately; semantic assessments arrive separately.
The worker does not inspect arbitrary scratch file bodies or verify repository
integration/preservation references by reading their targets.

## Verification

```sh
cargo test -p chaosbox --test scratch
cargo test -p chaosbox --test scratch_assess
cargo test -p chaosbox --test scratch_assess_regressions
node --test plugins/chaosbox-scratch/test/*.test.mjs
python3 scripts/test-opencode-scratch.py \
  --opencode /absolute/OpenCode-V2-checkout --chaosbox-bin /absolute/chaosbox
node scripts/test-scratch-doty.mjs /absolute/chaosbox /absolute/doty
# Optional bounded live Jev check with synthetic evidence only:
node scripts/test-scratch-doty.mjs /absolute/chaosbox /absolute/doty --live-jev
```

The integration verifier creates an isolated scratch/state fixture and checks
nested finalization holds, changes after preview, identity-preserving quarantine
and purge with both real binaries. It removes only its own fixture.
