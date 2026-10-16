# S02b.3 — Ordinary CLI/MCP neighborhood pages

Status: locally qualified, 2026-10-16. Parent S02b's ordinary-client gate passes.

Both `query neighbors` and the ordinary MCP `neighbors` tool now prepare the
shared S02b.2 request before acquiring backend credentials. The private connection
helper returns an unpinned handle; preparation selects active for first pages or
the exact historical publication on continuation. The old ordinary unbounded
neighborhood dispatch is removed.

## Client contract

- Preserve tool/command names, `id`, `outgoing`, `incoming` and scalar `rel`.
- Add `limit` (default 20, range 1..200 **per selected direction**), `cursor`, and
  `direction` (`both` by default, `outgoing`, or `incoming`).
- Add `rel_types` for lists; `rel` and `rel_types` are mutually exclusive. An
  omitted filter means all types; an explicitly empty list matches nothing.
- Expose the shared version/repository/build/generation, directional completion,
  overall `complete`, and `next_cursor` fields.
- Follow `next_cursor` with the same repository, anchor, direction, page size and
  canonical filter until `complete` is true. Normalized equivalent filters work.
- MCP rejects unknown arguments, malformed supplied types, invalid bounds and
  cursor/scope mismatches with `-32602` before acquiring credentials. Unavailable
  historical publications fail explicitly with `-32603`; CLI failures exit nonzero.

The old array fields remain, but the default response is now a bounded first
page. Consumers needing a complete neighborhood must follow continuation.
There is no implicit auto-drain into an unbounded response.

For example, retain the returned opaque cursor as `$cursor` and repeat:

```nu
chaosbox query neighbors ent:example --repo example --limit 20
chaosbox query neighbors ent:example --repo example --limit 20 --cursor $cursor
```

CLI list filters use `--rel-types calls,references`; `--rel-types` with no values
selects an empty filter. MCP uses `"rel_types": ["calls", "references"]` or `[]`.
The cursor retains S02b.2's repository-level domain and remains untrusted query
state rather than an authorization credential. Scoped-view admission, evidence
validation and its separate cursor contract are preserved.

## Verification

| Gate | Result |
| --- | --- |
| Initial red | Missing advertised paging schema; invalid arguments reached credential lookup |
| Final affected suites | 91 passed; four disposable cases ignored in the aggregate command |
| Required ordinary-client TypeDB case | 1 passed, zero ignored; 26.08 seconds |
| Required existing CLI/MCP explanation regression | 1 passed, zero ignored; 11.86 seconds |
| Scoped warnings-denied Clippy | Passed after splitting a long test function, without a lint waiver |
| Scoped treefmt / Git whitespace | Passed |

The affected suites cover library (44), binary/MCP (11), count-reference (2),
client argument validation (1), continuation (5), page-reference (1), scoped views
(20), and navigation (7). The existing high-degree fixture is shared between
backend and client tests; its identity and complete reference observations remain
unchanged. Added reference coverage advances both directional positions and tests
incoming-longer exhaustion with unbounded collection APIs forbidden.

The required TypeDB CE 3.13.0 test runs on `127.0.0.1:24729`, fresh database
`s02b3_clients_20261016`. Its fixture contains 1,026 nodes / 2,056 relationships,
with 1,029 hub rows in each direction. First pages pin `build:5eff79a93a12`,
generation 2. After publishing the empty hub neighborhood `build:9d87aa123524`,
generation 3, all continuations retain the old build and new first pages use the
new one.

Every response agrees across **fresh CLI, fresh MCP and one persistent MCP
process**. Each transport enumerates 29 pages, 4,124 delivered rows and 597,217
page-JSON bytes across nine cases. These totals include overlapping filters and
directions; self-loops appear once per selected direction. They exclude the
default/new-first-page, schema and error requests, and JSON-RPC framing. They are
not backend query or scanned-row measurements. Full equality checks every
relationship ID/type/direction/endpoint, with independent endpoint membership.
Cases cover simultaneous continuation, directional reads, exact-size completion,
three sparse late filter matches, scalar/list normalization, empty filters and
missing/foreign anchors. Both clients reject real wrong-scope and unavailable-build
cursors. The refactored connection helper also passes the existing explanation
test in fresh database `s02a_s02b3_regression_20261016` (three CLI cases and nine
calls through one MCP process).

The native live worker records 26.198 seconds / 122.7M peak with compilation warm.
Recorded disposable-server peak is 201,072,640 bytes, with zero current swap.
Workers retain 4 GiB memory ceilings, zero swap, two Cargo jobs and two CPU cores.
These fixture observations do not establish production limits or long-running
retention acceptance. Whole-request/path budgets and ordinary transport
framing/cancellation remain S02c; scoped transport cancellation tests do not prove
ordinary transport cancellation.

## Provenance and preservation

Evidence: `/data/scratch/tmp/opencode/chaosbox-s02b3-20261016/`.
All 27 checksums in `qualification.sha256` verify. Manifest SHA-256:
`6721e8bc2dfdad3dc9aa5e17916eb5a5e7dac28f91611bb3d6317d360f06e768`.
Tested-source archive SHA-256:
`99bb0eed547986aeee0cec2866a2c37b70eae0940a52991d0212f389a63b066a`.
Exact CLI/test executables and native command/resource logs are retained.

Qualification uses the dirty canonical checkout based on S02b.2 savepoint
`436f830283967509eee0a5d0093f3fbad327e614`; the archive includes unrelated changes.
This is not clean-release qualification of the twelve-file scoped savepoint alone.
The previously approved unchanged rooted environment was reused; normal direnv
preparation remains unverified, with no refresh or admission-policy change.
Canix staging, the foreign tracked patch and production TypeDB PID/start identity
compare unchanged. The disposable server was stopped with SIGTERM; data/logs remain.
Installed-consumer transition and broader release/resource acceptance remain open.
