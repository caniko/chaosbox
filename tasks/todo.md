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

## Strict Jev-only enforcement

- [x] Pin the production endpoint/model, disable redirects, and share exact
  model checks across fresh decisions, cache reuse, intelligence and publication.
- [x] Keep explicit offline fixtures distinct from live Jev output.
- [x] Replace Spark/generative continuation guidance with Jev-selected records
  and deterministic source-backed rendering.
- [x] Add credential-free `jev capabilities` and a research preflight before
  reserving attempts; replay successful cached receipts without invoking a CLI.
- [x] Run full native/Python tests, strict Clippy, formatting and docs checks.
- [ ] Validate the production package and record downstream adoption status.

2026-09-30 verification: 257 Rust tests pass (three existing opt-in tests remain
ignored), 57 Python tests pass, strict workspace/all-target Clippy, changed-file
treefmt, diff checks and mdBook pass. Regressions cover endpoint/model overrides,
redirects, fresh/cached/publication substitutions, research preflight, failure
accounting and offline fixture separation. Credential-free smoke confirms the
installed CLI fails preflight with zero reserved attempts, the current CLI
reports its policy and returns `auth` without provider access, and offline fixture
publication still succeeds. Artifact:
`/data/scratch/tmp/opencode/chaosbox-jev-only-smoke-je98erem/smoke.json`.

Production-package retry log:
`/data/scratch/tmp/opencode/chaosbox-jev-only-package.log`. The shared evaluation
lock is held by PID 164642 building
`.#checks.x86_64-linux.stalwart016-proxy-vmtest`; the package and packaged Git/CLI
checks remain pending. Canix still pins `a4dcdc2dc7f9bd82f22d2dc57f2f8c7a2aac0b0d`;
the new CLI can be selected through `SESSION_RESEARCH_CHAOSBOX_BIN` until package
adoption is completed. This follow-up is local and does not claim deployment.
