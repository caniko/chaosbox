# Capability matrix (v0.7.0)

This is the existing repository-pipeline capability reference. The proposed
session/repository intelligence direction and session migration scope described
in the new book chapters are not yet implemented.

## Implemented in native Rust

- Session-intelligence staging: bounded JSONL proposals with native provenance,
  typed Jev admission/consolidation, private immutable artifacts and decision
  cache replay, scoped CLI and opt-in read-only MCP context/evidence tools.
  This does not yet publish session knowledge into TypeDB or replace the
  migration/compaction scripts; see `SESSION_INTELLIGENCE.md`.

- Snapshot (content-addressed, repo-scoped), deterministic extraction for
  Rust/Python/JS-TS/Nix/Markdown/text (files, modules, symbols, definitions,
  imports, containment, explicit refs, headings, links, code mentions, spans).
  Nix: bindings/functions as definitions, relative `.nix` imports resolved to
  the target file when it is in the snapshot (stubs stay visible when not),
  interpolation/`inherit` references to in-file bindings (regex-based,
  parse-only; no Nix evaluation)
- Bounded candidate catalog (structural, lexical-import, co-occurrence; capped,
  never cartesian) with truthful truncation accounting: per-reason
  selected/omitted counts in `CandidateCatalog`, surfaced by `extract` and
  `run` output (cap never silently drops candidates)
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
  retries, auth-no-retry, and validation over the wire; budget preflight
  (`uncached_decisions`) counts cache misses/recorded failures per candidate
  and fails before spending when they exceed `max_requests` (default mismatch:
  200 candidates vs 100 requests now fails fast instead of burning budget and
  dying at publish)
- TypeDB write-through decisions: `put_decision`/`put_evidence` persist each
  paid decision (and its evidence) in a short transaction as produced, so a
  dead worker or a budget failure later in the run loses no completed
  inference; publish-time flush stays idempotent and readers pin builds, so
  unwritten-then-written rows stay invisible until the pointer swings
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
- Status freshness/provenance: `query status` (CLI + MCP) reports the pinned
  build's `status` and `snapshots` (sorted snapshot-id fingerprint, so
  consumers can detect a build that no longer matches its sources) alongside
  repo/build/generation/export caps; the in-memory and TypeDB backends fill
  it, the superseded Gel projection defaults to an empty list
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

- From the code-evidence pipeline: Python execution, NetworkX, LLM-harness extraction/labeling/repair/enrichment,
  embeddings, `ext::ai`, rerankers, neural OCR/transcription, generative
  summaries/labels/dedup/query-repair, arbitrary query escape hatches,
  single giant graph JSON storage, endpoint-to-endpoint multi-links

The proposed session extension keeps that evidence boundary: generated
continuation checkpoints are derived artifacts, not source labels, evidence or
independent corroboration. Jev assesses bounded candidates under an explicit
admission policy; it does not turn fluent summaries into facts.

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
- Broader language coverage (grammar parsing beyond the current path set),
  communities/hyperedges materialization, signed release tags
- Stored build statistics in `status` (node/edge counts, extraction coverage,
  catalog/rubric/model provenance recorded at publish; needs schema work —
  tracked in the chaosbox issue tracker), bounded NL `query ask`, workspace
  aggregate build, stats/communities read surface
- Single threshold source: `Materialization` carries every cutoff
  (Noul/Score/confidence); its identity covers all of them, so threshold
  changes reuse valid raw decisions instead of re-asking Jev
