# Storage lifecycle integration

Chaosbox consumes Harbor's database module (`harbor.nixosModules.database-default`).
Its migration project uses the `postgres` backend: `chaosbox db migrate` and the
read-only `db check` readiness probe run as the peer-authenticated database
owner, ordered after `postgresql-setup.service`, with no credential files. The
default runner is portable and uncached.

`nixosModules.chaosbox` ensures the database and its owner role, and creates the
pgvector extension as the superuser in `postgresql-setup` (pgvector is not a
trusted extension, so the unprivileged owner's migration only verifies it). The
schema itself is `crates/chaosbox-pg/schema.sql`, applied idempotently in one
transaction under an advisory lock.

The Harbor DB default module also exposes the PostgreSQL lifecycle adapter.
Consumers on Atlas share the host's guarded PG18 primary; Chaosbox owns only its
own database inside that cluster, not the cluster's physical identity, upgrades
or backups. Storage cutovers still require verified application records,
backups and explicit backend/schema acceptance (see `CUTOVER.md`).

The upstream release adds durable backup publication and rejects replacement of
missing adopted lock anchors. Deployments using those helpers must preserve the
authority directory with the data and keep runtime writers on inherited leases.

TypeDB was the storage backend until 0.2.0; its project, credential delivery and
service ordering were removed with it.
