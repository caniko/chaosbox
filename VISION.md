# Chaosbox vision

Chaosbox rewrites Graphify in native Rust. From a repository snapshot, it builds
a deterministic code graph using certified syntax facts, bounded semantic
decisions and validated evidence. It publishes atomically to PostgreSQL with
pgvector and serves the graph to people and agents through read-only CLI and MCP
queries. Claude, OpenCode, Codex and code review agents can query structure,
dependencies and reviewed reasoning behind a change instead of grepping.

## Problem

Text search is fast but blind to structure. In a large repository, it can't tell
an agent what's a declaration rather than a mention, what's a real call edge
rather than a lexical coincidence, or which constraints someone already
reviewed. Graphify partly addressed this but put network and model dependencies
in the operational path. Chaosbox keeps the graph and labels each claim by its
evidence: parser-certified fact, optional compiler evidence, deterministic
(lexical) inference, or bounded, cached, source-cited model decision.

## Principles

- Deterministic facts first. Parser-certified declarations, containment, and
  resolved calls/inherits/implements publish with zero model requests and no
  candidate slots. On syntax failure, withhold facts and report coverage rather
  than guess.
- Inference is bounded and pinned. Live inference must use `jev-1.13.0` at the
  approved Typesafe endpoint; aliases, endpoint overrides, returned-model
  substitutions and HTTP redirects are rejected. Budgets, retries and attempt
  accounting are explicit.
- Every choice is named. `--no-decisions`, `--live-jev` or `--fixture-decisions`
  is required; fixture graphs are disposable and recorded under `fixture-test`,
  and the live client never accepts that identity.
- Publication is atomic and versioned. A build row exists only after its complete
  sealed evidence membership commits. Concurrent writers must agree on the
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

The direction is "selective intelligence across code and sessions": connect
repository structure to decisions, findings and constraints from the work.
Use typed Jev decisions to admit only useful, grounded knowledge, then let other
sessions retrieve a small, applicable set with citations. Session migration is
the first proposed application, not a shipped capability. The roadmap keeps
source archives, admitted intelligence and continuation checkpoints distinct.

## Place in the fleet

Chaosbox 0.2 moved Atlas graphs from TypeDB to a `chaosbox` PostgreSQL + pgvector
database behind the writer fence, with `canix workspace graph chaosbox status`
and embedding retrieval through llama-swap. Canix's changelog and planning docs
retain the migration notes and plan to retire the old OCI path. The repository
isn't a Canix flake input; its retired Paperclip project is never scheduled.
Agents start with the `chaosbox` global skill in `ai-skills` and use MCP to query
source-backed graphs, reviewed impact and engineering memory instead of grepping.
