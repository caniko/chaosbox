# Cutover and rollback (TypeDB migration)

Historical scope: this records the 2026-09-20 backend rehearsal, not a current
fleet inventory or the status of external session archives. The proposed
session migration has its own preservation and rollback requirements.

## Data status

No real data exists on either backend: the Gel history held fixtures and
disposable test instances only (proven by inventory: `fixtures/`,
`scripts/test-gel.sh`, `nix/gel-vm-test.nix`; no persisted databases,
backups, or credentials anywhere). There is nothing to migrate.

What is proven instead (empty-install path):

- `chaosbox db migrate` applies the packaged TypeQL schema idempotently
  (re-define commits cleanly; verified live).
- `chaosbox run` publishes a real graph (demo 22n/26e, synth 160n/200e).
- `chaosbox db check` reports pending (exit 2) before migration and before
  the first build, ready (exit 0) after.
- Restart persistence holds (server reboot, same answers).
- Concurrent publishers from one generation: exactly one wins; the loser
  reports a conflict and the last good build stays active.

## Switching the default backend

1. Set `CHAOSBOX_DB_BACKEND=typedb` in the deployment environment (or flip
   the code default in `crates/chaosbox/src/main.rs::backend` once the
   validation below is green).
2. Re-run the full matrix: migrate → run → check ready → every consumer
   query → wrong-credential negative → reboot persistence
   (`scripts/test-typedb.sh` automates the disposable version).
3. Remove the Gel runtime path: DONE (2026-09-23) — `dbschema/` and the
   `chaosbox-gel` crate removed outright (shared `MemoryStore`/conformance
   relocated to `chaosbox-store`), Gel CI jobs and Gel docs deleted.
4. Move `harbor-db` pin to trunk after harbor-db#7 merges; drop the
   `nixpkgs-typedb` input after NixOS/nixpkgs#565068 merges.

## Rollback boundaries

- Before the first TypeDB-only write: rollback was the endpoint switch
  (`CHAOSBOX_DB_BACKEND=gel`); that switch was removed with the backend on
  2026-09-23 and nothing is lost.
- After TypeDB-only writes exist: flipping the endpoint back abandons those
  writes (there is no Gel replica). Rollback then means freeze the TypeDB
  database, export the affected builds/decisions/evidence through the
  deterministic export, and reconcile against the frozen Gel state; the
  procedure is exercised, never improvised, before any production cutover.
- Dual writes (two authoritative databases) are not maintained at any point.

## Security boundary and residual limitations

- TypeDB Community Edition has no read-only role: any credential holder can
  write. The MCP surface stays closed (read-only tools, no endpoint or model
  access), reads use read transactions (mutation rejected server-side), and
  raw credentials never reach MCP callers — but compromise of a
  credential-bearing service exceeds the read-only API by design. This is
  disclosed, not fixed, by this migration (no upstream RBAC implementation
  is added).
- Loopback binding by default; firewall exposure opt-in; vendor telemetry
  reporting off by default in the service module.
- Bootstrap: the default admin credential is test-only. Single-host pilots
  rotate it with the `typedb-bootstrap` package (console-only, idempotent,
  safe on every boot): run as root after `typedb.service` starts, then
  point the deployment at the generated application credential, e.g.
  `services.chaosbox.passwordFile = "/var/lib/typedb-auth/app-password"`.
  Ordering is typedb.service -> typedb-bootstrap -> db migrate ->
  application. No secret ever appears in argv: console authentication
  reads the password from stdin under a pty, secret-bearing commands run
  from a root-only script file, and failures report only the operation,
  never the transcript. Both rotated credentials are verified by
  re-authenticating, so a silent non-application fails loudly instead of
  reporting success. With no working credential left (stored password lost
  after rotation) bootstrap fails closed: recover the admin password via
  console and re-run.
- TLS is disabled on loopback (same trust boundary as before); enable it
  wherever connections cross a host boundary and verify it there.

## Production cutover: BLOCKED

No deployment target, credentials, or restore point have been provided, so
no production cutover is performed or claimed. When a target exists, the
gated sequence is: tested restore point → quiesce writers, record source
state → migrate → validate → switch endpoint → read/persistence checks →
resume writers → retain frozen source for rollback. Each step needs an
operator sign-off; this task does not authorize an unconfigured cutover.
