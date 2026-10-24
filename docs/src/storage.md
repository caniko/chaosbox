# Storage and semantic retrieval

Chaosbox 0.2 stores graph builds, decisions, private knowledge and peer
replicas in PostgreSQL (tested against 18) with the pgvector extension.
`crates/chaosbox-pg` implements the shared `Store`, `GraphQueries` and
`ReplicaStore` contracts and passes the same conformance suites as the
in-memory backend.

## Why TypeDB was replaced

The database only ever performed keyed upserts, build-membership joins, a
compare-and-swap active pointer, one-hop edge lookups, substring matching,
keyset paging and one count. Traversal, ranking and analytics already ran in
Rust. For that, TypeDB cost 13–14 GiB RSS plus 23–25 GiB swap on Atlas, about
1 TB written in 43 hours (an unpruned WAL), `[REX1]` read failures during
publication, and a forked binary with a temporary nixpkgs pin. The 0.1
measurements remain in the dated `s0*` reports.

## Connection

| Variable | Meaning |
|---|---|
| `CHAOSBOX_DATABASE_URL` | libpq key=value or URL. Every host must be a unix socket directory or loopback (sessions use no TLS). |
| `CHAOSBOX_DB_BACKEND=postgres` | `run` publishes to the database; otherwise it uses the disposable in-memory store. |

On Atlas the connection is `host=/run/postgresql dbname=chaosbox` with peer
authentication, so no password exists. Readers set
`default_transaction_read_only`; a real read-only boundary is a role granted
only `SELECT`. pgvector is not a trusted extension: an operator creates it
once (`CREATE EXTENSION vector`, done by the NixOS module in
`postgresql-setup`), and `db migrate` only verifies it.

`chaosbox db migrate` applies `crates/chaosbox-pg/schema.sql` in one
transaction under an advisory lock, so overlapping runs are safe. `db check`
keeps contract v2: exit 0 ready, 2 pending (no database, no schema or no
active build), 1 error.

## Publication

Writes stage in memory with the reference semantics, then one transaction:

1. Take the repository's advisory lock; a held lock is a lost race.
2. Check the live pointer: an exact retry must match the stored digest; a
   new build needs the expected predecessor and a higher generation.
3. Insert the build header (digest, node and edge counts), entities, node
   memberships, relationships with their sealed evidence, audit rows and
   claims, all `ON CONFLICT DO NOTHING` in key order.
4. Re-check every staged claim body against the durable winner.
5. Move the pointer and commit. An uncertain commit is reconciled by
   re-reading the pointer on a fresh session.

Published rows are never updated, so readers use single statements without
holding a transaction, and reads during publication cannot fail on it.
Payloads are `text`, not `jsonb`, because digests and claim, replica and
evidence bodies are compared byte for byte; key and sort columns use the
`"C"` collation so SQL order equals Rust order and keyset cursors.

Decisions and inferences are durable-first single statements: a decision
replaces its row only when the cache key changed or the old outcome failed;
an inference keeps the first write. Knowledge generations and replica views
advance their pointers with the same advisory-lock compare-and-swap.

## Embeddings

Embeddings are derived data. They never enter a publication, its digest or
its sealed evidence, and every failure degrades retrieval to lexical.

- Extraction gives definitions and markdown headings bounded text: leading
  doc comments plus the item (or the heading and its first paragraph),
  capped at 1 KB so one input fits the embedding server's 512-token batch.
- Publication stores `<qualified name>\n<text>` for definition, symbol and
  heading entities in the content-addressed `texts` table. Knowledge and
  replica publication add their statements.
- `chaosbox run` embeds texts that lack a vector for the configured model,
  32 per request, for at most 120 seconds; the next run continues. Vectors
  are truncated to 1024 dimensions, renormalized and stored as `halfvec` under
  the key `<model>@1024`.
- `search` and `context` fuse the lexical ranking (exact name, prefix,
  substring) with the build's nearest embedded entities using reciprocal
  rank fusion (k = 60). `context` reports `"semantic": true` and algorithm
  `bounded-hybrid-bfs-v1` when vectors contributed.
- MCP `intelligence_context` over the current replica snapshot and
  `compaction context` fuse word overlap with statement similarity above a
  0.3 cosine floor. Bundle files, federation and assessment stay lexical.

| Variable | Meaning |
|---|---|
| `CHAOSBOX_EMBED_URL` | OpenAI-compatible `/v1/embeddings` URL; loopback only, because private session text is sent to it. Unset disables embeddings. |
| `CHAOSBOX_EMBED_MODEL` | Model name sent to the endpoint (Atlas: `qwen3-embedding-8b`). |
| `CHAOSBOX_EMBED_TIMEOUT_MS` | Query embedding deadline, default 2000. A cold model load past it yields lexical results. |

The nearest-neighbour read is an exact scan of one build's embedded entities.
Add an HNSW index with `hnsw.iterative_scan` once latency is measured too
high; a filtered HNSW scan without iterative scan silently loses recall.

## Upgrading from 0.1

There is no TypeDB data migration. Graph builds are re-derived by the next
snapshot run, private knowledge republishes from each `memory.sqlite` outbox
(the journal forgets its TypeDB-era predecessor once), and replicas re-sync
from peers. Federation providers use `{"kind":"postgres"}` and read
`CHAOSBOX_DATABASE_URL`.

## Testing

`live_pg_*` tests start their own disposable cluster on a private socket
(`initdb`/`pg_ctl`/`psql` with pgvector on `PATH`, or `PG_BIN`):

```sh
cargo test --workspace -- --ignored live_pg_
```

The `postgres-live` flake check runs them in the build sandbox,
`postgres-integration` boots the NixOS module in a VM, and
`scripts/test-postgres.sh` (`.#test-postgres`) runs the CLI contract matrix
against a disposable cluster.
