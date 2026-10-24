# S02b.2 — Scope-bound neighborhood continuation

Status: shared repository-reader layer implemented and locally qualified, 2026-10-16.

`NeighborhoodQuery::prepare()` validates requests before a transport acquires
credentials or connects. `PreparedNeighborhood::read(handle)` selects the active
publication for a first page, or resolves the exact recorded historical build
through `published_build(repo, build)` on every continuation. There is no active
pointer lookup or fallback on resume. `GraphReader::published` exposes the same
exact-publication route; `GraphReader::neighborhood_page` is the convenience entry
for callers that already own a handle.

## Contract

- Preserve `id`, `outgoing` and `incoming`; add exact repository/build/generation,
  direction mode, directional completion, overall completion and `next_cursor`.
- `both` is the default mode. The two positions share one build and one cursor;
  an exhausted or unrequested direction stays empty without another page query.
- Case-insensitive filters canonicalize to sorted distinct vocabulary names.
  Omitted and explicit-all filters are equivalent; an empty list matches nothing.
- Each selected unfinished direction requests at most `limit + 1` rows from
  S02b.1, delivers at most `limit`, and resumes after the last delivered row.
  A final exact-size page is complete when no sentinel exists.
- The closed cursor binds contract `chaosbox.repository-neighborhood`, version 1,
  order `rel_id_asc`, repository, build, anchor, direction mode, canonical filter,
  page size and both positions. Wrong scope/version/order/domain, unknown fields,
  malformed tokens and impossible position combinations fail explicitly.
- Page size is 1..200 per direction; labels are at most 256 bytes and serialized
  requests/cursors are bounded at 16 KiB. These reuse existing scoped-reader
  ceilings; they are interface bounds, not newly accepted production RAM targets.

This is an **ordinary repository-level** read contract. The cursor is unsigned,
untrusted query state, not a credential or proof of authenticated server issuance.
Repository ownership is rechecked against the caller-selected backend on every
resume; the caller remains responsible for confining that backend/repository.
No connector authorization identity exists in this ordinary route. Scoped read
views retain their own cursor domain, admission identity, policy/revocation and
complete source/evidence validation. This slice does not migrate scoped reads to
raw backend pages or grant their access through a repository cursor.

## Verification

| Gate | Result |
| --- | --- |
| Initial red regression | Failed on missing continuation types/method |
| Final library, binary/MCP and affected integration suites | 87 passed, 3 disposable cases ignored in the aggregate command |
| Explicit required disposable TypeDB continuation | 1 passed, 0 ignored; 0.67 seconds |
| Scoped warnings-denied Clippy | Passed |
| Scoped treefmt and Git whitespace check | Passed |

The aggregate suite covers library (44), binary/MCP (9), count-reference (2),
continuation (4), S02b.1 page-reference (1), scoped views (20) and navigation (7).
The S02b.2 disposable case was then explicitly executed with
`CHAOSBOX_REQUIRE_TYPEDB=1`. S02a/S02b.1 live fixtures were not rerun in this slice;
their earlier receipts remain separate evidence.

Reference tests forbid unbounded collection APIs, count header calls to prove
scope rejection before reads, alternate absent/new active pointers on fresh
handles, reject actual foreign and missing publications, and compare every
relationship id/type/endpoint. They cover normalization, empty/one/both-direction
modes, sentinel retention, uneven completion, invalid positions and numeric bounds.

The TypeDB CE 3.13.0 fixture uses fresh database `s02b2_continuation_20261016` at
`127.0.0.1:24729`. Old build `build:8a1da3aa857a` has 10 nodes / 21 relationships
and 19 outgoing / 3 incoming hub rows. After publishing empty-neighborhood build
`build:ac0ed0471189`, every resumed page connects a fresh reader and returns the
complete old-build rows, exactly equal to the reference fixture. Fresh-process
CLI/MCP continuation is the subsequent S02b.3 gate.

Native environment preparation succeeded using the existing approved cache;
the unchanged GC-rooted profile was entered through the previously authorized
read-only local-store route. This is not claimed as fresh Nix evaluation or
realization. Workers used two Cargo jobs/two CPU cores, 4 GiB memory ceilings and
zero swap, without holding evaluation capacity. The live worker's native summary
records 789 ms wall time / 66.8 MiB peak, with compilation already warm. The
disposable server's observed peak was 136,437,760 bytes with zero current swap.
These small-fixture observations do not establish production resource acceptance.

## Evidence and next integration

Evidence root: `/data/scratch/tmp/opencode/chaosbox-s02b2-20261016/`.
All 21 entries in `qualification.sha256` verify; manifest SHA-256:
`7de9ae654801708b29466614b850f30004c31ad34bf10f6c17c46af1194d87d7`.
The exact tested source archive has SHA-256
`e82f24dcb95d9ab63a51adfe7b941af57ff57a4164e2e5f34425f8185b94b993`
and compares byte-for-byte with the checkout after qualification. The exact test
executable, commands, red/final logs and lifecycle observations are retained.

Qualification used the dirty canonical checkout based on signed S02b.1 commit
`46ab2034331116e14414c585148784a831ca8646`. Its complete source archive includes
unrelated changes; the scoped savepoint contains only the three production files,
new regression and report/receipt. This is not clean-release qualification of
that savepoint alone. Canix staging and production TypeDB PID/start identity
compare unchanged. The disposable server was stopped with SIGTERM; data/logs remain.

S02b.3 must construct/prepare this query **before** the ordinary MCP dispatch's
current unconditional `AnyReader::connect(repo)`, acquire a `TypeDbReader`, then
call the prepared read. Extend the advertised CLI/MCP schema and verify fresh
processes, exact relationship identities and historical continuation. Ordinary
`neighbors` remains on its existing route until that integration; parent S02b
remains open.
