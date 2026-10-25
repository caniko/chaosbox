# Chaosbox

<!-- simit:badges:start -->

[![CI](https://img.shields.io/badge/CI-managed-2088ff)](.github/workflows/ci.yaml) [![docs](https://img.shields.io/badge/docs-enabled-6f42c1)](docs) [![crates.io](https://img.shields.io/badge/crates.io-publishable-f46623)](https://crates.io/crates/chaosbox)

<!-- simit:badges:end -->

Native Rust fork/rewrite of [Graphify](https://github.com/Graphify-Labs/graphify):
deterministic code-graph extraction, bounded Jev decisions, versioned graph
builds in PostgreSQL with pgvector semantic retrieval, read-only CLI/MCP consumers.

Deployment storage contracts are described in [Storage lifecycle integration](docs/src/storage-lifecycle.md).

Pipeline: `source snapshot -> certified syntax facts + bounded candidates ->
optional Jev decisions -> validated evidence/claims -> policy build ->
atomic publication -> read-only consumers`.

Inference policy: deterministic source/compiler facts first, then cached typed
Typesafe Jev decisions for bounded semantic judgments. Every Chaosbox-owned live
inference must use `jev-1.13.0` at the approved Typesafe endpoint; aliases,
endpoint overrides, returned-model substitutions and HTTP redirects are rejected.
Session research uses Jev for span classification, candidate priority/status and
cross-repository comparison; citations and report text are rendered
deterministically. See the
[typed research workflow](docs/SESSION_RESEARCH_PILOT.md) for request receipts,
shared budgets, abstentions and migration from the earlier generative pilot.
`chaosbox jev capabilities` reports this policy without credentials or network
access; research checks it before reserving a new inference attempt.

## Direction: selective intelligence across code and sessions

The [source-custodied compaction coordinator](docs/SESSION_COMPACTION.md)
preserves native OpenCode evidence before context reduction, assembles
source-backed checkpoints, and defers intelligence assessment and private
PostgreSQL publication through a restartable journal.

Chaosbox should connect repository structure with the decisions, findings and
constraints discovered while working on it. Extract at meaningful evidence
boundaries, use Jev's typed decisions to assess support, scope, novelty and value,
and admit only useful, grounded knowledge. Other LLM sessions should retrieve a
small, applicable set with citations instead of receiving another large summary.

Source archives, admitted intelligence and continuation checkpoints
have different roles. Continuation will use Jev selections and deterministic
source-backed templates; derived views are never independent evidence;
contradictions, scope and superseded decisions remain visible. Session migration
is the first proposed application, not a capability already shipped.

See [Selective intelligence](docs/src/intelligence.md) and
[First delivery: session migration](docs/src/session-migration.md).

## Crates

- `chaosbox-core` — domain types, deterministic ids, evidence contracts, graph logic
- `chaosbox-extract` — snapshot, deterministic parsing (Rust/Python/JS-TS/Nix/Markdown/text), bounded candidates (selected/omitted accounting)
- `chaosbox-jev` — typed `POST /v1/systemone` client (`jev-1.13.0` pinned), budgets/retries/validation
- `chaosbox-store` — shared persistence abstractions: `Store`/`GraphQueries` traits, row types, in-memory backends, conformance suite
- `chaosbox-pg` — PostgreSQL + pgvector schema, store (single-transaction publication) and read-only reader (authoritative backend)
- `chaosbox` — orchestration, CLI, read-only MCP, `db check`/`db migrate` (JSON contract v2)

## Quick start (no credentials)

```sh
cargo test --workspace
cargo run -p chaosbox -- run fixtures/demo-repo --repo demo --no-decisions
cargo run -p chaosbox -- db check --json --repo demo
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | cargo run -q -p chaosbox -- mcp
```

Jev credentials: `CHAOSBOX_JEV_API_KEY_FILE` (or operator `TYPESAFE_API_KEY`).
Backend: PostgreSQL only — `db check`/`db migrate`/query/MCP always target
`CHAOSBOX_DATABASE_URL`; `run` publishes there with `CHAOSBOX_DB_BACKEND=postgres`,
in-memory otherwise. Semantic search is optional: with `CHAOSBOX_EMBED_URL`
set, `run` embeds new declaration and knowledge texts and `search`/`context`
fuse lexical and nearest-neighbour ranks. See [Storage and semantic
retrieval](docs/src/storage.md).
For a trusted host connector, `chaosbox reader --admission /private/read-view.json`
serves a run-isolated, source-scoped and version-pinned MCP connection. See
[run-bound read views](docs/READ_VIEWS.md) for its admission contract, budgets,
revocation, and worker-integration gates. The admission file is not a credential;
the existing `chaosbox mcp` command remains an operator surface.

Choose `--no-decisions`, `--live-jev`, or `--fixture-decisions` explicitly.
Fixture graphs are disposable/test-only and recorded under `fixture-test`.
`run --no-decisions` publishes parser-certified declarations and file/module
containment for Rust, TypeScript/TSX (including `.mts`/`.cts`), and Nix. These
facts consume no candidate slots or model requests, even with
`--max-candidates 0`. Optional `compiler capture` / `run --compiler` adds
receipt-bound SCIP definitions, resolved references and explicit implementation
links with the same zero-model publication path. Other relationships remain
bounded decision proposals.

Previously paid, validated inferences can still be reused. A structural-only
build can refresh without any model coverage. A decision-bearing or legacy
build keeps the conservative exit-4 guard when current candidates lack reusable
decisions. See the [certified fact contract](docs/STRUCTURAL_FACTS.md) for the
exact syntax subset, citation format, coverage limits, and regression baseline.
Run `chaosbox db migrate` once per database (idempotent) before the first `run`.
Run-bound readers require a published, source-complete sealed build. See
[`docs/READ_VIEWS.md`](docs/READ_VIEWS.md).

See [optional compiler evidence](docs/COMPILER_EVIDENCE.md) for capture/import,
source/configuration staleness checks and capability limits. The
[workspace impact pilot](docs/WORKSPACE_IMPACT.md) adds private local artifacts
with exact member builds, reviewed source-cited bridges, scoped constraints and
bounded impact queries:

```sh
chaosbox workspace capture recipe.json --output /path/to/new-workspace.json
chaosbox workspace impact /path/to/new-workspace.json response-endpoint --scope private:owner
```

### Machine-readable run contract

`run` speaks three stable stderr lines so a batching caller can budget and
defer without parsing prose, alongside these exit codes:

| Exit | Meaning |
| --- | --- |
| 0 | Published (or published nothing because there was nothing to do) |
| 1 | Configuration, budget preflight, or pipeline failure |
| 4 | `--no-decisions` kept a decision-bearing or legacy active build because current candidates lacked reusable decisions. Nothing was published or spent; assess the candidates with `run --live-jev` |

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
Database: `CHAOSBOX_DATABASE_URL` (libpq key=value or URL; unix socket or
loopback only, e.g. `host=/run/postgresql dbname=chaosbox`; peer auth needs no
password). Embeddings: `CHAOSBOX_EMBED_URL` (loopback `/v1/embeddings`),
`CHAOSBOX_EMBED_MODEL`, optional `CHAOSBOX_EMBED_TIMEOUT_MS`.
OpenAI/Anthropic/Gemini/Ollama env vars are never read.

## Docs

- [Documentation overview](docs/src/index.md) and [book contents](docs/src/SUMMARY.md).
- `docs/BASELINE.md` — pinned revisions, Graphify audit
- `docs/CAPABILITY.md` — implemented / removed / pending matrix
- `docs/HANDOFF.md` — results, handoffs, next commands
- `docs/SESSION_INTELLIGENCE.md` — selective session knowledge, Jev admission,
  private artifacts and read-only context/evidence retrieval

The docs-only mdBook uses [Harbor](https://github.com/caniko/harbor)'s
`lib.docs` namespace. Rust toolchains and database lifecycle modules consume
the same revision-qualified input. Build with
`canix cache build .#docs`; `.#site` resolves to the same output. The
`docs-summary` check verifies chapter coverage. Preview with `mdbook serve docs`
inside the docs dev shell. This wires packaging, not hosted-site deployment.

## License

MIT OR Apache-2.0. Upstream attribution in `NOTICE`.
