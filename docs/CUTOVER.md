# Cutover and rollback (TypeDB migration)

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
3. Remove the Gel runtime path: `nix/chaosbox.nix` Gel remnants (none left),
   `dbschema/` + `chaosbox-gel` EdgeQL (keep the crate: `MemoryStore` and
   the conformance reference stay), Gel CI jobs, Gel docs.
4. Move `harbor-db` pin to trunk after harbor-db#7 merges; drop the
   `nixpkgs-typedb` input after NixOS/nixpkgs#565068 merges.

## Rollback boundaries

- Before the first TypeDB-only write: rollback is the endpoint switch
  (`CHAOSBOX_DB_BACKEND=gel`); nothing is lost.
- After TypeDB-only writes exist: flipping the endpoint back abandons those
  writes (there is no Gel replica). Rollback then means freeze the TypeDB
  database, export the affected builds/decisions/evidence through the
  deterministic export, and reconcile against the frozen Gel state; the
  procedure is exercised, never improvised, before any production cutover.
- Dual writes (two authoritative databases) are not maintained at any point.

## Production cutover: BLOCKED

No deployment target, credentials, or restore point have been provided, so
no production cutover is performed or claimed. When a target exists, the
gated sequence is: tested restore point → quiesce writers, record source
state → migrate → validate → switch endpoint → read/persistence checks →
resume writers → retain frozen source for rollback. Each step needs an
operator sign-off; this task does not authorize an unconfigured cutover.
