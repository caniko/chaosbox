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

## Crates

- `chaosbox-core` — domain types, deterministic ids, evidence contracts, graph logic
- `chaosbox-extract` — snapshot, deterministic parsing (Rust/Python/JS-TS/Markdown/text), bounded candidates
- `chaosbox-jev` — typed `POST /v1/systemone` client (`jev-1.13.0` pinned), budgets/retries/validation
- `chaosbox-gel` — SDL + migrations, typed EdgeQL ops, `gel-tokio` handle, idempotent store (reference backend; runtime path superseded)
- `chaosbox-typedb` — TypeQL schema, driver-backed store + reader, literal encoder (authoritative backend)
- `chaosbox` — orchestration, CLI, read-only MCP, `db check`/`db migrate` (contract v1 Gel / v2 TypeDB)

## Quick start (no credentials)

```sh
cargo test --workspace
cargo run -p chaosbox -- run fixtures/demo-repo --repo demo --fixture-decisions
cargo run -p chaosbox -- db check --json --repo demo
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -q -p chaosbox -- mcp
```

Jev credentials: `CHAOSBOX_JEV_API_KEY_FILE` (or operator `TYPESAFE_API_KEY`).
Backend: `CHAOSBOX_DB_BACKEND=typedb` (default `gel` until cutover).
`run` without `--live-jev` refuses to publish unless `--fixture-decisions`
is given; fixture graphs are disposable/test-only and recorded under the
`fixture-test` model identity.
TypeDB credentials: `CHAOSBOX_TYPEDB_PASSWORD_FILE` (+ optional
`CHAOSBOX_TYPEDB_ADDR/USER/DATABASE`). OpenAI/Anthropic/Gemini/Ollama
env vars are never read.

## Docs

- `docs/BASELINE.md` — pinned revisions, Graphify audit
- `docs/CAPABILITY.md` — implemented / removed / pending matrix
- `docs/HANDOFF.md` — results, handoffs, next commands

## License

MIT OR Apache-2.0. Upstream attribution in `NOTICE`.
