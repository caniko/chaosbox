# Capability matrix

Living reference for the current source tree, not a released version. The
proposed session/repository intelligence direction and session migration scope
described in the new book chapters are not yet implemented.

## Implemented in native Rust

- Session-intelligence staging: bounded JSONL proposals with native provenance,
  typed Jev admission/consolidation, private immutable artifacts and decision
  cache replay, scoped CLI and opt-in read-only MCP context/evidence tools.
  This does not yet publish session knowledge into TypeDB or replace the
  migration/compaction scripts; see `SESSION_INTELLIGENCE.md`.

- Snapshot (content-addressed, repo-scoped), deterministic extraction for
  Rust/Python/JS-TS/Nix/Markdown/text (files, modules, symbols, definitions,
  imports, containment, explicit refs, headings, links, code mentions, spans).
  Rust/TypeScript/TSX use pinned tree-sitter grammars; Nix uses rnix. Certified
  declaration and file/module containment facts publish without model decisions
  or candidate slots. Syntax failures withhold direct facts with explicit
  coverage. Rust/TS `declarations-v2` also certifies syntactic nesting and
  marked rationale comments (`NOTE:`/`WHY:`/`TODO:`/`SAFETY:`/`ponytail:`…).
  Rust/TS `calls`, `inherits` and `implements`, plus ADR/RFC citations to
  captured documents, publish as INFERRED resolved relations (unique lexical
  match, exact use-site span, no model decisions, own `resolved_relations`
  coverage bucket). Imports and other name matches remain proposals; see
  `STRUCTURAL_FACTS.md` for the exact subset (no compiler resolution or Nix
  evaluation).
- Exclusion: `.chaosboxignore` (gitignore syntax) is always honored, and
  repeatable `--exclude` globs on `snapshot`/`extract`/`run` narrow capture.
  Non-empty exclusions join the snapshot id and policy digest, so narrowing is
  never silent; unrestricted captures keep their historical ids.
- Optional SCIP protobuf ingestion: operator-only capture with source/configuration
  receipts, exact occurrence ranges, package-qualified/global and scoped-local
  anchors, explicit resolved references and implementation links. Compiler facts
  persist without model decisions in both stores; schema v4 adds compiler identity.
  Missing/ambiguous targets and omitted files are counted. Calls, typecheck success
  and active-configuration membership are not inferred; see `COMPILER_EVIDENCE.md`.
- Reviewed workspace-impact pilot: immutable private selected-file member builds,
  cited cross-language links and constraints, strict lockfile/Git version checks,
  and directed bounded impact queries with stale/missing-member reporting. The
  TS consumer → Rust command → Nix package → Canix recipe exposes the consumer
  pin mismatch; it is not general automatic workspace inference or calibrated
  memory admission. See `WORKSPACE_IMPACT.md`.
- Bounded candidate catalog (structural, lexical-import, co-occurrence; capped,
  never cartesian) with truthful truncation accounting: per-reason
  selected/omitted counts in `CandidateCatalog`, surfaced by `extract` and
  `run` output (cap never silently drops candidates)
- Typed Jev `POST /v1/systemone` client: Noul/Choice/Score, pinned `jev-1.13.0`,
  exact requested+returned model enforcement, 64k/32k enforcement (error, never
  silent truncate), sole approved TLS endpoint, redirects disabled, deadlines,
  concurrency/spend/request budgets, 429/529 retries honoring `retry-after`,
  no retry on 401/403/400/404/422, response size
  caps, answer-id reconciliation, finite/range + candidate-membership checks,
  attempt accounting, sanitized diagnostics, cache identity incl. all decision
  inputs (thresholds excluded — materialization identity)
- Credential-free `jev capabilities` policy report and research preflight;
  incompatible installed binaries fail before reserving an inference attempt.
  Offline fixture decisions have their own explicit `fixture-test` identity;
  the live client never accepts that identity.
- Evidence classes EXTRACTED/INFERRED/AMBIGUOUS (confidence never upgrades;
  INFERRED comes from accepted decisions or from deterministic name
  resolution, distinguishable by the resolver's producer identity);
  below-floor confidence abstains (recorded, never retried); responder faults
  become recorded `Failed` decisions with catch-and-continue (fault text never
  copied into evidence); `Failed` rows are supersedable once, other outcomes
  immutable
- Storage seams: `Pipeline` generic over `Store`, `GraphReader` generic over
  `GraphQueries` (live `TypeDbReader` + in-memory fake + conformance suite
  proving parity); all reads scoped to the pinned build (leakage-tested),
  LIKE wildcards escaped
- Write path (graph half): `Store::put_*` stages in `MemoryStore` and the
  publication flush writes snapshots, files, entities (+spans),
  relationships, memberships, builds with a generation-guarded pointer
  swing (concurrent publisher wins; idempotent retry; last-good stays
  active on failure); `Store` is async end-to-end;
  decision/evidence/claim flushing waits for the candidate chain (Round 3)
- Decisions, evidence, and claims persist in-pipeline: `decide()` writes each
  record as produced; `build_and_publish()` assembles one claim per
  materialized relation (same-triple rejections as contradicting evidence);
  relation filters validated loudly against the vocabulary
- Decision cache behavior: skip-on-key-match with shared evidence assembler
  (byte-identical rebuilds); recorded failures always re-asked; per-axis
  invalidation (catalog/model/rubric invalidate, thresholds reuse); stale
  rows replaced on key change, valid rows immutable
- Full-chain flush: run/set/candidate/decision/evidence/claim rows plus
  relationship evidence links in flush order; conditional decision write
  mirrors the store supersedure rule
- Worker leases: claim/heartbeat/reclaim with injected clocks and generation
  guards; stale holders recognizable (backend persistence pending)
- Graph analytics (`leiden-v1`, CLI + MCP `stats`/`community`/`report`):
  deterministic Leiden communities with labels, cohesion and modularity,
  `--resolution` recomputation (Graphify `--resolution`/`--cluster-only`),
  god nodes, scored surprising connections and suggested questions that name
  the read answering them; every item cites entity/relation ids, and
  `query report` renders Graphify's `GRAPH_REPORT.md` with runnable reads.
  Exports (`query export --format html|mermaid|graphml|cypher|wiki`): offline
  interactive HTML, Mermaid, GraphML, Cypher and a Markdown/Obsidian wiki,
  byte-reproducible, narrowed by `--community` or `--query`, collapsing to
  communities instead of refusing large graphs.
- Agent and git integration (`docs/src/agent-integration.md`): `install` /
  `uninstall` for Claude Code, OpenCode and Codex (MCP registration plus a
  guidance block, never clobbering foreign or managed files), `hook install`
  for background refresh on commit/merge/branch switch, `watch`, name-or-id
  entity arguments with exit codes 3 (ambiguous) and 4 (absent), and a local
  JSONL usage ledger for every CLI query and MCP call.
  Context seeding drops short terms and function words (Graphify rule). See
  `docs/src/graph-navigation.md`.
- Shared read-only queries (search/lookup/neighbors/path/evidence/status/diff/
  export/explain/context/stats/community/report), deterministic + Graphify-compatible export,
  backend-backed `GraphReader` with per-request active-build pinning and
  capped projections. Full MCP handshake (`initialize` negotiation, paginated
  `tools/list` with `inputSchema`, validated `tools/call`, JSON-RPC errors);
  closed read-only tool set rejected before connecting (writes, migrations,
  and model tools rejected, no Jev creds, no prose evidence)
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
  read-tx mutation rejection; `TypeDbReader` implements `GraphQueries` and
  passes the reference conformance suite unchanged (fold-column `contains`
  search preserving `ilike`, server-side ordering, build pinning)
- `db check`/`db migrate` contract v2 JSON for the TypeDB path (same exit
  mapping: 0 ready, 2 pending, 1 error); backend switch via
  `CHAOSBOX_DB_BACKEND`, credentials via `CHAOSBOX_TYPEDB_PASSWORD_FILE`
- Status freshness/provenance: `query status` (CLI + MCP) reports the pinned
  build's `status` and `snapshots` (sorted snapshot-id fingerprint, so
  consumers can detect a build that no longer matches its sources) alongside
  repo/build/generation/export caps; the in-memory and TypeDB backends fill
  it, readers whose projection predates the field report an empty list
- Schema v3 retains per-file syntax coverage, direct/decision relation counts,
  and parser provenance. Entity projections expose spans; relationship evidence
  exposes snapshot/path/content hash/range. Structural-only builds refresh under
  `--no-decisions`; decision-bearing and legacy builds retain the coverage guard.
- Pilot safety: snapshot capture stays inside the repository boundary
  (outside symlinks, dangling links, nested checkouts, and link cycles
  excluded); batches with failed decisions refuse to publish so the last
  good build stays active (outcome counts on stderr); MCP requires an
  explicit repo and bounds search limits, path hops, and traversal visits
- Source scopes + effective policy identity (issue #8): explicit
  `--source-paths` restricts capture to subtrees (Graphify `sourcePaths`
  parity; validated fail-closed, stored on the snapshot, part of the
  snapshot id so `query status` fingerprints scope changes); `EffectivePolicy`
  (`scope` + `privacy` + `inference`) gates `run --live-jev` (needs `local` +
  `typesafe-jev`; `private` never allows external inference; fixture and
  `--no-decisions` runs never egress source) and its digest feeds every
  decision cache key, so consent changes invalidate rather than reuse;
  provider key precedence (`CHAOSBOX_JEV_API_KEY_FILE` then `TYPESAFE_API_KEY`,
  single choke point, ambient provider vars never consulted); MCP stays
  read-only with no credential loading (tested)
- Gel backend removed (2026-09-23): `chaosbox-gel` crate, `dbschema/`,
  EdgeQL assets, and Gel CI jobs deleted; shared abstractions relocated to
  `chaosbox-store` (`Store`/`GraphQueries` traits, `MemoryStore`, conformance
  suite)
- Vertical slice test: snapshot -> extract -> fixture Jev -> publish -> query ->
  change/delete source -> incremental replacement build
- Release tooling (generated by simit, drift-checked by `simit init ci --check`
  and `simit upgrade --check`): six per-crate CI workflows, six
  `publish-crate-*.yaml` workflows gated on a signed release tag
  (`keys/maintainers.gpg` verified via `git verify-tag`, tag must equal the
  Cargo version), plus dry-run publish and an already-published short-circuit.
  Publication is manual (`workflow_dispatch`) and must follow the dependency
  tiers, because each crate's dry-run resolves its workspace dependencies from
  crates.io: **`chaosbox-core` → `chaosbox-extract` / `chaosbox-jev` /
  `chaosbox-store` → `chaosbox-typedb` → `chaosbox`**. The workflows do not
  encode this order; publishing out of tier fails the dry-run rather than
  publishing a broken set. Still required before the first release:
  `CRATES_IO_API_TOKEN` on the repository and one annotated tag at `0.1.0`.

## Intentionally removed (vs Graphify)

- From the code-evidence pipeline: Python execution, NetworkX, LLM-harness extraction/labeling/repair/enrichment,
  embeddings, `ext::ai`, rerankers, neural OCR/transcription, generative
  summaries/labels/dedup/query-repair, arbitrary query escape hatches,
  single giant graph JSON storage, endpoint-to-endpoint multi-links
- Hyperedges: Graphify only produces them through LLM semantic extraction;
  there is no deterministic source, so they stay out (issue #10 closes with
  deterministic communities instead)

The proposed session extension keeps that evidence boundary: deterministically
rendered, Jev-selected continuation checkpoints are derived artifacts, not source
labels, evidence or independent corroboration. Jev assesses bounded candidates
under an explicit admission policy; it does not turn fluent summaries into facts.

## Not yet implemented (explicit)

- Live proof: VM test realization (needs a build-capable session),
  write/read conformance in-guest, cache-reuse demo
- Decision-cache reuse/invalidation live proof (the Gel integration gate was
  removed with the backend; TypeDB is the authoritative write path)
- Live Jev quality runs (adapter + `--live-jev` ready; needs operator key file)
- Broader language coverage (grammar parsing beyond the current path set)
- Remaining build statistics in `status` (candidate omissions,
  catalog/rubric/model provenance recorded at publish; needs schema work —
  tracked in the chaosbox issue tracker; node/edge counts are in `stats`),
  bounded NL `query ask` (`context` covers Graphify's lexical `query`),
  workspace aggregate build and graph merging
