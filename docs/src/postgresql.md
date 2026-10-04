# PostgreSQL catalog evidence

Chaosbox captures one explicitly selected database/schema through packaged SQL
in a repeatable-read, read-only transaction. It collects relations, columns,
constraints, indexes and routines. Definitions come from PostgreSQL's native
deparsers. Foreign keys use `pg_catalog.pg_constraint`, which preserves FK
visibility for a SELECT-only role; `information_schema.referential_constraints`
can omit those keys for that role.

```sh
PGHOST=/run/postgresql PGUSER=chaosbox-postgresql \
  chaosbox postgres capture --database canix --schema public --output catalog.json
chaosbox postgres publish --artifact catalog.json --database canix \
  --schema public --repo atlas-postgresql
```

`postgres collect` combines capture, create-only receipt persistence and TypeDB
publication. It requires the same explicit database, schema and repository,
plus `--output-directory`. Libpq peer/service/passfile configuration owns
authentication; no password-bearing DSN is accepted as a database name.

Each private receipt binds the packaged query hash, native database identity,
actual collector role, observation time and canonical ordered records. A
published graph links exact JSONL catalog quotations to their content hashes
and source spans. Publication uses the active-build predecessor/generation
guard and rejects a changed database/schema or an older capture time.
No model calls are involved.

Coverage reports native object counts, collection time, out-of-scope FK targets
and explicit omissions. Rows, triggers, sequences, types and policies are not
collected. Cross-schema FK targets remain named in the constraint evidence and
counted as unresolved; no endpoint is fabricated outside the selected schema.
Overloaded routines retain distinct native identities. Catalog OIDs are scoped
to a capture, not durable identity across source changes.

Canix owns the dedicated PostgreSQL role, scheduling, runtime TypeDB credential
and Hermes stdio MCP registration. `atlas-postgresql` is an explicit repository
argument for queries. A build proves a historical catalog observation, not
continuous database freshness or successful deployment.
