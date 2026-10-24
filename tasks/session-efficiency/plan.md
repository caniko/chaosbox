# Session efficiency and complete workspace traversal

Status: revised execution plan, 2026-10-19. Task checkboxes and individual
workcards are in [todo.md](todo.md). This plan is separate from the unfinished
compiler/cutover plan in `tasks/plan.md` and `tasks/todo.md`.

## Objective and fixed contracts

Make ordinary OpenCode sessions cheaper in RAM, I/O and model context while
preserving complete source-backed discovery and cross-project traversal through
Chaosbox MCP. Keep TypeDB authoritative for graph/admitted-knowledge queries and
the existing SQLite custody engine authoritative for original session records.
Reuse existing engines before introducing new components.

Preserve facts, selected repositories, deterministic legacy identities, exact
source evidence, scope boundaries, publication validation and recovery. A smaller
returned context may be bounded; it must not conceal a smaller indexed corpus.
Do not add a mandatory graph/vector/cache backend. New architecture release,
consumer pin changes and activation are separate task scopes, not consequences of
creating this plan. S26 is explicitly non-activating.

## Starting evidence

- [S01](../../docs/s01-efficiency-baseline.md): complete offline export parity;
  candidate peak RSS 340,792 KiB versus 754,616 KiB; reproducible Canix context
  failure at the 10,000-node export cap.
- [S01b](../../docs/s01b-resource-attribution.md): accessible observations collected;
  allocator/history attribution is incomplete. The operator deferred numeric
  limits until successful repaired bounded reads have been measured. The running
  OpenCode backend differs from PATH resolution. No absolute server RAM cap is
  justified yet.
- [S02](../../docs/s02-bounded-navigation.md): existing bounded source and published
   candidate qualify through CLI/fresh MCP on the recorded Canix graph. Complete
   export parity and disposable TypeDB conformance pass. The installed executable
   still fails; S25a attributes its filtered source to a pre-fix tree. Exact
   consumer qualification/promotion remains open.
- [S03a](../../docs/s03a-keyed-membership-audit.md): existing keyed headers and
  server-limited membership reads satisfy the audited materialization contract.
  Extended reference/required-TypeDB conformance passes; no production reader
  rewrite is needed. Server scan/sort cost and large-corpus acceptance remain
   unmeasured. S02a's follow-up narrows the evidence claim to boundary-summary
   parity; it does not establish row-payload or source-span equality.
- [S02a](../../docs/s02a-count-only-explanations.md): count-only TypeDB/reference
  reads and shared CLI/MCP explanations pass the collection-forbidden regression,
  high-degree disposable fixture and required backend conformance. Counts retain
   exact build/type/direction semantics. Signed savepoint: `92d635848ae0b57443a4cebb21cb1ef69b140ffe`.
- [S02b.1](../../docs/s02b1-directional-pages.md): directional backend pages are
  implemented and finally qualified through reference/required-disposable paging,
  affected libraries, existing required conformance, treefmt and scoped Clippy.
  Exact-size and three-page sparse filters pass; tested source/executables/logs are
  frozen. S02b.3 below establishes ordinary-client integration.
- [S02b.2](../../docs/s02b2-neighborhood-continuation.md): shared repository-level
  continuation and exact historical pinning pass reference/required-TypeDB tests,
  affected suites, treefmt and Clippy. One cursor binds the two positions and
  immutable request; ordinary CLI/MCP integration and fresh-process proof are
  established by S02b.3 below. Scoped-view admission and complete evidence validation remain intact.
  Signed local savepoint: `436f830283967509eee0a5d0093f3fbad327e614`.
- [S02b.3](../../docs/s02b3-ordinary-neighborhood-pages.md): ordinary CLI/MCP
  paging passes exact historical enumeration through fresh CLI, fresh MCP and
  persistent MCP. Parent S02b's fixture-backed ordinary-client gate is complete.
  Aggregate request/path cost, ordinary transport controls and installed-consumer
  transition remain open. The recorded native environment is unchanged rooted
  reuse, not verified normal preparation or clean-release qualification.
  Signed local savepoint: `d42a93c04efca667a8b5b05c4b9324465c26e1ed`.
- [S02c.1](../../docs/s02c1-bounded-path-search.md): the shared pinned reader now
  uses directional pages with aggregate local budgets. The retained qualification
  passed 98 tests; two disposable-TypeDB cases were ignored. Signed local
  savepoint: `accc0e5b1289b10c6c51733acc8b11737430c7a1`. S02c.2 owns shared
  argument validation, acquisition-inclusive request budgets and ordinary-client
  TypeDB proof; S02c.3 owns transport lifecycle and retention evidence.
- [S02c.3](../../docs/s02c3-mcp-lifecycle.md): S02c's ordinary-request gate is
  complete across all three slices. Bounded frames/output, matching cancellation,
  native remote-read cleanup and 147 persistent calls pass; 148 affected tests and
  three required disposable cases pass. Signed implementation: `0050c4631a951015247fe4d6ed947ff5ea19b9dd`;
  signed test/report savepoint: `6cce3cc50e5e5f7d88a034880c03322f0cdbeed5`.
  Production/whole-stack resource acceptance remains separate.
- [S04a.1](../../docs/s04a1-graph-connection-reuse.md): ordinary MCP retains one
  idle graph driver, repins each request and evicts on tool errors/cancellation.
  A native 20-lookup comparison opens one connection instead of 20, with 60 queries
  on both sides. Successful-call and failure-eviction retention, idle cleanup and
  historical path/page parity pass; 153 affected tests and five required live
  cases pass. Signed implementation/test savepoint:
  `39d94d6022fa03cd6154dda16e41df69701d8aed`; signed report savepoint:
   `c81b877f05f555be8a7b524c577dd163610debea`. S04a.2 below completes graph batching;
   S03b.1 can investigate knowledge independently.
- [S04a.2](../../docs/s04a2-graph-header-batching.md): the shared publication
  header/fingerprint read uses one request-owned transaction. Twenty lookups open
  40 transactions instead of 60, with 60 queries and one connection on both sides.
  Exact historical pins, complete paths/pages, warmed cleanup, successful-call and
  failure-eviction retention pass; 153 affected tests and seven required live cases
  pass. Signed implementation/test savepoint:
  `ea51f2ed7ae638787e28c679ee4fc30ab44999e6`; signed report savepoint:
  `e89a768bcfdcbf02ff9a69f6989d80ffe841d97d`. S04a's fixture-backed gate is complete;
  S03b.1 below is complete. The existing multiple-snapshot TypeDB publication
  ceiling is an S05 contract/migration follow-up after S01b, not a batching-schema
  change.
- [S03b.1](../../docs/s03b1-knowledge-validation.md): complete-generation hash,
  reference, historical eligibility and replica boundaries are traced; 55 focused
  tests and five required disposable cases pass. No runtime code/schema changed.
  Existing keyed full envelopes remain authoritative; targeted mirror retrieval
  requires S05's independent validation/admission contract and resolved S01b for
  substantial storage work. S02c.4/S02b.4 address the subsequent ordinary-client review;
  S01b/S05a still precede S05 and applicable S03b.2/S04b implementation.
  Signed local report savepoint: `97e9ec7ac6fe449447f78de5d9363d2b37d4603c`.
- [S02c.4](../../docs/s02c4-mcp-admission.md): bounded FIFO admission, typed
  active/queued cancellation, count/byte/wait limits and control-output cleanup
  pass. Queued calls repin and reread credentials at dispatch; 152 affected tests,
  eight required disposable-TypeDB cases, treefmt and scoped Clippy pass. Signed
  implementation/test savepoint: `75198f14c21df448cc14b033b9fbd2a070e781be`;
  signed report savepoint: `8b30ab76d4a151adf9298b08aac451d0e1d9d854`.
  S02b.4 below completes exact-publication hydration and new-anchor historical expansion.
- [S02b.4](../../docs/s02b4-historical-followups.md): optional exact selection
  reuses the published reader across ordinary MCP hydration and fresh-anchor
  neighborhoods, with existing CLI counterparts. Retained version-1 cursors,
  independently expected nonempty source evidence, two successive historical
  expansions and missing/foreign/unpublished rejection pass. 157 affected tests,
  ten required disposable-TypeDB cases, treefmt and warnings-denied Clippy pass;
  admission/lifecycle, driver bounds, batching, pages, paths and explanations are
  requalified. Signed implementation/test savepoint:
   `4fbdca424a2e8408eedbdc2df0b995800ee8523c`. S01b.1/S05a below close the native
   logical-inventory and independent-corpus prerequisites.
- [S01b.1](../../docs/s01b1-retained-build-inventory.md): the native read-only
  credential-file inventory pages retained headers and scalar membership counts.
  94 affected tests and three required disposable-TypeDB cases pass. The live
  Canix page independently finds three retained builds, with generation 3 current,
  in 0.91 seconds at 25,724 KiB client peak RSS. Signed implementation/test
  savepoint: `4704623025ee23a977ee5759043bc73e9c209836`. Physical bytes remain
   unmeasured in that workcard. The [operator follow-up](../../docs/s01b-operator-followup.md)
   now supplies process/filesystem totals: 26.60 GiB anonymous-plus-swap, 28.20 GiB
   Chaosbox disk use and 19.70 GiB WAL. Allocator/cache ownership, per-build payload
   costs and engine-history decomposition remain unresolved; S01b stays open.
   The [disposable profiler route](../../docs/s01b-profiler-route.md) qualifies
   existing Massif/xtmemory tooling with two required native cases and identifies
   an 81.95 MiB persistent all-host process-sampling subtree. The next producer
   workcard is the focused own-PID sampler initialization repair; representative
   production profiling and native allocator/history attribution remain open.
- [S05a](../../docs/s05a-workspace-corpus.md): a frozen synthetic source corpus
  and independent oracle cover four member versions, seven tasks and eleven exact
  citations. Three corpus checks, eight existing pilot regressions and a
  retained-copy rerun pass; same-size source corruption is rejected. Signed
  source/test savepoint: `73b20702cf62f0dab3ecc4a748dd162cb01f4b37`. This completes
  corpus preparation, not automatic resolution. Next: authorized S01b allocation
  and physical-storage attribution before S05.1/S05.2; qualify S05.3 separately.
- [S25a](../../docs/s25a-installed-provenance.md): the managed facade/wrapper/ELF
  chain and actual session-serving runtime are identified. All 151 filtered
  installed-source files match the archived pre-fix revision. Native custody
  compatibility and full independently collected derivation flags remain unproven.
- Canonical source was observed dirty at `accc0e5b1289b10c6c51733acc8b11737430c7a1`.
  S01's measured production candidate is `7313e6e5feaf6f50dad9595d74aeaa691c556a24`.
  These are distinct source identities. Refresh status, relevant diffs, staging
  and HEAD before every implementation session; preserve concurrent work.

## Sequence and checkpoints

The implementation review added two ordinary-client gates before promotion of a
candidate containing the reviewed MCP/reader changes: **S02c.4** bounded FIFO
admission, then **S02b.4** exact-publication follow-up reads and recursive
historical expansion. Both are locally qualified; exact consumer/release gates
remain open. Earlier fixture receipts retain their recorded scope.
Hydrating old endpoints alone is insufficient: clients must also start a new
neighborhood at a returned historical endpoint without selecting current.

S05 is split into **S05.1** workspace/evidence identities, **S05.2** complete
knowledge generation/admission and replica-payload bindings, and **S05.3** the
separate multi-snapshot schema migration. S03b.2/S04b cannot replace full-envelope
validation using identity vectors or an exact build row alone.

1. **S02a → S02b.1 → S02b.2 → S02b.3 → S02c.1 → S02c.2 → S02c.3**, following S02/S03a:
   count-only explanations, directional backend pages, exact historical
   continuation, ordinary CLI/MCP enumeration, then bounded path expansion. Continuations
   bind repository, build, anchor entity, direction, canonical filter, order,
   applicable authorization/view identity and token-contract version; pointer
   movement must not mix generations. Apply filtering before backend paging limits
   and cursor boundaries. S02c.1 bounds the already-pinned search; S02c.2 includes
   acquisition and validates ordinary CLI/MCP arguments before credentials.
   S02c.3 qualifies cancellation, framing/backpressure, cleanup and persistent-MCP
   retention. Measure aggregate queries/rows/bytes and whole-request cost. Row/output
   caps alone do not establish bounded request cost.
2. **S03b.1 and S04a** may proceed independently: investigate whole-generation
   validation/current eligibility before targeted knowledge implementation;
   measure graph connection/query reuse without waiting for knowledge schema
   work. S03b.2/S04b follow the applicable contracts. S01b's remaining
   allocation/history evidence stays open; agree numeric criteria from successful
   repaired reads before final resource qualification.
3. **Early S05a corpus → S05 → S06 → S07 → S08:** freeze a small versioned
    multi-project corpus, fix identity/evidence contracts, persist workspace
    views, resolve evidence-backed bridges and prove fresh-process TypeDB traversal.
    S05a's source/oracle preparation is complete. S01b's allocation/physical-storage
    gate remains required before S05 and substantial schema/storage implementation.
4. **Early S09a → S09b → S10 → S11 → S12 → S13:** prove native compatibility and legacy
   restoration, then incremental custody/capture, deferred workers and exact
   source retrieval. S09a uses both actual and intended runtime identities and
   proves checkpoint/model-bound context, settlement and recovery in isolation.
5. **S14 → S15 → S16a → S16b → S17 → S18 → S19:** immutable units, reuse,
   invalidation/assembly parity, streaming sealing, resumable publication and
   shared evidence using the contract already fixed in S05.
6. **S20 → S21 → S22a → S22b:** audit whether an initial context proof can reuse
   existing references/continuation planning before S19, then one context assembler,
   actual native consumption, measured client policy and measured TypeDB policy.
7. **S23 + S24 → S25 → S26:** resource and quality qualification against the same
   frozen candidate tuple, exact upstream/consumer qualification, non-activating
   Atlas production build.

The previously authorized memory-only consumer path can run earlier as S25b:
repair the concrete Pkl gate, refresh reconciliation requests, record explicit
direnv preparation, and qualify the exact enabled native runtime/adapter/custody
tuple. Its bounded non-activating Atlas build does not close the broader
architecture release or S23/S24 qualification. Live MCP acceptance requires a
separately authorized consumer transition and a fresh executable-chain check.
S09a explicitly gates S25b native acceptance; pin/Pkl preparation can proceed
before the isolated runtime/adapter proof is complete.
The S09a report additionally retains the complete five-patch isolated package
proof and the published-dependency blocker. Refresh the owning Pklr release,
qualify/publish Fleetix and qualify Toolbelt before fresh Canix reconciliation;
do not substitute a path/fork dependency for that release gate. Live assessment
also requires the independently missing provider credits when enabled.

Checkpoint after each phase: focused checks pass, evidence identifies the actual
source/runtime, and no required check is failed, skipped or silently weakened.
Reassess scope using the new measurements before starting the next phase.

## Required corrections to the earlier conversational plan

- S01b happens before substantial storage/schema work; do not defer server
  allocation attribution to S22.
- S08 requires automatic persisted TypeDB bridges/views, a fresh reader process,
  distinct provider versions and a blocked bridge. The reviewed artifact pilot
  and a preloaded memory graph cannot satisfy it.
- S05 fixes versioned evidence references/digests while preserving legacy identity;
  S19 implements sharing under that contract rather than redefining sealed data.
- S09a/S09b require actual runtime feasibility, an explicit compatibility tuple,
  copied legacy archives and interruption/restoration tests before replacement.
- S23/S24 both depend on S21 and S22a/S22b. Their receipts bind the same source,
  running runtime, adapter, schema, config, corpus, tasks and model; relevant
  changes invalidate both receipts.
- S03, S09, S16 and S22 are split with stable suffixes. Further splits retain
   those IDs as parents and identify a single invariant per new workcard.
- Complete traversal requires deterministic neighborhood continuation or another
  bounded route to enumerate every authorized relationship. Error-only
  high-degree reads do not satisfy this contract. Wrong-scope or unavailable-build
  continuation fails explicitly; path-budget exhaustion differs from no path.
- Knowledge reads preserve complete-generation hash/reference validation,
  historical current eligibility and synchronized-replica contracts. Investigate
  these before selecting targeted projections; schema-dependent changes wait
  for S05. Graph-side S04a reuse is independent.

## Session execution contract

Use one workcard per fresh session. Prefer 1–5 changed files and one subsystem
invariant. Trace real callers before choosing the smallest shared fix. If the
work needs more than three independent acceptance criteria or expands across
unrelated subsystems, split it before implementation.

Target 80–160k total context; stop expanding around 200k and hand off by 240k,
leaving output/recovery headroom below 300k. Count instructions, tools, source,
results and conversation. Use actual harness usage telemetry where available;
record unavailable telemetry instead of treating estimates as exact. Bound tool
results and keep large evidence files outside conversation context.

Every handoff records: exact HEAD plus scoped dirty identity, owned files,
commands/results, immutable evidence locations/digests, unresolved issues and the
next workcard. Tests must exercise the owning invariant; do not run the full
workspace after every small edit. Use approved direnv, bounded Cargo jobs and
treefmt. Native workers use two Cargo jobs, a 4 GiB memory ceiling and no swap;
these execution limits are not production resource acceptance. Real TypeDB
acceptance uses the existing disposable server gate with
`CHAOSBOX_REQUIRE_TYPEDB`; a skipped server test is not evidence.

Later workcards are bounded scopes, not permission to guess APIs or rewrite
uninspected code. Resolve their exact focused test names and file set at entry.
The report/receipt pair remains the authority for measured outcomes; a checkbox
alone is not a qualification receipt.
