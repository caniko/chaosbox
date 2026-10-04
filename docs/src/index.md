# Chaosbox

Chaosbox is a native Rust repository-intelligence pipeline: source snapshots,
deterministic extraction, bounded Jev decisions, validated evidence and claims,
versioned publication, and read-only CLI/MCP retrieval. TypeDB is its
authoritative backend; the Gel implementation was removed and its shared
abstractions now live in `chaosbox-store`. `db check`, `db migrate`, MCP and
queries always target TypeDB; `run` publishes through TypeDB only with
`CHAOSBOX_DB_BACKEND=typedb` (in-memory otherwise).

## Direction: retain intelligence, not noise

Repository structure tells us what exists. Session history can explain why it
exists, which approaches failed, what was learned, and what remains unresolved.
Chaosbox should connect these sources and make a small, useful set of
evidence-backed knowledge available to other LLM sessions.

The [selective-intelligence design](intelligence.md) defines what earns admission,
how Jev standardizes bounded decisions, and how consumers retrieve applicable
knowledge without receiving an entire archive. [Session migration](session-migration.md)
is the first intended producer and consumer of this capability.

**Status:** the session-ingestion, knowledge-admission and continuation features
are a proposed extension, not implemented commands. The existing repository
pipeline and its limits are described in the [capability reference](capability.md).
This documentation does not claim the session migration or OpenCode cutover is
complete.

## Existing CLI

From the repository root:

```sh
cargo run -p chaosbox -- --help
cargo test --workspace
```

The CLI owns mutating indexing and administration. The MCP server exposes only
bounded read operations; it does not execute inference or mutate the store.

## Build this book

The flake uses `caniko/harbor-projects`, the current name of the requested
`caniko/harbor-docs` library. `docs` and `site` are the same docs-only output;
hosting/deployment is not configured by this change.

```sh
canix cache build .#docs
canix cache build .#checks.x86_64-linux.docs-summary
```

With the project's docs shell selected, preview with `mdbook serve docs`.
Rust API documentation remains the separate `checks.<system>.doc` output.

The reference chapters include the existing repository documents rather than
copying them. Dated handoffs and cutover records describe their original
verification scope, not necessarily today's deployment state.
