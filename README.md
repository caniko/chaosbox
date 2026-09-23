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
`fixture-test` model identity. `run --no-decisions` publishes extracted entities with no new semantic
decisions: no inference, no fixture accept-all, safe for real corpora.
Decisions an earlier run already paid for are republished from cache, so a
refresh whose cache covers every current candidate keeps the active build's
relations; only a refresh with nothing reusable publishes nodes alone. When
the cache cannot cover every candidate while the active build still
publishes relations, the run keeps that build and exits 4 (see below)
instead of publishing an under-covered graph.

### Machine-readable run contract

`run` speaks three stable stderr lines so a batching caller can budget and
defer without parsing prose, alongside these exit codes:

| Exit | Meaning |
| --- | --- |
| 0 | Published (or published nothing because there was nothing to do) |
| 1 | Configuration, budget preflight, or pipeline failure |
| 4 | `--no-decisions` kept the active build: the decision cache could not cover every current candidate while that build still publishes relations. Nothing was published and nothing was spent; assess the candidates with `run --live-jev` |

- `usage: requests=N input_tokens=M` — printed on **every** live-Jev exit
  path, including failures and pre-spend refusals (which report zeros), so
  a caller debits what this run dispatched (retries and timed-out sends
  included) rather than assuming a failed run spent nothing. Non-live runs
  print no `usage:` line: they cannot dispatch.
- `budget: pending=N allowed=M` — accompanies the human `live-jev budget:`
  refusal: `N` uncached candidates still need a decision each and `M` requests
  were allowed. `N <=` the caller's own batch cap means "defer this one to a
  fresh allowance", `N >` the cap means it can never fit.
- `coverage: …` — the exit-4 condition above, one human-readable line.
  Only exit 4 in `--no-decisions` mode carries it. An unreadable active
  build prints `active build: …` instead and exits 1: it keeps the current
  build untouched but fails, so a batching caller never mistakes an
  operational backend failure for a successful deferral.
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
