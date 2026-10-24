# Cutover and rollback (PostgreSQL)

Chaosbox 0.2.0 replaces TypeDB with PostgreSQL 18 + pgvector (`chaosbox-pg`).
Earlier cutovers (Gel → TypeDB, 2026-09-20/23) are history: their backends and
gates were deleted, and this page records only what still applies.

## Data status: fresh start

No TypeDB data is migrated. Every published graph is re-derivable from source:

- graph builds re-derive on the next snapshot run;
- private knowledge republishes from each `memory.sqlite` outbox (a one-time
  journal migration forgets the TypeDB-era predecessor, so the first
  PostgreSQL publication is not blocked by a pointer the new store never had);
- peer replicas re-sync from their owners.

Paid Jev decisions held only in TypeDB are not carried over; affected
candidates are re-asked under the normal budget and cache rules.

What is proven (empty-install path, `postgres-integration` VM test and
hermetic `live_pg_*` tests):

- `chaosbox db migrate` applies `crates/chaosbox-pg/schema.sql` idempotently,
  serialized under an advisory lock; pgvector is created beforehand by the
  superuser (`postgresql-setup` in the NixOS module).
- `chaosbox run` publishes a real graph; a fresh process publishes generation 2.
- `chaosbox db check` reports pending (exit 2) before migration and before
  the first build, ready (exit 0) after; an unknown role is an error (exit 1).
- Restart persistence holds (guest reboot, same answers).
- Concurrent publishers from one generation: exactly one wins; the loser
  reports a lost race and the last good build stays active.

## Switching a deployment

1. Ensure the database, its owner role and `CREATE EXTENSION vector`
   (`nixosModules.chaosbox` does all three; on Atlas the canix host config
   owns the cluster).
2. Set `CHAOSBOX_DB_BACKEND=postgres` and `CHAOSBOX_DATABASE_URL` (unix socket
   or loopback only, e.g. `host=/run/postgresql dbname=chaosbox`); optionally
   `CHAOSBOX_EMBED_URL` / `CHAOSBOX_EMBED_MODEL` for semantic retrieval.
3. Run `db migrate`, then the matrix: run → check ready → every consumer
   query → foreign-role negative → restart persistence
   (`scripts/test-postgres.sh` automates the disposable version).
4. Stop the TypeDB service only after validation, and keep its data directory
   frozen until the operator explicitly approves deletion.

## Rollback boundaries

- Before PostgreSQL-only writes matter: roll back by redeploying the previous
  Chaosbox package against the frozen TypeDB data; nothing new is lost.
- After PostgreSQL-only writes exist: TypeDB holds none of them. Rollback then
  means freezing the PostgreSQL database, exporting affected builds through the
  deterministic export, and re-deriving; it is exercised, never improvised.
- Dual writes (two authoritative databases) are not maintained at any point.

## Security boundary and residual limitations

- Peer authentication over the local socket: no password exists. A real
  read-only boundary is a separate role granted `SELECT` only (for example a
  hosted agent's reader); the MCP and read paths also open read-only sessions.
- Readers never create databases or apply schemas; a missing database reads
  as pending, a missing table as "run db migrate".
- Connections are unencrypted, so configuration rejects every host that is
  not a unix socket or loopback address.
- The embedding endpoint receives private statements and source excerpts, so
  only loopback HTTP(S) endpoints are accepted.

## Production cutover

The gated sequence is: tested restore point → quiesce writers, record source
state → migrate → validate → switch endpoint → read/persistence checks →
resume writers → retain the frozen source for rollback. Each step needs an
operator sign-off.
