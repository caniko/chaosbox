# Chaosbox

<!-- simit:badges:start -->

![CI](https://img.shields.io/badge/CI-managed-2088ff) [![docs](https://img.shields.io/badge/docs-enabled-6f42c1)](https://docs.rs/chaosbox) [![crates.io](https://img.shields.io/badge/crates.io-ready-f46623)](https://crates.io/crates/chaosbox)

<!-- simit:badges:end -->

Native Rust fork/rewrite of [Graphify](https://github.com/Graphify-Labs/graphify):
deterministic code-graph extraction, bounded Jev decisions, TypeDB-backed
versioned graph builds, read-only CLI/MCP consumers.

Pipeline: `source snapshot -> deterministic extraction + candidates ->
bounded Jev decisions -> validated evidence/claims -> policy build ->
atomic publication -> read-only consumers`.

## Direction: selective intelligence across code and sessions

Chaosbox should connect repository structure with the decisions, findings and
constraints discovered while working on it. Extract at meaningful evidence
boundaries, use Jev's typed decisions to assess support, scope, novelty and value,
and admit only useful, grounded knowledge. Other LLM sessions should retrieve a
small, applicable set with citations instead of receiving another large summary.

Source archives, admitted intelligence and generated continuation checkpoints
have different roles. Generated summaries are never independent evidence;
contradictions, scope and superseded decisions remain visible. Session migration
is the first proposed application, not a capability already shipped.

See [Selective intelligence](docs/src/intelligence.md) and
[First delivery: session migration](docs/src/session-migration.md).

## Crates

- `chaosbox-core` — domain types, deterministic ids, evidence contracts, graph logic
- `chaosbox-extract` — snapshot, deterministic parsing (Rust/Python/JS-TS/Nix/Markdown/text), bounded candidates (selected/omitted accounting)
- `chaosbox-jev` — typed `POST /v1/systemone` client (`jev-1.13.0` pinned), budgets/retries/validation
- `chaosbox-store` — shared persistence abstractions: `Store`/`GraphQueries` traits, row types, in-memory backends, conformance suite
- `chaosbox-typedb` — TypeQL schema, driver-backed store + reader, literal encoder (authoritative backend)
- `chaosbox` — orchestration, CLI, read-only MCP, `db check`/`db migrate` (JSON contract v2)

## Quick start (no credentials)

```sh
cargo test --workspace
cargo run -p chaosbox -- run fixtures/demo-repo --repo demo --fixture-decisions
cargo run -p chaosbox -- db check --json --repo demo
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -q -p chaosbox -- mcp
```

Jev credentials: `CHAOSBOX_JEV_API_KEY_FILE` (or operator `TYPESAFE_API_KEY`).
Backend: TypeDB only — `db check`/`db migrate`/query/MCP always target
TypeDB; `run` publishes through TypeDB with `CHAOSBOX_DB_BACKEND=typedb`,
in-memory otherwise.
`run` without `--live-jev` refuses to publish unless `--fixture-decisions`
is given; fixture graphs are disposable/test-only and recorded under the
`fixture-test` model identity. `run --no-decisions` publishes extracted
entities with no semantic decisions at all (nodes, no relations or claims):
no inference, no fixture accept-all, safe for real corpora.
TypeDB credentials: `CHAOSBOX_TYPEDB_PASSWORD_FILE` (+ optional
`CHAOSBOX_TYPEDB_ADDR/USER/DATABASE`). OpenAI/Anthropic/Gemini/Ollama
env vars are never read.

## Docs

- [Documentation overview](docs/src/index.md) and [book contents](docs/src/SUMMARY.md).
- `docs/BASELINE.md` — pinned revisions, Graphify audit
- `docs/CAPABILITY.md` — implemented / removed / pending matrix
- `docs/HANDOFF.md` — results, handoffs, next commands
- `docs/SESSION_INTELLIGENCE.md` — selective session knowledge, Jev admission,
  private artifacts and read-only context/evidence retrieval

The docs-only mdBook uses [harbor-projects](https://github.com/caniko/harbor-projects),
the renamed `github.com/caniko/harbor-docs` repository. Build with
`canix cache build .#docs`; `.#site` resolves to the same output. The
`docs-summary` check verifies chapter coverage. Preview with `mdbook serve docs`
inside the docs dev shell. This wires packaging, not hosted-site deployment.

## License

MIT OR Apache-2.0. Upstream attribution in `NOTICE`.
