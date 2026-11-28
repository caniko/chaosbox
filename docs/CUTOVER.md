# Cutover and rollback (PostgreSQL)

Chaosbox 0.2.0 replaces TypeDB with PostgreSQL 18 + pgvector (`chaosbox-pg`).
The earlier cutovers (Gel → TypeDB, 2026-09-20/23) no longer apply: their backends
and gates were deleted.

## Data status: fresh start

No TypeDB data migrates. Every published graph can be rebuilt from source:

- graph builds re-derive on the next snapshot run;
- private knowledge republishes from each `memory.sqlite` outbox (a one-time
   journal migration forgets the TypeDB-era predecessor so its pointer cannot
   block the first PostgreSQL publication);
- peer replicas re-sync from their owners.

Paid Jev decisions stored only in TypeDB are not migrated to PostgreSQL.
Affected candidates are re-asked under normal budget and cache rules.

The `postgres-integration` VM test and hermetic `live_pg_*` tests verify the
empty-install path:

- `chaosbox db migrate` applies `crates/chaosbox-pg/schema.sql` idempotently,
   under an advisory lock that serializes migration. The superuser creates
   pgvector first (`postgresql-setup` in the NixOS module).
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
4. Stop TypeDB only after validation, and keep its data directory
   frozen until the operator explicitly approves deletion.

## Rollback boundaries

- Before PostgreSQL-only writes matter: roll back by redeploying the previous
  Chaosbox package against the frozen TypeDB data; nothing new is lost.
- After PostgreSQL-only writes exist: TypeDB has none. Freeze PostgreSQL, run
  the deterministic export for affected builds, and re-derive; this rollback is
  exercised, never improvised.
- There are never dual writes to two authoritative databases.

## Security boundary and residual limitations

- Local socket peer authentication has no password. Enforce read-only access
  with a separate role granted only `SELECT` (for example a hosted agent's
  reader); the MCP and read paths also open read-only sessions.
- Readers never create databases or apply schemas; a missing database reads
  as pending, a missing table as "run db migrate".
- Connections are unencrypted, so configuration accepts only unix sockets
  and loopback addresses.
- The embedding endpoint receives private statements and source excerpts, so
  only loopback HTTP(S) endpoints are accepted.

## Production cutover

The gated sequence is: tested restore point → quiesce writers, record source
state → migrate → validate → switch endpoint → read/persistence checks →
resume writers → retain the frozen source for rollback. Do not advance to the
next step until the operator signs off.
