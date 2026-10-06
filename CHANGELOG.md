# Changelog

## [Unreleased]

### Fixed

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
