# Capability matrix (v0.2.0)

## Implemented in native Rust

- Snapshot (content-addressed, repo-scoped), deterministic extraction for
  Rust/Python/JS-TS/Markdown/text (files, modules, symbols, definitions,
  imports, containment, explicit refs, headings, links, code mentions, spans)
- Bounded candidate catalog (structural, lexical-import, co-occurrence; capped, never cartesian)
- Typed Jev `POST /v1/systemone` client: Noul/Choice/Score, pinned `jev-1.13.0`,
  requested+returned model identities, 64k/32k enforcement (error, never silent
  truncate), TLS endpoint, deadlines, concurrency/spend/request budgets, 429/529
  retries honoring `retry-after`, no retry on 401/403/400/404/422, response size
  caps, answer-id reconciliation, finite/range + candidate-membership checks,
  attempt accounting, sanitized diagnostics, cache identity incl. all decision
  inputs (thresholds excluded — materialization identity)
- Evidence classes EXTRACTED/INFERRED/AMBIGUOUS (confidence never upgrades);
  rejected/abstained/negative/failed recorded separately, never retried as empty
- Gel SDL + migration, first-class Relationship objects, typed EdgeQL ops with
  bound params, `gel-tokio` handle with typed decoding, idempotent writes,
  predecessor-checked atomic publication, durable task claim/recovery
- Shared read-only queries (search/lookup/neighbors/path/evidence/status/diff/
  export/explain), deterministic + Graphify-compatible export, Gel-backed
  `GelReader` with per-request active-build pinning and capped projections.
  Full MCP handshake (`initialize` negotiation, paginated `tools/list` with
  `inputSchema`, validated `tools/call`, JSON-RPC errors); closed read-only
  tool set rejected before touching Gel (writes/EdgeQL/migrations/model tools
  rejected, no Jev creds, no prose evidence)
- Live Jev path: `LiveResponder` adapter + `run --live-jev` (real inference,
  real spend; default stays fixture); HTTP-level mock-service tests cover
  retries, auth-no-retry, and validation over the wire
- `db check`/`db migrate` contract v1 JSON (stdout machine-readable, stderr diagnostics)
- Vertical slice test: snapshot -> extract -> fixture Jev -> publish -> query ->
  change/delete source -> incremental replacement build

## Intentionally removed (vs Graphify)

- Python execution, NetworkX, LLM-harness extraction/labeling/repair/enrichment,
  embeddings, `ext::ai`, rerankers, neural OCR/transcription, generative
  summaries/labels/dedup/query-repair, arbitrary query escape hatches,
  single giant graph JSON storage, endpoint-to-endpoint multi-links

## Not yet implemented (explicit)

- Real Gel integration gate (needs disposable server; `test-gel` runs schema,
  mock-HTTP, and pipeline gates, exits 3 PENDING without a `gel` server)
- harbor-db Gel runtime/test interfaces for the disposable instance +
  credentials/readiness flow (plan validates against the `gel` backend)
- simit named gates + ordered publication (await parallel simit session)
- Live Jev quality runs (adapter + `--live-jev` ready; needs operator key file)
- Broader language coverage (grammar parsing beyond the 5-path vertical slice),
  communities/hyperedges materialization, signed release tags
- Threshold layering note: `decide()` applies fixed Noul/Score cutoffs while
  `Materialization` thresholds gate publication; both layers are tested and the
  materialization identity covers every threshold, so raw decisions are reusable
