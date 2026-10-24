# Source-custodied session compaction

Chaosbox now has a private, durable custody coordinator, a deferred intelligence
worker, PostgreSQL publication and an OpenCode V2 adapter. Rust owns capture validation,
settled-record selection, checkpoint bounds, pruning dispositions and recovery.
The runtime extension is integrated against Canix's OpenCode pin
`047c74ec64ca6029f93be4bb4d1f5797c8838b9a` on the local `chaosbox-custody` branch.

## Contract

1. At tool settlement, capture the complete result before generic output bounding.
   Reconcile those bytes into the settled native message before assessment so
   the early capture never becomes a second independent source occurrence.
2. Before compaction, capture native history across all previous checkpoints.
3. Commit immutable record hashes, a capture manifest and pending work together in
   a private SQLite database with `synchronous=FULL`.
4. Build a deterministic continuation view from original records. Large tool
   bodies and attachment bytes have recoverable hash/pointer references. Tool
   inputs, outcome, exit metadata, original user requirements and uncertain
   assistant state remain in the checkpoint.
   Pre-bounding results replace bounded content and metadata together; a result
   already marked truncated cannot establish complete output custody.
5. Independently assess reusable intelligence and continuation relevance using
   pinned Jev. Publication uses the existing admission/consolidation policy.
6. Publish immutable PostgreSQL knowledge generations through a scope-specific,
   compare-and-swap predecessor pointer. Cross-session readers pin that generation.

The archive keeps exact native JSON values and all captured variants. JSON
serialization is canonicalized for identity; this is not a byte-for-byte export
of OpenCode's SQLite pages. Normalized inference sources remain recoverable by
their separate digests. Provider state, reasoning and generated checkpoints are
excluded from independent intelligence evidence.

## Why custody precedes assessment

Private cross-session visibility, Chaosbox-owned checkpoints and proceeding after
custody were selected for this integration. Model or database availability must
not decide whether original sources survive. The capture transaction, deferred
assessment journal, database pointer update and runtime checkpoint installation
are separate durable stages with explicit retry identities.

Knowledge admission and pruning answer different questions. A rejected or
abstained knowledge proposal never permits deletion of working state. Only a
validated high-confidence `irrelevant` continuation classification can omit a
tool-free assistant record; its receipt must match the exact original record and
the current latest user anchor. A changed anchor conservatively restores it.

## Enable

Build Chaosbox and apply the current packaged schema (version 8) through the
existing migration command:

```sh
cargo build --locked -p chaosbox
chaosbox db migrate --json
```

The backend uses `CHAOSBOX_DATABASE_URL` (unix socket or loopback; peer
authentication). With `CHAOSBOX_EMBED_URL`/`CHAOSBOX_EMBED_MODEL` set,
`compaction context` also ranks by statement similarity. On first open under
0.2.0 each journal forgets its TypeDB-era publication predecessor once, so the
first PostgreSQL publication starts a fresh chain. Live assessment
uses Chaosbox's existing pinned Jev credential handling. The adapter does not
read credentials or call the inference endpoint itself.

Use an OpenCode runtime containing the `compaction.plan` extension. The source
worktree is `projects/worktrees/opencode/chaosbox-custody` in the Canix
workspace. The installed runtime has not been repinned or restarted by this
implementation. An unextended runtime fails visibly at the adapter's compatibility
fence instead of silently performing standard lossy compaction.

Copy the operator configuration from
[`plugins/chaosbox-compaction/opencode.example.jsonc`](../plugins/chaosbox-compaction/opencode.example.jsonc).
Set an absolute private `work` directory, `scope`, and the built `chaosboxBin`
if it is not on PATH. Global configuration omits `repo`: the adapter resolves
each session's canonical project root through the native runtime, including
worktrees. An explicit `repo` remains available for project-local configuration.
Use the same archive root for a private scope
across sessions and repositories. `liveAssessment: true` authorizes background
Jev assessment; `publish: true` enables the PostgreSQL outbox. Compaction itself needs
neither live inference nor database connectivity.

### OpenCode runtime handoff

The local `chaosbox-custody` branch integrates the original extension
(`8a3968a2f4d3751eeeed1e6a1f19dde3ed7f07b9`) with the current pinned OpenCode
compaction lifecycle. It adds the hook, encoded full-history read, checkpoint
budget checks, regression tests and public plugin documentation.
Export a portable patch from that checkout with:

```sh
git -C /path/to/patched/opencode format-patch -1 --stdout \
  HEAD > chaosbox-compaction.patch
```

After applying it to the pinned base with `git am`, install that checkout's
dependencies and run its root `bun run check`, then:

```sh
bun test --cwd packages/core test/session-compaction.test.ts \
  test/session-native-compaction.test.ts test/session-compaction-transport.test.ts
```

From Chaosbox, run the native-host verifier with the patched checkout and built
binary. The installed Canix package still needs to consume this runtime commit
before the adapter can own compaction in ordinary sessions.

## Commands and recovery

```sh
chaosbox memory compact --work /private/memory --scope private:can \
  --max-chars 60000 --budget-tokens 80000 < capture.json
chaosbox memory observe --work /private/memory --scope private:can < capture.json
chaosbox memory verify-plan --work /private/memory --scope private:can PLAN_ID
chaosbox memory status --work /private/memory --scope private:can
chaosbox memory drain --work /private/memory --scope private:can --live-jev \
  --publish --max-requests 1000 --max-input-tokens 10000000
chaosbox memory publish --work /private/memory --scope private:can
chaosbox memory context --scope private:can --repo chaosbox 'source custody'
chaosbox memory evidence --work /private/memory --scope private:can HASH \
  --pointer /parts/1/state/content/0/text --offset 0 --max-chars 12000
```

Capture stdin has the versioned shape:

```json
{
  "version": 1,
  "scope": "private:can",
  "repo": "chaosbox",
  "source": "opencode",
  "session": "ses_native",
  "records": [
    {
      "id": "msg_native",
      "type": "user",
      "text": "Keep exact requirements.",
      "time": { "created": 1 }
    }
  ]
}
```

- Failed/interrupted inference retains its spending reservation and pending job.
  An explicit `drain --retry` authorizes another attempt. Negative decisions and
  successful receipts replay; budgets accumulate across restarts and sessions.
- `observe` validates the complete snapshot in Rust, commits its settled subset
  and reports unsettled IDs. Strict `capture` and `compact` refuse active work.
  Missing content, malformed outcomes, duplicate calls and foreign session IDs
  reject the snapshot before mutation.
  Settled shells require their output, and legacy prompt attachments require
  inline bytes before a checkpoint can externalize them by hash and pointer.
- `compact` persists a plan only within both character and optional runtime-token
  bounds. The adapter supplies the runtime bound; Rust conservatively charges
  UTF-8 bytes. Custody survives a budget refusal.
- `verify-plan` checks the full plan digest, archived sources, externalization
  and source-bound omission receipts. Compaction verifies its stored plan before
  returning it. Later assessments cannot rewrite an issued plan's dispositions.
  Earlier `session-pruning-v1` plans remain archived; regenerate a plan to obtain
  the full-payload `session-pruning-v2` identity required by this verifier.
- Workers share a separate SQLite write lease; capture is not blocked by network
  dispatch. Kernel/process cleanup releases the lease after crashes.
- A publication interrupted after the PostgreSQL commit reconciles by content hash.
  A competing scope publisher changing the predecessor produces a conflict;
  the outbox is retained and no newer generation is overwritten.
- The archive tool pages exact source fields with Unicode character offsets.
  `has_more` and `next_offset` expose incomplete retrieval explicitly.
- Legacy bounded outputs missing pre-bounding custody, unsettled assistant/tool
  groups, oversize protected checkpoints and missing attachment bytes refuse
  reduction. Native OpenCode history remains available for recovery.

## Bounds and quality

Capture is limited to 64 MiB per native snapshot and 100,000 records; checkpoints
are limited to 120,000 characters and independently checked against OpenCode's
remaining context budget. These limits fail closed rather than truncating intent.

Live extraction scans all source lines within the existing 24–1200 byte candidate
contract. Out-of-contract lines and oversized candidate context remain explicit
unassessed coverage. Semantic recall is not certified. Every source remains
recoverable even when it is not admitted, and protected working state remains
conservative. The initial planner can refuse a sufficiently long session because
all original user requirements are still protected.

The adapter also captures settled context before primary requests and after
step/job completion events for sessions it has observed locally. This includes
the final assistant reply even when there is no subsequent primary request.
Compaction reconciles the original native history independently of those advisory
event deliveries.

## Verification

`crates/chaosbox/tests/compaction.rs` exercises custody/restart recovery, scope and
symlink isolation, source variants, complete tool-result linkage, non-English
candidate extraction, independent admission/pruning, changed-anchor restoration
and persistent failed-attempt budgets. `compaction_native.rs` additionally tests
malformed native records, Rust settlement selection, bounded-preview reconciliation,
Unicode budget refusal, stored-plan tampering and lost-custody refusal.
The `live_pg_` test proves
cross-session PostgreSQL publication, duplicate consolidation and fresh-handle reads
on its own disposable cluster; the backend's pinned-publication, idempotency and
predecessor guards run in the `chaosbox-pg` live suite (`cargo test --workspace
-- --ignored live_pg_`). The adapter's `node --test` suite verifies fail-closed reduction and fixed-scope
command dispatch. OpenCode's compaction suites exercise manual, automatic,
native and overflow paths with the extension enabled or absent.

`scripts/test-opencode-compaction.py --opencode /path/to/patched/opencode
--chaosbox-bin /path/to/chaosbox` typechecks the adapter against the supplied
runtime and loads its configured directory through OpenCode's native Promise
host. It verifies the advertised tool names, complete output recovery, bounded
checkpoint failure and unloading of hooks/tools. It uses a disposable archive
and does not require inference or database credentials.

Verified locally on 2026-10-08: Rust workspace tests and build; focused Clippy
without warnings; 22 current-pin OpenCode compaction/native/transport tests;
root OpenCode lint/type checks and documentation build; all six adapter tests
(including real Rust subprocess recovery); native-host loading/typechecking;
and the disposable TypeDB gate with 19 backend tests and required-server
cross-session publication. Rust formatting passes through treefmt with the
repository's edition-2021 configuration. (That record predates 0.2.0, which
replaced the TypeDB backend with PostgreSQL.)
Runtime package consumption and activation are still required for ordinary sessions.
