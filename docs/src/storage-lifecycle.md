# Storage lifecycle integration

Chaosbox pins Harbor DB's qualified storage release
`99aca6956890ca347a06fe73ca93bbe22fa72368`. Its TypeDB migration project uses
the `typedb` backend, credential-file delivery, read-only readiness checks and
ordering after `typedb.service`. The default runner is portable and uncached.

The Harbor DB default module also exposes the PostgreSQL lifecycle adapter.
PostgreSQL catalog consumers on Atlas share the host's guarded PG18 primary;
Chaosbox does not claim a second authority for that physical cluster.

TypeDB provisioning and application schema migration retain their existing
engines. PostgreSQL's physical identity/upgrade adapter is not a TypeDB upgrade
engine. TypeDB storage cutovers still require verified application records,
backups and explicit backend/schema acceptance. A generic storage-authority
integration must supply an independently verified binding and a read-only
consumer validator before it can guard TypeDB writers.

The upstream release adds durable backup publication and rejects replacement of
missing adopted lock anchors. Deployments using those helpers must preserve the
authority directory with the data and keep runtime writers on inherited leases.
