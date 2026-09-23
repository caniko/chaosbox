# Changelog

## [Unreleased]

### Fixed

- Run publication no longer defers on unreadable active builds. Typed
  backend diagnostics flow through `active_publishes_relations`, and callers
  exit 1 with an `active build:` message instead of reporting a successful
  deferral.

### Added

- Sessions adoption and pinned install/rollback commands. Campaign sources
  resolve canary snapshots under `snapshots/` and boundary snapshots through
  the receipt's provenance record, with unresolvable boundaries reported as
  source errors rather than skipped silently.
- Receipt `provenance_record_digest` accessor exposing
  `provenance.boundaryRecordSha256`, so verifiers can bind a receipt to the
  exact boundary record bytes.
