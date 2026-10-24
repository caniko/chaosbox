# S02b.4 — exact-publication follow-ups

Status: locally qualified, 2026-10-18. Signed implementation/test savepoint:
`4fbdca424a2e8408eedbdc2df0b995800ee8523c`.
The [receipt](s02b4-historical-followups.json) binds the tested source and evidence.

Ordinary clients can hydrate and recursively expand an endpoint returned by an
old neighborhood after the repository's active pointer moves. The implementation
reuses `GraphReader::published`, including its existing repository/publication
ownership checks. Selection grants no scoped-view authorization.

## Request contract

MCP `lookup`, `explain`, `evidence`, and `neighbors` accept optional `build_id`.
The existing CLI `query lookup`, `query explain`, and `query neighbors` accept
`--build-id`.

| Selection | Resolution |
|---|---|
| No selector or cursor | Current publication at dispatch |
| Explicit build, no cursor | Exact published build owned by the requested repository |
| Cursor, omitted selector | Cursor's publication |
| Cursor, matching selector | Cursor's publication |
| Cursor, conflicting selector | Invalid arguments before credential acquisition |

Supplied MCP selectors must be strings. Selectors are opaque, nonblank,
at most 256 bytes, and contain no control characters. Validation does not trim
or rewrite identities. Malformed selectors and cursor conflicts return MCP
`-32602`; unavailable/foreign/unpublished publications fail through the existing
backend error path without consulting current as a fallback. Unsupported ordinary
tools reject a supplied `build_id` rather than silently ignoring it.

`NeighborhoodQuery::prepare` removes the selector before binding traversal scope.
Publication identity stays in the cursor's existing top-level `build_id`, so the
version-1 cursor wire format remains compatible. A cursor still binds its anchor,
directions, canonical relation filter, limit, order, and repository. A new anchor
uses the returned page's `build_id` with no cursor.

```sh
chaosbox query lookup ent:example --repo example --build-id build:example
chaosbox query explain ent:example --repo example --build-id build:example
chaosbox query neighbors ent:example --repo example --build-id build:example --limit 20
```

An MCP evidence follow-up has arguments such as
`{"repo":"example","rel":"rel:example","build_id":"build:example"}`.
Existing default result shapes and count-only explanations are preserved.

## Behavioral proof

The native regression first runs against the retained S02c.4 executable. An
exact historical lookup returns `null` instead of its independently expected
entity (`red-hydration.log`). An earlier retained run also exposes the CLI's
missing flag. Preparation regressions separately fail on the old credential
lookup and unknown neighborhood field, then pass after the fix.

The final required-server fixture advances from `build:157daab9acb0` to
`build:aaf20ac77017`, removing the old leaf endpoints from current. Fresh CLI,
fresh MCP, and persistent MCP then establish:

- Omitted selection retains current behavior, returning `null` for the old
  entity/explanation and empty evidence for the old relationship.
- Explicit selection returns the historical entity and exact count-only
  explanation: one outgoing and two incoming relationships.
- Both MCP modes return a known nonempty sealed evidence row, compared against
  fixture source bytes rather than another reader. Its quote, producer, file,
  snapshot, full-file span, and source hash are checked. The hash is
  `633fe20a6587744cc5ba2776ea2f17280346bcd18eec247ac8ef21cf72e39ff8`,
  using Chaosbox's existing trailing-separator convention.
- Two successive fresh-anchor expansions preserve the old publication and
  independently expected complete directional rows.
- A cursor produced by the retained pre-change executable resumes with omitted
  or matching selection. An explicit historical first page is byte-identical
  in its cursor to that legacy response. Conflicting selection fails.
- Missing and foreign builds fail through each applicable client surface even
  when current contains the requested endpoint. An instrumented page backend
  forbids active-pointer reads on exact starts and continuations. A separate
  native backend case proves a persisted `staging` header is unavailable while
  a valid active pointer exists.

## Qualification

**157 affected tests passed.** Fourteen opt-in cases are ignored in the aggregate;
eight were selected and passed explicitly, alongside two required native backend
cases: **ten required disposable-TypeDB cases passed, zero ignored**. Six other
aggregate opt-in cases and 18 other backend live cases were not selected.
Warnings-denied scoped Clippy, treefmt CI, and whitespace checks pass. Initial
Clippy findings in the test code were corrected with a focused expansion helper
and `String::new()`, without adding lint suppressions. The first test-filter
attempt selected zero cases and is retained separately from the actual failing
and passing runs.

The final candidate also passes S02c.4 admission/credential refresh, native
cleanup, successful/error-eviction retention, connection reuse/idle expiry,
header batching, ordinary paths, historical pages, explanations, and backend
conformance. Native projections retain the exact observations:

- Eight queued calls select the replacement publication at dispatch; changed
  queued credentials are rejected. Seven eight-call bursts retain one driver,
  with RSS samples from 32,504 to 32,532 KiB, stable threads/FDs, and zero swap.
- Twenty lookups retain one connection, 40 transactions, and 60 queries. The
  batching baseline uses 60 transactions; the pre-reuse baseline opens 20
  connections. The idle sample at 31,153 ms has zero TCP sockets.
- Active cancellation, EOF, oversized input, closed output, and output overload
  release observed native reads. Wide-response EOF/deadline cleanup and the
  147-call error-eviction fixture pass.
- Each transport passes 14 ordinary path cases and 29 historical neighborhood
  pages delivering 4,124 rows.

Workers used the harness-approved project environment, two Cargo jobs/two Tokio
workers, 200% CPU, `MemoryHigh=2G`, `MemoryMax=4G`, and `MemorySwapMax=0`.
The disposable TypeDB CE 3.13.0 graph endpoint was `127.0.0.1:24779`; metrics
were read through `127.0.0.1:24781`. The server peaked at 446,644,224 bytes with
zero swap. Final diagnostics report zero active
transactions in every disposable database. These fixture measurements do not
establish production resource acceptance.

## Provenance and preservation

Evidence root: `/data/scratch/tmp/opencode/chaosbox-s02b4-20261018/`.
`commands.txt` records reproduction; `qualification.sha256` covers frozen logs,
native projections, binaries, source, preservation evidence, and report/roadmap
snapshots. `finalization.json` binds the manifest and local savepoints separately.

- Baseline HEAD: `8b30ab76d4a151adf9298b08aac451d0e1d9d854`.
- Tested source archive SHA-256:
  `3eca0d3092a187791e50947c161f377b8fa3ed99ac7af4a25c1a20145967708f`.
- Tested CLI SHA-256:
  `d45394c3e7c414390526c6ce6ed0967a784067c2ef08cbacc762148cb79ce1d3`.
- Legacy cursor producer SHA-256:
  `e68041d16fa7bf7f58bc47645db025b96449149d5324c554eeca09bb1474ff9c`.

The Git-filtered source archive includes preserved foreign dirty/untracked source;
`docs/` and `tasks/` are retained separately. This is local dirty-source
qualification. The staged and committed implementation patches match exactly;
the tested source inventory still matches after the savepoint. Only the owned
export hunk in `lib.rs` was staged alongside the ten otherwise-clean code/test
paths. The existing foreign export and other edits are preserved. Configured
author/committer and signature were verified.

The disposable server stopped with SIGTERM, its listeners are gone, and
production TypeDB's PID/start identity is unchanged. No consumer promotion,
publication, host activation, or production restart occurred.

## Next work

S02b.4 closes the historical follow-up promotion gate for this locally qualified
reader/MCP candidate. Resolve S01b allocation/history attribution and freeze
S05a's independent multi-project corpus before the applicable S05 identity,
complete-generation admission, and schema-migration slices. S03b.2/S04b remain
behind those contracts. S09a/S25b retain their exact release/runtime/consumer
gates and the Pklr → Fleetix → Toolbelt → Canix published-dependency chain.
