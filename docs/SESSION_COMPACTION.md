# Source-custodied session compaction

Chaosbox now has a private, durable custody coordinator, a deferred intelligence
worker, TypeDB publication and an OpenCode V2 adapter. The runtime extension is
implemented against `f18083c78e54e65907000ab5a9ca4472b723ee7f` on the local
`chaosbox-compaction` OpenCode branch.

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
5. Independently assess reusable intelligence and continuation relevance using
   pinned Jev. Publication uses the existing admission/consolidation policy.
6. Publish immutable TypeDB knowledge generations through a scope-specific,
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

Build Chaosbox and apply schema version 5 through the existing migration command:

```sh
cargo build --locked -p chaosbox
chaosbox db migrate --json
```

The backend uses `CHAOSBOX_TYPEDB_ADDR`, `CHAOSBOX_TYPEDB_USER`,
`CHAOSBOX_TYPEDB_DATABASE` and `CHAOSBOX_TYPEDB_PASSWORD_FILE`. Live assessment
uses Chaosbox's existing pinned Jev credential handling. The adapter does not
read credentials or call the inference endpoint itself.

Use an OpenCode runtime containing the `compaction.plan` extension. The source
worktree is `projects/worktrees/opencode/chaosbox-compaction` in the Canix
workspace. The installed runtime has not been repinned or restarted by this
implementation. An unextended runtime fails visibly at the adapter's compatibility
fence instead of silently performing standard lossy compaction.

Copy the operator configuration from
[`plugins/chaosbox-compaction/opencode.example.jsonc`](../plugins/chaosbox-compaction/opencode.example.jsonc).
Set an absolute private `work` directory, `scope`, explicit `repo`, and the built
`chaosboxBin` if it is not on PATH. Use the same archive root for a private scope
across sessions and repositories. `liveAssessment: true` authorizes background
Jev assessment; `publish: true` enables the TypeDB outbox. Compaction itself needs
neither live inference nor database connectivity.

### OpenCode runtime handoff

The local `chaosbox-compaction` branch contains commit
`8a3968a2f4d3751eeeed1e6a1f19dde3ed7f07b9`, based on
`f18083c78e54e65907000ab5a9ca4472b723ee7f`. It adds the hook, encoded full-history
read, checkpoint budget checks, regression tests and public plugin documentation.
Export a portable patch from that checkout with:

```sh
git -C /path/to/patched/opencode format-patch -1 --stdout \
  8a3968a2f4d3751eeeed1e6a1f19dde3ed7f07b9 > chaosbox-compaction.patch
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
  --max-chars 60000 < capture.json
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
- Workers share a separate SQLite write lease; capture is not blocked by network
  dispatch. Kernel/process cleanup releases the lease after crashes.
- A publication interrupted after the TypeDB commit reconciles by content hash.
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
and persistent failed-attempt budgets. Its required-server test proves
cross-session TypeDB publication, duplicate consolidation and fresh-handle reads.

`scripts/test-typedb.sh` runs that test and the backend's pinned-publication,
idempotency and predecessor guards against a disposable TypeDB 3.13.0 server.
The adapter's `node --test` suite verifies fail-closed reduction and fixed-scope
command dispatch. OpenCode's compaction suites exercise manual, automatic,
native and overflow paths with the extension enabled or absent.

`scripts/test-opencode-compaction.py --opencode /path/to/patched/opencode
--chaosbox-bin /path/to/chaosbox` typechecks the adapter against the supplied
runtime and loads its configured directory through OpenCode's native Promise
host. It verifies the advertised tool names, complete output recovery, bounded
checkpoint failure and unloading of hooks/tools. It uses a disposable archive
and does not require inference or database credentials.

Verified locally on 2026-10-01: Rust workspace tests and build; 23 OpenCode
compaction/native/transport tests; root OpenCode lint/type checks; the OpenCode
documentation build and Chaosbox mdBook build; all six adapter tests (including a real Rust subprocess);
native-host loading/typechecking; and the disposable TypeDB gate, including
14 backend conformance tests and required-server cross-session publication.
Clippy reports existing warnings in adjacent work; the new compaction and memory
modules introduce none. The narrow Nix package evaluation was denied by tool
permissions. The installed Nix formatter also reports baseline layout drift in
`flake.nix`; the test wrapper change adds only its required Node runtime input.
