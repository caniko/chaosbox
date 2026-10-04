# Source-custodied compaction

Chaosbox's `memory capture|compact|drain|publish` commands implement private source
custody, conservative continuation, deferred Jev assessment and TypeDB knowledge
publication. `memory context|knowledge-evidence` pin a database generation for
cross-session retrieval; `memory evidence` recovers paginated archived sources.

The OpenCode adapter requires the `compaction.plan` extension before tail
selection. It captures full tool results at settlement and supplies both the
checkpoint and retained-tail fields. Failure to preserve a bounded-away result
or fit protected context blocks reduction.

```sh
chaosbox memory status --work /private/memory --scope private:can
chaosbox memory drain --work /private/memory --scope private:can --live-jev --publish
chaosbox memory context --scope private:can --repo chaosbox 'active requirements'
```

The repository's `docs/SESSION_COMPACTION.md` contains the full operator contract,
including configuration, archive identity, privacy boundaries, budgets and recovery.
