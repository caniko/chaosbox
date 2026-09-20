# Capability matrix (v0.7.0)

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
  below-floor confidence abstains (recorded, never retried); responder faults
  become recorded `Failed` decisions with catch-and-continue (fault text never
  copied into evidence); `Failed` rows are supersedable once, other outcomes
  immutable
- Storage seams: `Pipeline` generic over `Store`, `GelReader` generic over
  `GelQueries` (live `GelHandle` + in-memory fake + conformance suite proving
  parity); all reads scoped to the pinned build (leakage-tested), LIKE
  wildcards escaped
- Gel write path (graph half): `GelStore` stages in `MemoryStore` and flushes
  snapshots, files, entities (+spans), relationships, memberships, builds at
  publication with a generation-guarded pointer swing (concurrent publisher
  wins; idempotent retry; last-good stays active on failure); `Store` is
  async end-to-end; decision/evidence/claim row flushing waits for the
  candidate chain (Round 3); full decision-chain EdgeQL consts reviewed
  (12-param tuple ceiling forced span/evedence statement splits)
- Decisions, evidence, and claims persist in-pipeline: `decide()` writes each
  record as produced; `build_and_publish()` assembles one claim per
  materialized relation (same-triple rejections as contradicting evidence);
  relation filters validated loudly against the vocabulary
- Decision cache behavior: skip-on-key-match with shared evidence assembler
  (byte-identical rebuilds); recorded failures always re-asked; per-axis
  invalidation (catalog/model/rubric invalidate, thresholds reuse); stale
  rows replaced on key change, valid rows immutable
- Full-chain Gel flush: run/set/candidate/decision/evidence/claim rows plus
  relationship evidence links in FK order; conditional decision upsert mirrors
  the store supersedure rule; FK-chain consts reviewed (12-param ceiling,
  `<uuid><str>` casts, no new deps)
- Durable worker leases: `WorkerTask` SDL + `m2` migration; claim/heartbeat/
  reclaim with injected clocks and generation guards; stale holders recognizable
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
- TypeDB backend (`chaosbox-typedb`, authoritative): TypeQL schema mirroring
  the domain with typed relations/roles (`relationship`, memberships,
  occurrences, claim evidence), stable ids as `@key`s, deterministic relation
  keys, epoch-millis timestamps; centralized literal encoder (adversarial
  tests); staging-validated flush with CNT9 idempotency and bounded transient
  retries; single-transaction pointer swing re-validating predecessor and
  generation live (exactly-once publication proven with concurrent
  publishers); uncertain-commit reconciliation by durable-state re-read;
  read-tx mutation rejection; `TypeDbReader` implements `GelQueries` and
  passes the reference conformance suite unchanged (fold-column `contains`
  search preserving `ilike`, server-side ordering, build pinning)
- `db check`/`db migrate` contract v2 JSON for the TypeDB path (same exit
  mapping: 0 ready, 2 pending, 1 error); backend switch via
  `CHAOSBOX_DB_BACKEND`, credentials via `CHAOSBOX_TYPEDB_PASSWORD_FILE`
- Pilot safety: snapshot capture stays inside the repository boundary
  (outside symlinks, dangling links, nested checkouts, and link cycles
  excluded); batches with failed decisions refuse to publish so the last
  good build stays active (outcome counts on stderr); MCP requires an
  explicit repo and bounds search limits, path hops, and traversal visits
- Gel runtime path superseded (reference `MemoryStore`/conformance remain);
  removal from the active path after cutover validation
- `db check`/`db migrate` contract v1 JSON (stdout machine-readable, stderr diagnostics)
- Vertical slice test: snapshot -> extract -> fixture Jev -> publish -> query ->
  change/delete source -> incremental replacement build

## Intentionally removed (vs Graphify)

- Python execution, NetworkX, LLM-harness extraction/labeling/repair/enrichment,
  embeddings, `ext::ai`, rerankers, neural OCR/transcription, generative
  summaries/labels/dedup/query-repair, arbitrary query escape hatches,
  single giant graph JSON storage, endpoint-to-endpoint multi-links

## Not yet implemented (explicit)

- Live proof: VM test realization (needs a build-capable session),
  write/read conformance in-guest, cache-reuse demo
- Real Gel integration gate green (blocked on realization, not on design);
  Gel write-path integration (`Pipeline` through a Gel-backed `Store`,
  candidate/decision/evidence inserts) and decision-cache reuse/invalidation
- harbor-db Gel runtime/test interfaces for the disposable instance +
  credentials/readiness flow (plan validates against the `gel` backend)
- simit named gates + ordered publication (await parallel simit session)
- Live Jev quality runs (adapter + `--live-jev` ready; needs operator key file)
- Broader language coverage (grammar parsing beyond the 5-path vertical slice),
  communities/hyperedges materialization, signed release tags
- Single threshold source: `Materialization` carries every cutoff
  (Noul/Score/confidence); its identity covers all of them, so threshold
  changes reuse valid raw decisions instead of re-asking Jev
