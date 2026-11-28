# Storage lifecycle integration

Chaosbox consumes Harbor's database module (`harbor.nixosModules.database-default`).
The migration project uses the `postgres` backend. After
`postgresql-setup.service`, `chaosbox db migrate` and the read-only `db check`
readiness probe run as the peer-authenticated database owner without credential
files; application readiness remains Chaosbox's responsibility. The default
runner is portable and uncached.

`nixosModules.chaosbox` ensures the database and owner role exist. It creates
pgvector as the superuser in `postgresql-setup`. Because PostgreSQL does not
classify pgvector as a trusted extension,
the unprivileged owner's migration only verifies it. The migration applies
`crates/chaosbox-pg/schema.sql` idempotently in one transaction under an advisory lock.

The Harbor DB default module also exposes the PostgreSQL lifecycle adapter.
Atlas consumers share the host's guarded PG18 primary. Chaosbox owns its database,
not the cluster's physical identity, upgrades or backups. Storage cutovers still
require verified application records, backups and explicit backend/schema
acceptance (see [`CUTOVER.md`](cutover.md)).

The upstream release adds durable backup publication and rejects replacement of
missing adopted lock anchors. Deployments using those helpers must preserve the
authority directory with the data and keep runtime writers on inherited leases.

In 0.2.0, Chaosbox removed the TypeDB backend along with its project, credential
delivery and service ordering.
