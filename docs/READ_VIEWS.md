# Run-bound graph read views

## Trust boundary

Paperclip owns authorization. Its trusted host connector resolves company,
project, agent, task, run, execution host, and server policy into a read view.
`ReadView` is an admission contract, **not a bearer credential**. Deserializing a
view or passing a CLI flag does not authenticate a worker. Only a trusted
connector may construct an admitted reader or launch its stdio process. The
worker must not have the connector's configuration, database credentials,
producer credentials, writable admission files, or a direct database route.

The initial reader supports one repository and one explicit published build.
It never follows the active pointer after admission. The existing query backend
supplies a bounded in-memory graph and complete evidence for the connection;
the backend handle is dropped after admission. This introduces no
persisted snapshot format or grant database. A missing recorded version blocks
recovery instead of silently switching to a newer build.

## Version 1 contract

- Required identity: company, project, agent, task, run, execution host, server.
- Required view ID, policy revision, issue time, expiry, repository, build ID,
  exact snapshot set, source roots and exclusions. `.` explicitly grants the
  repository root; an empty source-root list grants nothing.
- A non-empty operation allowlist. Only `status`, `search`, `lookup`,
  `neighbors`, `path`, `explain`, `evidence`, and `export` are supported.
  Cross-build diff and intelligence require further admission contracts.
- Nonzero, ceiling-checked graph, evidence, request, page, response-byte,
  traversal, call-count, and whole-call deadline budgets.
- Tool arguments are strict typed objects. They may narrow the view, never
  select credentials, transports, paths outside the source scope, builds, or
  a different identity. Cursors are bound to the view, operation and arguments.
- Every successful response carries the view/policy/run identity and pinned
  data identity. Workspace observation is separate and defaults to `null`;
  applicability is `unknown`, never inferred from reproducible graph data.
- Every retained relationship requires non-empty evidence and complete, allowed
  source citations. Legacy uncited or out-of-scope evidence blocks admission,
  including when a caller only intended to read neighbors, paths or counts.
  The reader never silently drops contradicting evidence to make a view pass.
- Revocation and cancellation discard in-flight results. Expiry is checked
  before work and before release. Backend errors are redacted at the boundary.

## Lifecycle

Begin with run-isolated MCP connections and conversations. Reconnect and
recovery require new authorization for the recorded versions. Narrower scopes
and different policy revisions require a new conversation; historical model
context is itself data access. A host connector must settle the connection on
completion/cancellation and revoke its reader when policy changes. These are
worker-integration requirements, not guarantees conferred by a JSON document.

Capability discovery must distinguish this reader contract from filesystem
`execution_context` and from operator MCP. A worker lacking a qualified
host-local connector must return a blocked capability result. SSH terminal
selection does not relocate native stdio MCP.

## Qualification

The reader's local gates exercise foreign/missing builds, exact version pinning,
source and transitive-evidence scope, identity mismatch, operation denial,
expiry/revocation, malformed arguments/cursors, pagination, byte and traversal
budgets, and cancellation. Live TypeDB conformance is a separate gate.

Deployment additionally requires Paperclip/Hermes admission, conversation and
checkpoint isolation, A→B→A and scope-narrowing tests, direct shell/endpoint/config
bypass tests, redacted audit, and host-local credential/network isolation.
Graphify retirement also requires source-scope, exclusion, budget, and Jev
consent parity. Reader qualification alone does not authorize retirement or
deployment.

## Connector entrypoint

The library entrypoint is `ScopedReader::admit(backend, view, authenticated_identity)`;
`call` checks the same identity and the controller-held `Revoker` is one-way.
`read_view::mcp::serve` binds one admitted identity to one stdio connection and
advertises only granted operations. List tools return one definition per page;
graph lists return `items` and `next_cursor`. `export` requires `kind: nodes|edges`.
Every result includes a `read_view` envelope. Denials are MCP tool errors with a
redacted `blocked`, `code`, and actionable `message` payload.

For a Linux host connector that supervises subprocesses:

```sh
chaosbox reader --admission /run/connector/approved-run/read-view.json
```

The admission JSON is a serialized version-1 `ReadView`. It must be a regular
file, private to and owned by the connector's process identity, at most 128 KiB.
The connector must keep the file and all parent directories outside worker
writes. The process uses its existing `CHAOSBOX_TYPEDB_*` configuration only
during admission. It loads no Jev credentials and exposes no ingestion tools.

The connector owns the stdio pipe. EOF settles the active call. Removing,
replacing, changing or making the admission file non-private revokes the
connection; the Linux launcher checks at 100 ms intervals. In-process revocation
also interrupts blocked output. Expiry is at most 24 hours after issue time and
is checked at admission, on calls, at result release, and while the pipe is idle.
MCP cancellation applies only to its matching in-flight request. Each pipe
allows one in-flight call; a client must wait or cancel before another call.

This command is a trusted connector primitive, not the Paperclip/Hermes host
connector itself. The existing operator `chaosbox mcp` is still an operator
surface and must not be installed as the worker's managed reader. Before worker
integration, the controller must resolve actual grants, enforce direct-route
isolation, and provide the conversation/recovery lifecycle described above.

## Qualification status — 2026-09-30

The isolated reader worktree passed:

- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace --exclude chaosbox-typedb`: 251 passed,
  three existing opt-in tests ignored (live compiler readback, live Jev pilot,
  and the separate plugin pilot).
- `cargo test --locked -p chaosbox-typedb --lib`: four passed.
- Changed-file `treefmt --ci` and `git diff --check`.

The new reader suite has 19 passing tests; the binary also tests private lease
file handling, and the legacy graph reader has a foreign/missing-build diff
regression. The shared backend conformance contract now includes published-build
ownership and bounded evidence.

Live TypeDB conformance was not executed. This is a local source qualification,
not a package, VM or deployed-worker receipt. Independent review attempts returned
errors instead of a verdict: the configured analysis/frontier/max models were
unavailable and the direct reviewer route rejected tool selection. Review and
live backend qualification remain open gates before the next worker-integration
slice. Paperclip grant resolution, authenticated host admission, conversation
and checkpoint recovery, and direct-route isolation still need that slice.
