# Compiler evidence and impact pilot

- [x] Context receipt and protobuf normalization: reject stale source/config,
  escaping paths and malformed ranges; distinguish scoped locals, ambiguous
  targets and unresolved references. Verify with extract crate tests and real
  rust-analyzer/scip-typescript fixtures.
- [x] Capture/import CLI and publication: bracket explicit indexer execution,
  publish compiler facts with no decisions, persist anchors and bounded
  coverage through both stores. Verify CLI, pipeline and live TypeDB tests.
- [ ] Workspace impact artifact: pin member builds and reviewed bridge evidence;
  return bounded impacted endpoints and scoped source-cited constraints;
  reject stale/missing members and wrong scope. Verify focused integration tests.
- [ ] Real pilot: trace the plugin context-response consumer through Rust,
  packaging and Canix, record the applicable constraint, execute the impact
  query and document observed coverage and limitations.
- [ ] Final checks: workspace tests, strict Clippy, changed-file treefmt,
  diff review and coherent local commits.
