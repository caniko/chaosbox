# Chaosbox vision

A native Rust rewrite of Graphify that builds a deterministic code graph from a
repository and serves it read-only to people and agents: source snapshot,
certified syntax facts, bounded semantic decisions, validated evidence, atomic
publication into PostgreSQL with pgvector, then CLI and MCP consumers. Other
sessions - Claude, OpenCode, Codex, code review agents - query the graph instead
of grepping, so they can see structure, dependencies and the reviewed reasoning
behind a change.

## Problem

An agent reading a large repository starts from text search: fast, shallow, and
blind to structure. It cannot tell a declaration from a mention, a real call
edge from a lexical coincidence, or a constraint someone already reviewed. The
previous system (Graphify) answered some of this but pulled in network and model
dependencies into the operational path. Chaosbox keeps the useful graph and
labels every claim by its evidence: a parser-certified fact, optional compiler
evidence, a deterministic (lexical) inference, or a bounded, cached,
source-cited model decision.

## Principles

- Deterministic facts first. Parser-certified declarations, containment, and
  resolved calls/inherits/implements publish with zero model requests and no
  candidate slots; syntax failures withhold facts with explicit coverage rather
  than guessing.
- Inference is bounded and pinned. Live inference must use `jev-1.13.0` at the
  approved Typesafe endpoint; aliases, endpoint overrides, returned-model
  substitutions and HTTP redirects are rejected. Budgets, retries and attempt
  accounting are explicit.
- Every choice is named. `--no-decisions`, `--live-jev` or `--fixture-decisions`
  is required; fixture graphs are disposable and recorded under `fixture-test`,
  and the live client never accepts that identity.
- Publication is atomic and versioned. A build row exists only when its complete
  sealed evidence membership committed; concurrent writers must agree on the
  full graph/evidence digest.
- Consumers are read-only. MCP and reader surfaces never follow the active
  pointer after admission, and run-bound readers are admission contracts rather
  than bearer credentials.
- Honest accounting for batching callers: stable stderr lines (`usage:`,
  `budget:`, `coverage:`) and exit code 4 mean "deferred, nothing spent or
  published", distinct from operational failure.
- Derived views are never independent evidence; contradictions, scope and
  superseded decisions stay visible.
- Privacy by omission: no OpenAI/Anthropic/Gemini/Ollama environment variables
  are read; database and embedding endpoints are loopback or unix sockets.

## Scope

- `chaosbox-core`: domain types, deterministic ids, evidence contracts, graph
  logic.
- `chaosbox-extract`: content-addressed snapshots, tree-sitter and rnix parsing
  for Rust, Python, JS/TS, Nix, Markdown and text, plus bounded candidate
  catalogs with selected/omitted accounting.
- `chaosbox-jev` and the decision pipeline; `chaosbox-store` and `chaosbox-pg`
  for PostgreSQL + pgvector persistence and semantic search.
- The `chaosbox` crate: orchestration, CLI, read-only MCP, `db check`/`db
  migrate`, install/uninstall for agent configs and git hooks.
- Optional compiler evidence (SCIP ingestion), workspace-impact pilot, session
  intelligence/compaction staging and run-bound read views.

## Non-goals

- A general-purpose LLM summarizer or chatbot over a repository.
- Automatic workspace inference beyond the reviewed pilot; a cited
  cross-language link is not a calibrated general claim.
- Replacing the migration scripts or full runtime integration of session
  compaction today; the capability matrix marks that as incomplete.
- Carrying Graphify's network/runtime dependencies in the operational path.

## Direction

The stated direction is "selective intelligence across code and sessions":
connect repository structure to the decisions, findings and constraints found
while working, using typed Jev decisions to admit only useful, grounded
knowledge, then let other sessions retrieve a small applicable set with
citations. Session migration is the first proposed application, not a shipped
capability; the roadmap keeps source archives, admitted intelligence and
continuation checkpoints as distinct roles.

## Place in the fleet

Canix's history records a Chaosbox 0.2 cutover that moved Atlas graphs from
TypeDB onto a `chaosbox` PostgreSQL + pgvector database behind the writer fence,
with a `canix workspace graph chaosbox status` command and embedding retrieval
through llama-swap; the migration notes and the plan to retire the old OCI path
remain in Canix's changelog and planning docs. The repository is not currently a
Canix flake input, and its Paperclip project is retired and never scheduled. The
`chaosbox` global skill in `ai-skills` is the agent-facing entry point: agents use
the MCP server to query source-backed graphs, reviewed impact and engineering
memory instead of grepping.
