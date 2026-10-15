# Chaosbox compaction for OpenCode V2

Source custody, source-backed checkpoints, deferred Jev assessment and private
cross-session TypeDB retrieval. This adapter requires the `compaction.plan`
OpenCode extension implemented on `chaosbox-custody`, based on
`047c74ec64ca6029f93be4bb4d1f5797c8838b9a`.

Configure an absolute private archive directory and `scope` using
[`opencode.example.jsonc`](opencode.example.jsonc). Omit `repo` in global
configuration to use the native session's canonical project root; project-local
configuration may pin an explicit repository identity. The Rust process owns custody,
budgets, assessment and publication. `liveAssessment` is explicit consent;
capture/checkpoint assembly remains available when inference or TypeDB is down.

- `compaction.plan`: commits full native history and supplies both checkpoint
  fields before OpenCode's normal tail selection/serialization.
- Tool `execute.after`: captures complete tool results before generic bounding.
- `context`: optionally injects bounded historical intelligence from TypeDB.
- `chaosbox_archive`: paginated recovery of complete sources by hash/JSON pointer.
- `chaosbox_memory_context`: repository-scoped knowledge retrieval.
- `chaosbox_memory_status`: deferred jobs, spending and degradation.

Capture failure preserves the tool's native outcome and is reported in status.
Context reduction requires custody of any bounded-away tool body. On an
unextended runtime, the legacy compaction hook throws a compatibility error
before a lossy summary can be installed.

Inference failures require `chaosbox memory drain --retry` to spend a fresh
reservation. Database publication can be retried independently with
`chaosbox memory publish`. Complete contracts and recovery commands are in
[`docs/SESSION_COMPACTION.md`](../../docs/SESSION_COMPACTION.md).

```sh
node --test plugins/chaosbox-compaction/test/*.test.mjs
python3 scripts/test-opencode-compaction.py --opencode /path/to/patched/opencode \
  --chaosbox-bin "$PWD/target/debug/chaosbox"
```

The native-host verifier checks the adapter's TypeScript types, loads it through
OpenCode's real plugin loader, exercises complete tool custody and paginated
retrieval, and verifies unload cleanup. Supply a checkout with dependencies
installed. It removes its own temporary fixture when finished.
