# Session workcards

See [plan.md](plan.md) for scope, checkpoints and the shared execution contract.
Dependencies are gates, except where explicitly marked as observational inputs.
Each session retains its focused commands/results in its handoff. The initial
estimates are small/medium (1–5 files); split a workcard when source tracing
shows a larger independent change. None of the future verification below has
been claimed as run by this planning task.

## Resource attribution and graph reads

### [x] S01 — Reproducible baseline
Invariant: comparisons identify exact binaries, unchanged complete output and a frozen corpus.
Evidence: [report](../../docs/s01-efficiency-baseline.md) and its JSON receipt.
Verified: 25 checksums, complete-export `cmp`, JSON/coverage consistency; source/staging preserved.
Boundary: no production-stack resource acceptance or full-workspace source freeze.

### [ ] S01b — Resource attribution
Depends: S01. Status: accessible observation pass complete; attribution gate open.
Invariant: process/client costs, cache/kernel/zswap, history and background work are distinguished.
Exit: allocation/history evidence obtained; resource-policy decision recorded; no unsupported attribution.
Likely scope: report/receipt and existing native inspection facilities.
Verify: [recorded native commands and exact blockers](../../docs/s01b-resource-attribution.md); checksum and counter consistency checks.
Handoff: [S01b receipt](../../docs/s01b-resource-attribution.json). S02 may proceed with these observations; major storage/schema implementation may not.
Operator decision: defer numeric limits until the repaired read path is measured; the previous numerical proposal was not adopted.
Unblock: the operator supplied the protected process/filesystem totals; the [follow-up report](../../docs/s01b-operator-followup.md) and [receipt](../../docs/s01b-operator-followup.json) reconcile 26.60 GiB anonymous-plus-swap and 28.20 GiB Chaosbox disk use, including 19.70 GiB WAL. The [disposable profiler route](../../docs/s01b-profiler-route.md) now qualifies native Massif/xtmemory allocation stacks with two required graph/knowledge cases and identifies an 81.95 MiB persistent all-host process-sampling subtree. Next producer workcard: reuse the existing own-PID refresh at sampler initialization, with a focused regression; resolve a current TypeDB producer checkout first. Production allocation/allocator/cache ownership, per-build payload bytes and engine-history decomposition remain unresolved. Massif requires a new owned profiled process; production lifecycle changes remain separately authorized. Another generic observation pass does not close this gate.

### [x] S01b.1 — Native retained-build inventory prerequisite
Depends: S01/S01b observations; independent of physical allocation attribution.
Invariant: credential-file-backed read-only inspection separates stored status from the current pointer and bounds returned metadata.
Exit: validated native CLI pagination, scalar membership counts, acquisition deadline and disposable/native readback pass.
Handoff: [report](../../docs/s01b1-retained-build-inventory.md) and [receipt](../../docs/s01b1-retained-build-inventory.json). Signed implementation/test savepoint `4704623025ee23a977ee5759043bc73e9c209836`. 94 affected tests and three explicitly required disposable-TypeDB cases pass, with scoped Clippy and treefmt. The live read independently found three retained Canix headers, with generation 3 current, in 0.91 seconds at 25,724 KiB client peak RSS. `physical_bytes` remains null; S01b stays open. Production TypeDB identity is unchanged and the disposable server is stopped.

### [x] S02 — Bounded graph navigation
Depends: S01 and S01b observations.
Invariant: a bounded navigation request does not require a complete repository export.
Exit: the recorded full-scope Canix context case succeeds with bounded source-backed results and honest omissions.
Likely scope: `src/navigation.rs`, `src/reader_navigation.rs`, shared query reader and one focused test target.
Verify: regression for the recorded `RuntimeManifest` request; focused reader tests; required disposable TypeDB readback and complete-export preservation.
Handoff: [S02 report](../../docs/s02-bounded-navigation.md) and [receipt](../../docs/s02-bounded-navigation.json). Reused the existing committed bounded reader; the new exact-executable CLI/MCP regression fails on the installed binary and passes on the published candidate and canonical build. Complete frozen export parity and disposable conformance pass. Installed-consumer promotion remains S25. This closes only the recorded context case; S02a–S02c cover remaining high-degree reads.

### [x] S02a — Count-only explanations
Depends: S02, S03a audit.
Invariant: explaining one entity counts exact pinned relationships without fetching their collections.
Exit: shared CLI/MCP explanation preserves fields, missing-entity behavior, incoming/outgoing counts and scope.
Likely scope: `GraphQueries`, reference/TypeDB counts, `GraphReader::explain` and its CLI/MCP callers; split if needed.
Verify: high degree, self-loops, missing/foreign members and exact CLI/MCP parity; record materialized rows, queries and bytes.
Handoff: [report](../../docs/s02a-count-only-explanations.md) and [receipt](../../docs/s02a-count-only-explanations.json). TypeDB aggregates both directions; reference counting scans edges directly. CLI/MCP share `GraphReader::explain`. The collection-forbidden regression, 1,024-degree disposable fixture, required backend conformance and scoped Clippy pass. Two count queries return two scalar rows (source-derived); nine calls in one MCP process produce 1,518 payload bytes. Persistent-process resource acceptance remains S02c. Next: S02b.

### [x] S02b — Complete bounded neighborhood enumeration
Depends: S02a.
Invariant: every authorized relationship remains enumerable through bounded ordinary MCP calls.
Exit: deterministic continuation or an equivalent bounded complete route preserves relationship ID, direction, type and endpoint membership.
Likely scope: shared neighborhood reader/query contract and CLI/MCP arguments; reuse existing continuation facilities.
Verify: high degree, parallel edges, self-loops and filters; enumerate every edge exactly once per direction across boundaries. Bind continuation to repository/build/anchor entity/direction/canonical filter/order, applicable authorization/view identity and token-contract version; reject wrong scope or unavailable builds and never mix generations after pointer movement. Apply the type filter before backend limits/continuation; test sparse matches after many excluded relationships.
Handoff requirement: when touching `navigation_live.rs`, verify returned relationship ID/direction/type against the pinned backend, beyond endpoint membership. Error-only overflow does not establish complete traversability.
Handoff: S02b.1/S02b.2/S02b.3 now establish backend pages, exact continuation and complete ordinary-client enumeration on the recorded fixtures. See the [ordinary-client report](../../docs/s02b3-ordinary-neighborhood-pages.md). Installed-consumer transition, aggregate request cost and production resource acceptance remain separate gates.

### [x] S02b.1 — Directional backend page primitive
Depends: S02a.
Invariant: backend filtering and exclusive relationship-ID boundaries precede row limits.
Exit: reference and TypeDB readers return deterministic directional pages without materializing the whole neighborhood; callers can request one sentinel row.
Likely scope: `GraphQueries`, reference/TypeDB implementations and `tests/neighborhood_pages.rs`.
Verify: exact and overfull boundaries, high degree, parallel types, self-loops, repeated/empty/encoded filters, sparse late-sorting matches and historical/missing/foreign members; enumerate every expected row once per direction against the complete fixture.
Handoff: [report](../../docs/s02b1-directional-pages.md) and [receipt](../../docs/s02b1-directional-pages.json). Final-source reference/required-disposable paging, 59 affected library/count-reference tests, existing required conformance, treefmt and warnings-denied Clippy pass. Three sparse matches per direction cross three pages, separated by more than 128 excluded rows. The frozen 26-entry evidence manifest verifies. Qualification used the separately authorized unchanged rooted shell after normal preparation was blocked; no admission policy changed. Signed local savepoint: `46ab2034331116e14414c585148784a831ca8646`; signature and author/committer verified. Parent S02b remains open for S02b.2/S02b.3.

### [x] S02b.2 — Scope-bound continuation and historical pinning
Depends: S02b.1.
Invariant: continuation resumes the exact authorized immutable request.
Exit: versioned continuation binds repository/build/anchor/direction/canonical filter/order and applicable authorization/view identity. Resolve recorded builds through `published_build(repo, build)`; unavailable or wrong-scope builds fail without active-pointer fallback.
Likely scope: existing continuation facilities and shared reader.
Verify: token/scope/version rejection, active-pointer movement without generation mixing, unavailable builds and a new reader resuming historical pages.
Handoff: [report](../../docs/s02b2-neighborhood-continuation.md) and [receipt](../../docs/s02b2-neighborhood-continuation.json). The shared ordinary repository-reader layer is qualified: 87 affected tests plus the explicit required TypeDB case pass, as do treefmt and scoped Clippy. Preparation precedes transport credential acquisition; exact ownership is rechecked on every resume. One cursor carries two positions and independent completion. It is untrusted query state in a repository-specific domain; scoped-view admission/evidence checks remain a separate contract. Fresh-handle historical readback passes after pointer movement; fresh-process CLI/MCP integration remains S02b.3. Signed local savepoint: `436f830283967509eee0a5d0093f3fbad327e614`; author, committer and signature verified.

### [x] S02b.3 — Ordinary CLI/MCP paging
Depends: S02b.2.
Invariant: ordinary clients can enumerate the complete authorized neighborhood with bounded responses.
Exit: advertised arguments and compatible responses expose exact build, completion and continuation metadata through the shared path.
Likely scope: CLI/MCP definitions/dispatch, shared reader and focused integration tests.
Verify: every relationship ID/direction/type/endpoint against the pinned backend, repeated calls and fresh-process continuation. Parent S02b closes only after this gate passes.
Handoff: [report](../../docs/s02b3-ordinary-neighborhood-pages.md) and [receipt](../../docs/s02b3-ordinary-neighborhood-pages.json). 91 affected tests, explicit required TypeDB client enumeration and the existing required ordinary explanation regression pass, as do treefmt and scoped warnings-denied Clippy. Fresh CLI, fresh MCP and persistent MCP agree across 29 pages / 4,124 delivered rows per transport, continuing the old generation after pointer movement. Dual-position/incoming-longer coverage is added; source, exact executables and all 27 qualification checksums are frozen. Signed local savepoint: `d42a93c04efca667a8b5b05c4b9324465c26e1ed`; author, committer and signature verified. Parent S02b's ordinary-client gate is complete; S02c owns remaining request cost/path/transport controls.

### [x] S02b.4 — Exact-publication follow-up and recursive historical traversal
Depends: S02b.3; execute after S02c.4. This review follow-up gates promotion of the reviewed reader/MCP candidate.
Invariant: every follow-up read and new expansion can retain the publication returned by a historical page.
Exit: ordinary MCP and applicable CLI callers expose validated exact-publication selection through the existing `GraphReader::published` path; default current reads retain compatibility.
Likely scope: shared current/exact request preparation, ordinary definitions/dispatch and focused client tests; split hydration and recursive expansion if tracing exceeds the session ceiling.
Verify: after pointer movement, retrieve historical endpoints/explanations and independently expected nonempty evidence, then start a new neighborhood at a returned historical endpoint. Reject foreign/unavailable/conflicting build selections without current fallback. Existing cursor scope, authorization, count-only explanations, lifecycle and driver bounds remain intact.
Handoff: [report](../../docs/s02b4-historical-followups.md) and [receipt](../../docs/s02b4-historical-followups.json). Signed local implementation/test savepoint: `4fbdca424a2e8408eedbdc2df0b995800ee8523c`. Optional exact-publication selection reuses `GraphReader::published` through MCP lookup/explain/evidence/neighbors and existing CLI lookup/explain/neighbors. Version-1 cursors retain their wire format and accept omitted or matching selection; conflicts fail before credentials. The retained old executable returns null in the historical hydration regression; the final candidate passes known nonempty source evidence, two fresh-anchor historical expansions, missing/foreign/unpublished rejection, 157 affected tests, ten explicitly required disposable-TypeDB cases, treefmt and warnings-denied Clippy. Admission, credential refresh, cleanup, driver retention/expiry, batching, pages, paths and count-only explanations are requalified. Tested source and executable identities are frozen; consumer promotion and production resource acceptance remain separate. Next: S01b attribution/inventory and S05a corpus.

### [x] S02c — Bounded path expansion and request cost
Depends: S02b.
Invariant: path search bounds aggregate work and distinguishes exhaustion from a proven no-path result.
Exit: shared CLI/MCP expansion accounts for edges, queries, bytes, whole-request deadline and cancellation without hiding reachable paths behind silent truncation.
Likely scope: shared path reader, request budgets and focused high-degree tests.
Verify: found/no-path/exhausted cases, cancellation and slow reads; repeated requests through one persistent MCP process with measured retention. Per-row/output limits alone do not satisfy the gate.

### [x] S02c.1 — Shared pinned path reader and local budgets
Depends: S02b.
Invariant: an already-pinned path search uses bounded pages and aggregate work limits.
Handoff: [report](../../docs/s02c1-bounded-path-search.md) and [receipt](../../docs/s02c1-bounded-path-search.json), signed savepoint `accc0e5b1289b10c6c51733acc8b11737430c7a1`. The retained gate passed 98 tests, including 10 focused path cases; two disposable-TypeDB cases were ignored. This establishes local reader behavior, not acquisition-inclusive deadlines or transport cancellation.

### [x] S02c.2 — Ordinary path validation and whole-request budgets
Depends: S02c.1.
Invariant: CLI and MCP reject the same malformed path arguments before credentials and account for acquisition through the last read.
Exit: shared preparation and one monotonic request budget/deadline cover backend acquisition, build pinning, endpoints and directional pages.
Likely scope: existing path request preparation, CLI/MCP dispatch and ordinary-client integration tests; reuse S02c.1's page-backed BFS and `PathBudget`.
Verify: fresh CLI, fresh MCP and persistent MCP against required disposable TypeDB; uneven directional pages, historical pinning, cycles/self-loops, found/null/exhausted results and exact work boundaries. Malformed MCP `max_hops` must not silently default; CLI and MCP share the supported ceiling.
Handoff: [report](../../docs/s02c2-whole-path-requests.md) and [receipt](../../docs/s02c2-whole-path-requests.json). Shared preparation, acquisition-inclusive monotonic budgets and build-header byte accounting pass 109 affected tests plus the explicitly required disposable TypeDB case. Fresh CLI, fresh MCP and persistent MCP agree across 14 cases each; all report query-budget exhaustion explicitly. Stalled connection deadlines return after about 10 seconds through both clients. Parent S02c remains open for S02c.3's transport lifecycle and retention evidence.

### [x] S02c.3 — Transport lifecycle and bounded retention
Depends: S02c.2.
Invariant: cancellation and transport failure stop the matching request and release retained request/backend state.
Exit: bounded frames and output backpressure, matching-ID cancellation, EOF/output-failure cleanup and measured repeated persistent-MCP retention.
Likely scope: ordinary MCP stdio lifecycle and focused client/backend tests.
Verify: unrelated cancellation IDs preserve active work; matching cancellation, oversized frames, slow/closed output and EOF release local and remote reads. Repeat ordinary calls in one MCP process and retain actual resource observations. Dropping an instrumented future alone does not close this gate.
Handoff: [report](../../docs/s02c3-mcp-lifecycle.md) and [receipt](../../docs/s02c3-mcp-lifecycle.json). Signed implementation `0050c4631a951015247fe4d6ed947ff5ea19b9dd` adds bounded framing/output and in-place matching-ID cancellation; signed test/report savepoint `6cce3cc50e5e5f7d88a034880c03322f0cdbeed5` retains qualification. 148 affected tests and three explicitly required disposable cases pass. Native TypeDB metrics observe active reads then zero after five failure/cancellation modes; wide EOF and output deadlines pass. One persistent MCP handles 147 calls with flat thread/FD counts and zero retained TCP connections. Parent S02c's fixture-backed ordinary-request gate is complete; whole-stack/production RAM acceptance and S01b remain separate.

### [x] S02c.4 — Bounded ordinary-MCP admission
Depends: S02c.3, S04a. This review follow-up gates promotion of the reviewed ordinary MCP candidate.
Invariant: overlapping valid calls serialize behind one active backend future without retaining queued backend/policy/publication state.
Exit: (1) FIFO admission bounds raw frames/count/wait and refreshes credentials/pins at dispatch; (2) active/queued cancellation, duplicate IDs, EOF, overload and backpressure preserve deterministic cleanup; (3) required disposable TypeDB proves queued pointer/credential refresh and bounded persistent retention.
Likely scope: ordinary stdio lifecycle, existing MCP unit/client tests and report/receipt. No knowledge cache or schema change.
Selected request controls: eight pending frames, 256 KiB aggregate raw-frame bytes and ten seconds of admission wait, separate from unchanged dispatch-owned backend/path deadlines. Pings and cancellation bypass admission. These are protocol controls, not accepted production resource budgets.
Verify: behavioral red/green, real stalled acquisition, count/byte exhaustion and capacity recovery, typed queued cancellation, admission expiry without another acquisition, controls during a backpressured final response, required-server queued repinning/credential replacement, existing remote cleanup/driver idle/traversal gates, treefmt and scoped warnings-denied Clippy.
Handoff: [report](../../docs/s02c4-mcp-admission.md) and [receipt](../../docs/s02c4-mcp-admission.json). Signed local implementation/test savepoint: `75198f14c21df448cc14b033b9fbd2a070e781be`; signed report savepoint: `8b30ab76d4a151adf9298b08aac451d0e1d9d854`. Author, committer and signatures verified. Both the original busy-rejection regression and the flushed-ID-reuse review regression failed before their fixes. Final source passes 152 affected tests, eight explicitly required disposable-TypeDB cases, treefmt and scoped warnings-denied Clippy. Eight calls queued across pointer movement select the replacement publication; queued credential replacement is rejected. Seven eight-call bursts retain one driver with 24 KiB RSS growth, unchanged threads/FDs and zero swap. Native queued cancellation/EOF/overload cleanup, prior path/page/explanation gates, connection batching and idle expiry pass. Production resource acceptance and consumer promotion remain separate. S02b.4 subsequently qualifies historical follow-ups and recursive expansion.

### [x] S03a — Targeted build and membership reads
Depends: S02.
Invariant: header/membership queries materialize only the rows their contract needs.
Exit: keyed headers and server-limited prefixes preserve pinned membership, declared order and sentinel semantics; these APIs are not cursor pagination.
Verified scope: `chaosbox-store/src/conformance.rs`; existing production reader reused unchanged.
Verify: current/historical headers and snapshots, missing/foreign membership and limits `-1,0,1,2,3` against reference and required disposable TypeDB. Deliberate over-return negative control fails; restored reader passes.
Handoff: [report](../../docs/s03a-keyed-membership-audit.md) and [receipt](../../docs/s03a-keyed-membership-audit.json). Client row bounds are source-derived and fixture-verified; server scan/sort and large-corpus costs remain unmeasured. S02a follow-up corrects the comparison claim to boundary-summary parity; row-payload/span equality was not asserted. The original 200-artifact bundle remains unchanged and verifies.

### [ ] S03b — Targeted knowledge and evidence reads
Parent: S03b.1 investigation precedes S03b.2 implementation. Use S03a reader conventions.
Invariant: targeted retrieval preserves complete-generation validity and authorization.
Exit: both child workcards pass; no targeted read bypasses required validation.

### [x] S03b.1 — Knowledge validation/current-eligibility investigation
Depends: S02/S03a observations; may run alongside S04a.
Invariant: the existing whole-generation guarantees are understood before retrieval changes.
Exit: trace generation hashing, reference validation, historical current eligibility and synchronized-replica checks; identify what can be reused and what requires S05 schema contracts.
Likely scope: `chaosbox-typedb/src/knowledge.rs`, federation consumers, `sync/current.rs` and an evidence-backed contract report; a no-code result is valid.
Verify: current/exact/historical retrieval and replica caller traces with existing focused checks; record the implementation gate explicitly.
Handoff: [report](../../docs/s03b1-knowledge-validation.md) and [receipt](../../docs/s03b1-knowledge-validation.json). Signed local report savepoint: `97e9ec7ac6fe449447f78de5d9363d2b37d4603c`; author, committer and signature verified. Current/exact reads already select one keyed full envelope; record/evidence mirrors lack an independent complete-generation validation proof. Complete-bundle bindings, historical current-policy eligibility and replica signed-import/published-view boundaries are traced. 55 focused tests and five explicitly required disposable-TypeDB cases pass. A missing schema prerequisite in the first compaction run was corrected through explicit native CLI migration; the failed log is retained. No runtime code/schema changed. S03b.2's backend-targeted path requires S05 and resolved S01b for substantial representation work; request-local borrowing is only a candidate optimization. Next: S01b/S05a before S05.

### [ ] S03b.2 — Targeted knowledge implementation
Depends: S03b.1; schema/representation-dependent changes also require S05 and resolved S01b.
Investigation gate: preserve full-envelope hashing/reference validation until S05 defines an independently verifiable generation/admission contract; an exact build row alone is not a publication seal. Replica event-set digests are not serialized-view hashes. See the S03b.1 report before selecting an implementation.
Invariant: small-result reads reduce materialization without weakening whole-generation validation.
Exit: current/exact reads preserve scopes, source/evidence validation, historical eligibility and replica compatibility.
Likely scope: existing knowledge storage/consumer and focused integration tests; reuse validated immutable units where contracts allow.
Verify: small result/large generation, invalid envelope/reference rejection, wrong scope, historical/current eligibility and fresh-handle TypeDB reads.

### [ ] S04 — Shared request reads
Parent: S04a graph reuse and S04b knowledge reuse; they need not execute together.
Invariant: repeated reads share immutable work with bounded lifetime and scope-safe keys.
Exit: both child workcards pass without stale-build mixing or indefinite memory growth.

### [x] S04a — Graph connection/query reuse
Depends: S03a and the relevant graph-read contracts; independent of S03b implementation.
Invariant: reuse preserves per-request build pinning, scope/policy isolation and bounded retention.
Exit: measured repeated graph reads share existing connections/immutable query work safely.
Likely scope: shared graph reader/request context and focused tests; reuse existing facilities.
Verify: before/after query and connection counts, pointer movement, scope changes, cancellation and repeated persistent-MCP memory.
Handoff: S04a.1 and S04a.2 close the fixture-backed graph reuse gate. One bounded idle driver serves fresh request pins, and each header/fingerprint batch shares one read transaction. Production/whole-stack resource acceptance remains separate.

### [x] S04a.1 — Ordinary MCP graph connection lifetime/reuse
Depends: S03a, S02c.3.
Invariant: one reusable ordinary graph connection retains no request pin or query result and is discarded on failure/cancellation or idle expiry.
Exit: (1) native before/after connection/query counts prove reuse with fresh per-request pins and credential/repository isolation; (2) cancellation/transport and idle cleanup pass; (3) persistent-process resource observations stay bounded.
Likely scope: existing TypeDB reader ownership, ordinary MCP acquisition and focused existing client tests. No graph-query result cache is introduced.
Verify: required disposable TypeDB, pointer movement, credential-file replacement, historical continuation, cancelled/failed requests, idle expiry and repeated successful calls. S04a.2 below closes the remaining parent query-batching gate.
Handoff: [report](../../docs/s04a1-graph-connection-reuse.md) and [receipt](../../docs/s04a1-graph-connection-reuse.json). Signed implementation/test savepoint `39d94d6022fa03cd6154dda16e41df69701d8aed` reuses one idle driver and repins each request; no result/header/transaction cache. Signed report savepoint: `c81b877f05f555be8a7b524c577dd163610debea`. Native 20-lookup comparison drops connections from 20 to 1 with 60 queries on both sides. 140 successful calls, credential/repository/pointer checks, idle expiry, warmed cancellation/transport cleanup and 147-call error-eviction retention pass. 153 affected tests and five explicitly required disposable cases pass, with treefmt and warnings-denied Clippy. S04a.2 below completes parent query batching/reuse; production resource acceptance and S01b remain separate gates.

### [x] S04a.2 — Bounded graph query batching/reuse
Depends: S04a.1 and existing pinned page/work-budget contracts.
Invariant: shared immutable query work reduces round trips without widening scopes, mixing builds or weakening work/error accounting.
Exit: measured backend query/transaction reduction with complete traversal and existing lifecycle/retention gates. Select the smallest batching facility after tracing callers; retained query results require an explicit validity/key/byte/lifetime contract.
Verify: focused before/after counts, page completeness, pointer movement, exact work boundaries and cancellation. Schema-dependent reuse also requires S05 and resolved S01b.
Handoff: [report](../../docs/s04a2-graph-header-batching.md) and [receipt](../../docs/s04a2-graph-header-batching.json). Signed implementation/test savepoint `ea51f2ed7ae638787e28c679ee4fc30ab44999e6` batches the selected header and exact-build fingerprint through existing `read_rows_in`. Signed report savepoint: `e89a768bcfdcbf02ff9a69f6989d80ffe841d97d`. Twenty lookups open 40 transactions instead of 60; both candidates issue 60 queries and open one connection. Zero/one-snapshot parity, historical/repository/credential checks, complete pages/paths, 140 successful-call retention, idle expiry and 147-call error-eviction retention pass. 153 affected tests and seven required disposable cases pass, with treefmt and warnings-denied Clippy. Parent S04a is complete; next is S03b.1. The existing multi-snapshot schema ceiling is retained as an S05 follow-up after S01b.

### [ ] S04b — Knowledge request reuse
Depends: S03b.2 and its applicable S05 contracts.
Invariant: reused knowledge cannot bypass generation validation or current eligibility.
Exit: scope-safe validated immutable reuse reduces repeat work with bounded lifetime.
Likely scope: existing knowledge reader/request context and focused tests.
Verify: generation/policy movement, historical eligibility, replica changes, query counts and bounded repeated-request memory.

## Workspace graph

### [x] S05a — Versioned multi-project qualification corpus
Depends: existing S02/S25a evidence; define early, before S05–S08 implementation.
Invariant: the same small immutable corpus exercises complete automatic cross-project resolution.
Exit: versioned sources/tasks/expected evidence cover distinct provider versions, ambiguity/unresolved cases and scope-blocked relationships.
Likely scope: existing workspace fixtures and corpus manifest; no storage/schema implementation.
Verify: exact source/version identities and expected positive/negative resolution independently of a preloaded graph or reviewed artifact pilot.
Handoff: [report](../../docs/s05a-workspace-corpus.md) and [receipt](../../docs/s05a-workspace-corpus.json). Signed source/test savepoint `73b20702cf62f0dab3ecc4a748dd162cb01f4b37` freezes four member versions across three logical projects, eight files, seven tasks and eleven exact citations. Three integrity/ingestion checks, a retained-copy rerun, eight existing pilot regressions, scoped Clippy and treefmt pass; a same-size source mutation is rejected. The independent oracle covers exact versions, ambiguity, missing/mismatched providers and denied/authorized scope controls. This closes source/oracle preparation only; automatic persisted resolution remains S06–S08. S01b's allocation/physical-storage gate still precedes S05.

### [ ] S05 — Identity and evidence contracts
Depends: S03b.1 contract findings, S04a, S05a and resolved S01b gate. S03b.2/S04b schema-dependent work follows this contract.
Invariant: new workspace/evidence identities are versioned without changing legacy digests.
Exit: S05.1/S05.2 define and verify the independent identity/validation contracts; S05.3 separately qualifies the recorded multi-snapshot schema migration. Downstream work must identify the applicable completed contract rather than treating an exact build row or digest as admission proof.
S04a.2 finding: current TypeDB `graph-build` ownership of `snapshot-id` has implicit `@card(0..1)`; a two-snapshot publication fails. Resolve the multi-snapshot contract and migration here after S01b; S04a.2 qualifies supported zero/one-snapshot reads. See its [report](../../docs/s04a2-graph-header-batching.md), `green.log` and baseline schema evidence.

### [ ] S05.1 — Versioned workspace/evidence identity contract
Depends: S05's prerequisites.
Invariant: exact member/provider builds, bridge identities and evidence references are deterministic without changing legacy identities.
Exit: documented old/new contracts, fixed legacy vectors and deterministic new vectors reject conflicting versions before schema implementation.
Likely scope: existing identity/evidence types, contract documentation and one focused fixed-vector test target.

### [ ] S05.2 — Complete-generation admission and replica-payload contract
Depends: S03b.1, S05a and resolved S01b.
Invariant: independently verifiable admission binds complete records, assessments, source references, lifecycle dependencies, coverage and exact payloads before targeted projection.
Exit: versioned validation/admission and replica payload/projection-code bindings preserve current-policy historical eligibility, legacy envelopes/exports and existing signed-import boundaries. An event-set digest does not certify serialized view bytes.
Likely scope: contract types/documentation and focused adversarial vectors; implementation of targeted storage/reuse remains S03b.2/S04b.
Verify: reject corruption outside the selected result, missing/asymmetric references and incompatible replica payload/version movement; preserve current-policy denial, absent-history behavior and explicit degraded fallback. No knowledge cache is authorized by defining this contract.

### [ ] S05.3 — Multi-snapshot schema compatibility/migration
Depends: S05.1 and resolved S01b; separate from contract-definition sessions.
Invariant: supported multi-snapshot publication preserves old identities, exact fingerprints, sealed evidence and historical readability.
Exit: fresh and copied disposable TypeDB databases migrate and publish zero/one/multiple-snapshot builds with explicit old/new compatibility.
Likely scope: existing schema/migration owner and one required-server integration target.
Verify: idempotent migration, interruption/retry, historical reads and exact membership/fingerprint preservation; no live database migration in this workcard.

### [ ] S06 — Persisted workspace views
Depends: S05.
Invariant: exact workspace membership can be reconstructed solely from persisted TypeDB state.
Exit: publish/read a version-bound view with complete membership validation and scope isolation.
Likely scope: existing workspace engine, TypeDB view storage and one integration test target.
Verify: fresh-process reconstruction, absent/stale member rejection and interruption-safe visibility.

### [ ] S07 — Automatic bridge resolution
Depends: S06.
Invariant: cross-project edges require compiler/package/configuration evidence, not name similarity.
Exit: supported bridge families resolve exact provider versions; unresolved/ambiguous coverage remains explicit.
Likely scope: existing extraction/workspace resolution engines and focused bridge fixtures.
Verify: automatic positive bridge, two provider versions, ambiguity and blocked/unresolved bridge; split by family if necessary.

### [ ] S08 — Workspace MCP end-to-end
Depends: S07.
Invariant: normal MCP traversal uses persisted automatic TypeDB relationships.
Exit: fresh-process source-backed traversal across projects, distinct provider versions and a blocked bridge.
Likely scope: workspace MCP integration and required-server end-to-end tests.
Verify: no reviewed workspace-artifact input and no preloaded memory graph; exact evidence, scope and omission assertions.

## Session custody

### [ ] S09a — Native runtime feasibility
Depends: S01b/S25a running-runtime findings; advance early, before the workspace phase completes.
Invariant: the proposed capture cursor/lifecycle works in the actual supported runtime.
Exit: exact runtime/adapter capability matrix and a runnable native lifecycle proof.
Likely scope: existing OpenCode extension/Chaosbox adapter tests and compatibility documentation.
Verify: actual observed and intended runtime/package/adapter/config identities, supported/missing hook behavior, checkpoint/model-bound transmitted context, complete settlement and interruption/cursor recovery in isolation. Capability markers alone are insufficient.
Handoff: [native capability report](../../docs/s09a-native-custody.md) and [receipt](../../docs/s09a-native-custody.json). The intended production `2.0.22+047c74e` package passes packaged-adapter checkpoint transmission, complete settlement, interrupted custody and restart/Unicode recovery (r10). The actual listener's `2.0.12+f18083c78e` package fails explicitly for missing `compaction.plan` in the same isolated packaged fixture (r11). Native core typing and the Promise-host regression pass against clean `db48c4233c` (1 test, 17 assertions). Live Jev assessment/publication is blocked by HTTP 402. The other worker has committed the cancellation patch; the current immutable consumer build, qualified producer pins and full profile remain pending. Parent S09a/S25b remain open; no serving transition or activation occurred.

Later S09a evidence: the complete five-patch production package also passes the isolated lifecycle and injected-failure cleanup proofs. The [dependency-release blocker](../../docs/s09a-native-custody.md#dependency-release-qualification-blocker) remains distinct: refresh the owning published Pklr repair, then qualify/publish Fleetix and qualify Toolbelt before a fresh exact Canix reconciliation. Toolbelt's successful Nix matrix did not erase its failed dependency audit. Current immutable consumer gates and enabled live assessment/publication remain unqualified; the latter independently needs provider credits.

### [ ] S09b — Journal compatibility and recovery
Depends: S09a, S05.
Invariant: journal evolution preserves every original record across supported versions.
Exit: copied legacy archives migrate and restore through interruption, with explicit old/new tuple support.
Likely scope: custody journal migration and one native recovery test target.
Verify: original hashes, interrupted migration/restart and old/new adapter/runtime compatibility; never test against the live archive.

### [ ] S10 — Incremental capture
Depends: S09b.
Invariant: deltas preserve the same source occurrences and variants as complete capture.
Exit: changed/appended records reconcile without full-history duplication; failed custody leaves native records recoverable.
Likely scope: `src/compaction/` capture/journal and focused native-capture tests.
Verify: repeated/updated/late-settled records, restart replay and byte/hash parity against full capture.

### [ ] S11 — Native adapter delta capture
Depends: S10.
Invariant: real lifecycle events produce complete settled deltas without per-event full-history serialization.
Exit: supported native runtime drives delta capture and recovery correctly.
Likely scope: `plugins/chaosbox-compaction/` and native-host integration tests.
Verify: event loss, ordering, tool settlement, final assistant reply and restart reconciliation.

### [ ] S12 — Bounded deferred workers
Depends: S11.
Invariant: background assessment/publication cannot block or invalidate source custody.
Exit: queue/concurrency/retry/spending bounds survive restart with the pinned Jev policy.
Likely scope: existing custody worker/outbox and focused worker tests.
Verify: slow/failing provider, repeated retry, cumulative budgets, lease release and successful outbox reconciliation.

### [ ] S13 — Exact archive retrieval
Depends: S10; integrate S11/S12 where needed.
Invariant: bounded retrieval reconstructs exact archived source fields and attachment bytes.
Exit: digest/pointer paging, scope and visible incompleteness are preserved.
Likely scope: archive/evidence API and MCP retrieval tests.
Verify: Unicode offsets, page boundaries, missing/tampered objects, attachments and fresh-process recovery.

## Incremental indexing and publication

### [ ] S14 — Immutable extraction units
Depends: S05, S08.
Invariant: reuse keys bind content, configuration, producer and evidence provenance.
Exit: immutable units distinguish incompatible inputs without changing published legacy identity.
Likely scope: extraction identity/reuse engine and focused fixed-vector tests.
Verify: content/config/toolchain changes, cross-repository isolation and deterministic unit identity.

### [ ] S15 — Unchanged-unit reuse
Depends: S14.
Invariant: reusing unchanged extraction units preserves complete facts and coverage.
Exit: unchanged work is skipped and measured; full output matches fresh extraction.
Likely scope: existing reuse/pipeline engine and focused corpus tests.
Verify: zero-change replay, partial edits, absent cache and complete-export/coverage parity.

### [ ] S16a — Dependency invalidation
Depends: S15, S07.
Invariant: edits, deletion, configuration and provider movement invalidate every affected derived unit.
Exit: exact dependency/version changes produce neither stale bridges nor unrelated invalidation.
Likely scope: dependency/reuse invalidation and focused adversarial fixtures.
Verify: delete/rename/config changes, provider-version replacement and unresolved-to-resolved transitions.

### [ ] S16b — Incremental assembly parity
Depends: S16a.
Invariant: assembled incremental state equals a complete fresh build for the same inputs.
Exit: bounded assembly preserves graph/evidence identity and honest coverage across replay/restart.
Likely scope: pipeline/materialization assembly and one parity test target.
Verify: edit sequences, interruption, fresh/incremental complete exports and publication digest comparison.

### [ ] S17 — Streaming validation and sealing
Depends: S16b, S05.
Invariant: bounded sealing validates complete membership/evidence under the predeclared contract.
Exit: streaming removes redundant full copies without weakening any publication guard.
Likely scope: store sealing/validation and fixed-identity/regression tests.
Verify: duplicate/missing/conflicting members, historical seals, digest parity and isolated memory evidence.

### [ ] S18 — Resumable TypeDB publication
Depends: S17.
Invariant: partial publication is invisible and recovery cannot overwrite a rival publication.
Exit: durable bounded publication checkpoints reconcile crashes and uncertain commits.
Likely scope: existing TypeDB publication engine and required-server recovery tests.
Verify: interruption at durable boundaries, exact retry, predecessor conflict and fresh-reader visibility.

### [ ] S19 — Shared evidence payloads
Depends: S18, S05, S13.
Invariant: evidence sharing changes storage/copy cost while preserving source and sealed-history meaning.
Exit: implement S05's versioned references with legacy reading/restoration intact.
Likely scope: evidence representation/storage adapter and compatibility tests.
Verify: shared-object accounting, missing/tampered references, historical exports and exact source recovery.

## Normal-session context and resource policy

### [ ] S20 — Single context assembler
Depends: S08, S12, S13, S19 and completed S04 for full integration.
Invariant: one bounded assembler combines working state, graph and admitted knowledge with exact provenance.
Exit: required state survives budgeting and every omission has a recoverable source reference.
Likely scope: existing continuation/context engines and task-context tests.
Verify: protected requirements, contradictory/stale evidence, graph traversal and deterministic overflow behavior.
Early investigation: audit whether existing references and continuation planning support an isolated initial context proof before S19. Record the actual dependency; split out a bounded proof if justified. Full integration/qualification still covers shared evidence and the joint S23/S24 tuple.

### [ ] S21 — Native OpenCode consumption
Depends: S20, S11.
Invariant: actual native requests consume the verified assembled context/checkpoint.
Exit: manual/automatic/overflow paths work on the documented runtime/adapter tuple.
Likely scope: native adapter/runtime integration and its verifier.
Verify: exact transmitted context, missing-hook behavior, restart, source recovery and supported compatibility matrix.

### [ ] S22a — Client resource policy
Depends: S21, resource criteria agreed from repaired-path measurements.
Invariant: workers, queues and caches remain bounded while preserving correctness and progress.
Exit: measured admission/concurrency/cache behavior respects the agreed client limits.
Likely scope: existing worker/request controls and resource-policy tests.
Verify: contention, cancellation, pressure, scope-safe reuse and bounded repeated-request growth.

### [ ] S22b — TypeDB resource policy
Depends: S22a, resolved S01b allocation/history evidence.
Invariant: server/history/cache policy reduces measured costs while retaining required data and recovery.
Exit: justified settings and lifecycle behavior pass on disposable/copied data before any live rollout scope.
Likely scope: existing TypeDB configuration/lifecycle ownership and focused qualification evidence.
Verify: warm working set, retained history, interrupted recovery and complete source/graph/evidence preservation.

## Final qualification and consumption

### [ ] S23 — Resource qualification
Depends: S21, S22a, S22b; candidate tuple frozen jointly with S24.
Invariant: accepted resource claims cover the complete declared corpus and actual production path.
Exit: full selected source inventory, representative million-fact/full-workspace measurements and agreed limits pass.
Likely scope: existing benchmark/receipt facilities and qualification report.
Verify: exact source/runtime/adapter/schema/config/corpus/tasks/model tuple, steady/indexing peaks, swap, I/O, latency and no omitted repositories/facts.

### [ ] S24 — Session-quality qualification
Depends: S21, S22a, S22b; same immutable tuple as S23.
Invariant: resource savings preserve required task outcomes and original-source recovery.
Exit: pinned coding/workspace/custody tasks demonstrate answer quality, recoverability and context efficiency.
Likely scope: existing task evaluation facilities and quality report.
Verify: model/task identity, source citations, cross-project answers, requirements recovery and failures; invalidate S23/S24 on relevant changes.

### [ ] S25 — Exact release and Canix consumption
Depends: S23, S24 and explicit candidate release/consumer scope.
Invariant: Canix consumes the qualified published bytes and compatible runtime tuple.
Exit: required exact upstream checks, consumer compatibility repairs, guarded pin reconciliation and direnv evidence pass.
Likely scope: scoped upstream release plus Canix declaration/lock/compatibility surfaces; split release and consumer work if too broad.
Verify: exact hosted candidate completion, published outputs, native sidecars, scoped staging and consumer checks; preserve earlier Pkl blockers until actually repaired.

### [x] S25a — Installed executable/source attribution
Invariant: the observed managed MCP/runtime chain is bound to source-backed executable evidence.
Handoff: [report](../../docs/s25a-installed-provenance.md) and [receipt](../../docs/s25a-installed-provenance.json). Operator derivation excerpt binds the installed output to its filtered source; all 151 files match archived `e6698a34c380b11c0216f3157a548868b7823f79`. Pre-fix CLI/MCP export calls explain the 10,000-node failure.
Boundary: full derivation/build flags and a unique original Git revision remain unproven. No native custody qualification or consumer promotion is implied.

### [ ] S25b — Early memory-only Canix consumer qualification
Depends: S02/S25a, qualified published memory-only revisions and the existing scoped consumer authorization; independent of broader architecture release. S09a is an explicit gate for native acceptance; pin/Pkl preparation may proceed earlier.
Invariant: the exact enabled Atlas native runtime/adapter/custody tuple consumes qualified published bytes.
Exit: Colibri retirement preserves GGUF inference/fetch validation and Atlas lifecycle/health policy; current native Pkl constraints/registered sidecars, fresh guarded pin reconciliation and explicit direnv preparation evidence pass.
Likely scope: Canix Pkl compatibility and selected declaration/lock updates, then existing native qualification gates.
Verify: actual model-bound context/checkpoint, settlement and interruption/recovery; copied-archive compatibility when formats change. Qualify compaction/live assessment/publication as enabled in the declared tuple. The observed serving configuration has no compaction adapter entry and does not establish this result.
Handoff: then perform the previously authorized bounded non-activating Atlas production build with exact derivation/output/cache/log evidence. This does not close S23/S24 or broader S25/S26. Live MCP acceptance needs a separately authorized consumer transition and fresh executable-chain verification; do not merge the two draft upstream PRs as a shortcut.

Release unblock: refresh the owning Pklr registry release and exact required CI, then qualify/publish Fleetix, adapt/requalify Toolbelt and regenerate the revision-bound Canix reconciliation request. Preserve the complete five-patch isolated package evidence while qualifying the current immutable consumer tuple. Resolve the independent HTTP-402 assessment blocker when live assessment is enabled. A candidate containing the reviewed ordinary MCP/reader changes also requires S02c.4/S02b.4; the earlier memory-only candidate retains its own exact scope and authorization.

### [ ] S26 — Non-activating Atlas candidate
Depends: S25 and non-activating build authorization.
Invariant: actual Atlas production outputs match the qualified source/runtime inputs.
Exit: bounded Canix build/private publication succeeds with exact derivation/output/log evidence and no activation.
Likely scope: Canix qualification receipts; source fixes only when a concrete gate failure requires them.
Verify: canonical checkout/pins, approved environment, actual production output and cache evidence. Activation is not part of this workcard.
