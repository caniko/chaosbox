# Changelog

## [Unreleased]

### Fixed

- Run publication no longer defers on unreadable active builds. Typed
  backend diagnostics flow through `active_publishes_relations`, and callers
  exit 1 with an `active build:` message instead of reporting a successful
  deferral.

### Added

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
