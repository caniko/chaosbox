# Compiler evidence and impact pilot

- [x] Context receipt and protobuf normalization: reject stale source/config,
  escaping paths and malformed ranges; distinguish scoped locals, ambiguous
  targets and unresolved references. Verify with extract crate tests and real
  rust-analyzer/scip-typescript fixtures.
- [x] Capture/import CLI and publication: bracket explicit indexer execution,
  publish compiler facts with no decisions, persist anchors and bounded
  coverage through both stores. Verify CLI, pipeline and live TypeDB tests.
- [x] Workspace impact artifact: pin member builds and reviewed bridge evidence;
  return bounded impacted endpoints and scoped source-cited constraints;
  reject stale/missing members and wrong scope. Verify focused integration tests.
- [x] Real pilot: trace the plugin context-response consumer through Rust,
  packaging and Canix, record the applicable constraint, execute the impact
  query and document observed coverage and limitations.
- [x] Final checks: workspace tests, strict Clippy, changed-file treefmt,
  diff review and coherent local commits.
- [ ] Nix package check: realize `.#chaosbox` and verify the packaged workspace
  command with its bundled Git runtime. Currently blocked by another session's
  shared Canix evaluation lock, including on retry.

Verification record: `docs/WORKSPACE_IMPACT.md`. Native workspace tests, the
opt-in plugin/CLI experiment, disposable TypeDB 3.13.0 readback and mdBook pass.
The real query returns seven endpoints and one scoped historical-evidence
constraint; the Canix bridge remains blocked by its older Chaosbox revision pin.

## Jev-first inference follow-up

- [x] Audit model call sites: graph decisions and memory admission already use
  typed Jev; the remaining general-purpose calls were the three research stages.
- [x] Add an operator-only native Jev Choice bridge using the pinned Rust client.
- [x] Replace research extraction, report and portfolio generation with finite
  choices, exact quotations, deterministic prose and visible abstentions/omissions.
- [x] Persist request-bound receipts and cumulative attempt/token budgets; remove
  OpenCode model sessions and credential copying from new research runs.
- [x] Verify Python behavior, native CLI validation, workspace tests, strict Clippy and an actual
  five-request synthetic Jev smoke; cache replay adds no requests.
- [x] Document inference policy, migration and uncalibrated quality boundaries in
  `docs/SESSION_RESEARCH_PILOT.md`.
