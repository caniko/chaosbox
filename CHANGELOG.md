# Changelog

## [Unreleased]

### Changed

- Session research uses pinned Typesafe Jev choices for extraction, reviewed
  reports and cross-repository comparison. Exact source spans and fixed action
  templates replace generated claims/prose, with explicit abstention and omission
  accounting. Persistent request/token budgets and content-bound receipts survive
  restarts. Start a new work directory for Jev generation; legacy reports remain
  readable. The OpenCode model-session runner and `seed-auth` were removed.

### Fixed

- Run publication no longer defers on unreadable active builds. Typed
  backend diagnostics flow through `active_publishes_relations`, and callers
  exit 1 with an `active build:` message instead of reporting a successful
  deferral.

### Added

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
