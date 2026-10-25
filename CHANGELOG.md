# Changelog

## [Unreleased]

### Breaking: PostgreSQL + pgvector replaces TypeDB (0.2.0)

- The authoritative backend is the new `chaosbox-pg` crate. `chaosbox-typedb`,
  the TypeQL schema, `typedb-bootstrap`, the TypeDB VM/bootstrap tests,
  `test-typedb` and the `nixpkgs-typedb` pin are removed.
- Configuration: `CHAOSBOX_DATABASE_URL` (unix socket or loopback only)
  replaces `CHAOSBOX_TYPEDB_{ADDR,USER,DATABASE,PASSWORD_FILE}`, and
  `CHAOSBOX_DB_BACKEND=postgres` replaces `=typedb`. Lifecycle reports carry
  backend `postgres`, schema version 1. Federation providers use
  `{"kind":"postgres"}`; the Home Manager option is `backend.databaseUrl`.
- Publication writes the build, members, sealed evidence, audit rows and the
  pointer swing in one transaction under a per-repository advisory lock;
  published rows are immutable and reads are single statements, so reads
  during publication no longer fail. Evidence is no longer written one row
  per transaction before publication.
- No data migration: builds re-derive on the next snapshot, knowledge
  republishes from `memory.sqlite` (whose TypeDB-era predecessor is forgotten
  once), and replicas re-sync.
- The NixOS module enables PostgreSQL with pgvector, a peer-authenticated
  owner role and harbor-db's `postgres` migration backend; it no longer takes
  a password file.

### Added

- Graphify parity, deterministic and cited:
  - **Extraction.** Rust/TS calls, inheritance and implementations resolve to
    unique same-language definitions as INFERRED evidence (`unique-name-v1`)
    with exact use-site spans and no model decisions. Coverage gains
    `resolved_relations`.
  - **Certified facts.** `declarations-v2` certifies syntactic nesting and
    rationale comments (`NOTE:`, `WHY:`, `SAFETY:`, `ponytail:`, …). ADR/RFC
    citations link to captured documents.
  - **Exclusion.** `.chaosboxignore` and `--exclude` narrow capture, and
    non-empty exclusions join the snapshot identity.
  - **Analytics.** `leiden-v1` publication analytics: deterministic Leiden
    communities with labels and cohesion, god nodes, scored surprising
    connections and questions that name the read answering them.
  - **Reads.** `--resolution` recomputation, `query report` / MCP `report`
    (GRAPH_REPORT.md with runnable citations), `query evidence`, and context
    seeding that ignores short terms and function words.
  - **Exports.** `query export --format html|mermaid|graphml|cypher|wiki`:
    offline interactive HTML, forge-native Mermaid, GraphML, Neo4j Cypher and
    an Obsidian-compatible wiki, all byte-reproducible.
  - **Ops.** `install`/`uninstall` for Claude Code, OpenCode and Codex,
    `hook install`, `watch`, name-or-id entity arguments with exit codes 3
    (ambiguous) and 4 (absent), and a local JSONL usage ledger
    (`CHAOSBOX_QUERY_LOG=off` disables it).

- Semantic retrieval: definitions and headings carry bounded declaration
  text; `run` embeds new texts through a loopback OpenAI-compatible endpoint
  (`CHAOSBOX_EMBED_URL`, `CHAOSBOX_EMBED_MODEL`, `CHAOSBOX_EMBED_TIMEOUT_MS`).
  `search` and `context` fuse lexical and nearest-neighbour ranks (`context`
  reports `"semantic"`), and current-snapshot `intelligence_context` and
  `compaction context` rank statements by similarity too. Any embedding
  failure degrades to lexical.
- Lexical `search` ranks exact names, then prefixes, then substrings.
- Hermetic `live_pg_*` tests on disposable clusters, the `postgres-live` and
  `postgres-integration` checks, and `scripts/test-postgres.sh`.

### Changed

- Rust/TS producers report `declarations-v2`. The reader export adds
  `source_location` and relationship `scope`. `stats` gains Leiden fields
  while keeping `hubs`, and its stored packets are recomputed when their
  algorithm is older.

### Fixed

- TypeDB publication flushes content-keyed rows (spans, entities,
  relationships, memberships, file versions, candidates, evidence) in chunks of
  256 per write transaction, probing each key first, instead of committing one
  transaction per row. A canix-sized build drops from roughly 300k commits to
  about 1.2k; a chunk that meets a rival's duplicate or conflict replays through
  the per-row path, so first-write-wins semantics are unchanged. The redundant
  per-edge sealed-evidence read-back is gone; the publication swing still
  verifies the complete membership set.
- Snapshot capture skips files under a source extension that are not valid
  UTF-8 and reports them as `unreadable` instead of failing the whole run.
  Explicit file selections still refuse them.
- `no active build for repo` names the next action and carries
  `data.code = "no_active_build"` on MCP errors; TypeDB `[REX1]` read
  interruptions explain the read timeout and server-restart causes.
- Nix deployment and federation checks import existing package-set paths
  directly so cold, read-only flake qualification does not need a new store copy.
- OpenCode adapters load from standalone packaged runtimes without a runtime
  dependency on the SDK. Production install checks cover all four adapter imports.
- Reject malformed pre-bounding truncation metadata and preserve string-valued
  tool output exactly when reconstructing source-custodied checkpoints. The
  global plugin example now uses native project resolution instead of a fixed repo.
- Session-research validators reject malformed top-level input types with
  `TypeError`, including non-object portfolio and reproduction inventories.
- Global OpenCode session custody binds each archive capture and retrieval to
  the native session's canonical project root, including worktrees, and refuses
  context reduction when that identity cannot be verified.
- Source-custodied session pruning validates native outcomes and selects settled
  records in Rust. Checkpoints recover complete pre-bounding tool content and
  metadata, enforce runtime budgets, and verify full-payload plan identities
  against archived evidence before installation.
- Graph context reads bounded, build-pinned neighborhoods independently of full
  export limits. Statistics and connectivity groups read immutable analytics
  computed at publication; large legacy builds gain the cache on refresh.
- Preserve legacy published-build headers for operator membership diffs while
  keeping unsealed evidence unavailable to run-bound readers.
- Federation TypeDB reads use the read-only reader for current and historical
  knowledge. Querying a missing database no longer creates it.
- Flush one-shot federation responses before process shutdown so callers always
  receive the complete JSON frame, including denial and invalid-request replies.

### Changed

- Rust, database and documentation infrastructure now consume one qualified
  Harbor monorepo input. Database service identities remain `services.harbor-db`,
  and the default development shell includes its configured treefmt wrapper.
- Indexing avoids redundant graph/evidence copies and streams publication hashing
  and CLI exports. Complete-export comparisons show 47–56% lower peak RSS on the
  measured corpora; TypeDB server memory and the original million-fact workload
  remain unqualified. See [memory verification](docs/memory-performance.md).
- The continuation design now requires Jev-selected records and deterministic
  source-backed rendering, replacing the earlier generative-model contract.
- Session research uses pinned Typesafe Jev choices for extraction, reviewed
  reports and cross-repository comparison. Exact source spans and fixed action
  templates replace generated claims/prose, with explicit abstention and omission
  accounting. Persistent request/token budgets and content-bound receipts survive
  restarts. Start a new work directory for Jev generation; legacy reports remain
  readable. The OpenCode model-session runner and `seed-auth` were removed.

### Fixed

- Live inference is restricted to the exact pinned Typesafe Jev model and
  endpoint, with redirects disabled. Fresh graph decisions, cached inferences
  and publication reject model substitutions; explicit offline fixtures retain
  their separate identity. Legacy cache records with substituted identities are
  refused rather than silently reused.
- Research checks the CLI's enforced Jev capabilities before reserving an
  attempt, so incompatible installed binaries do not consume its request budget.
- Run publication no longer defers on unreadable active builds. Typed
  backend diagnostics flow through `active_publishes_relations`, and callers
  exit 1 with an `active build:` message instead of reporting a successful
  deferral.

### Added

- Reason-gated Nix additions retain private, durable native execution receipts,
  exact retry identity, offline context and read-only cleanup inspection.
- Mandatory server-present TypeDB federation regressions and a disposable
  reciprocal SSH gate exercising Home Manager-generated query-only endpoints,
  exact history, attribution, revocation, withholding and outage behavior.
- Durable OpenCode scratch custody records allocation provenance, native purpose
  evidence and explicit finalization holds. Bounded Jev assessments retain
  source-backed obligations; Doty consumes read-only packets and requires an
  explicit release before cleanup.
- Signed same-user peer intelligence replication preserves source capsules,
  causal history and persistent inference spending across restarts. Project
  federation exposes recipient-authorized, snapshot-pinned read-only evidence
  through SSH and portable Home Manager/NixOS modules.
- Read-only graph statistics, bounded task context and deterministic connectivity
  components through CLI and MCP, with optional reviewed workspace artifacts.
- PostgreSQL catalog capture and zero-model publication with private receipts,
  real columns/deparsed definitions, SELECT-role-visible foreign keys and
  explicit catalog coverage. Packaged SQL and PostgreSQL tools ship with the CLI.
- Bounded source-selected continuation with exact native records, mandatory user
  anchors, Jev Choice classification, deterministic rendering and persistent
  paid-attempt reservations/replay.

- Credential-free `jev capabilities` emits the model, endpoint, receipt version
  and enforced identity/redirect policy for machine-readable compatibility checks.
- Operator-only `jev evaluate --input FILE --privacy-reviewed` exposes the shared
  Rust Jev client for bounded Choice requests, with pinned-model validation and
  JSON success/failure receipts.

- Optional `compiler capture`, `compiler inspect` and `run --compiler` commands
  for source/configuration-bound SCIP evidence. Definitions, resolved references
  and explicit implementations publish without model decisions; omission and
  resolution limits remain visible. Existing TypeDB databases need additive
  schema v4 (`chaosbox db migrate`).
- Private `workspace capture` and `workspace impact` artifacts with exact member
  builds, quoted reviewed bridges, scoped constraints and bounded traversal.
  Stale sources withhold current answers; mismatched consumer/provider revisions
  block cross-repository links. Includes the TS/Rust/Nix/Canix impact pilot and
  an opt-in runtime consumer experiment.
- Bounded, deterministic selected-file snapshots with strict repository path
  checks, including shared validation for explicit compiler inputs.
- Parser-certified Rust, TypeScript/TSX/MTS/CTS and Nix declarations with
  zero-model structural publication independent of candidate budgets.
- Source citations and parser provenance in read-only queries, plus persisted
  syntax coverage and direct/decision relationship counts (additive TypeDB
  schema v3; run `chaosbox db migrate`). Structural-only builds can refresh
  under `--no-decisions` without inference coverage.
- Release tooling: six per-crate CI workflows and six signed-tag
  `publish-crate-*` workflows (simit-generated, drift-checked by
  `simit init ci --check`), with the maintainer public key in
  `keys/maintainers.gpg`. Publication stays manual and must follow the
  dependency tiers `core → extract/jev/store → typedb → chaosbox`; the
  workflows validate the tag against the Cargo version and dry-run before
  publishing.
- Sessions adoption and pinned install/rollback commands. Campaign sources
  resolve canary snapshots under `snapshots/` and boundary snapshots through
  the receipt's provenance record, with unresolvable boundaries reported as
  source errors rather than skipped silently.
- Receipt `provenance_record_digest` accessor exposing
  `provenance.boundaryRecordSha256`, so verifiers can bind a receipt to the
  exact boundary record bytes.
