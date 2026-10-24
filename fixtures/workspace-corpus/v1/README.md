# Workspace corpus v1

S05a's immutable source corpus has three logical projects, four exact member
versions, eight source/manifest files, and seven tasks. `corpus.json` owns the
source inventory and task inputs. `expected.json` is an independently specified
oracle with eleven exact source quotations and byte/line/column spans.

The members are two versions of `fixture-api`, a caller of both versions, and a
scope-restricted provider. The caller's absent `missing-api` dependency is
intentional: version `3.0.0` must remain unresolved even while versions `1.0.0`
and `2.0.0` are available. Do not fetch dependencies or generate a replacement
source tree to make that case compile.

The tasks cover exact v1 impact, exact v2 resolution, ambiguous unversioned lookup,
an unavailable version, an explicitly wrong provider, denied private-source
traversal, and an authorized private-scope positive control. Call relationships
point from consumer to provider; impact reverses that direction. Metadata and
evidence outside the requesting fixture scope must not appear in its answer.
`PRIVATE_SOURCE_CANARY` is synthetic test data for that boundary.

Run the corpus integrity and source-ingestion checks from the repository root:

```sh
cargo test --locked --offline -p chaosbox --test workspace_corpus --jobs 2
```

Hashes use standard SHA-256 over raw bytes. A member's content-manifest digest is
the SHA-256 of `sha256sum Cargo.toml src/lib.rs` output run from that member's root.
These are corpus input identities, not Git revisions, graph build IDs, admission
proofs, or replacements for Chaosbox's legacy trailing-separator digests.

No graph, bridge list, workspace-impact-pilot artifact, inferred relationship, or
model answer is supplied as an input. A future resolver must discover bindings
from the manifests and Rust source, then compare its results to the separate
oracle. The current checks establish source and oracle integrity plus local
syntax ingestion; they do not establish that S05–S08's future automatic resolver
or persisted workspace view is implemented. Record such qualification separately.

Keep v1's sources, inputs, and oracle fixed. Add a new corpus version for changed
source bytes or expected results; retain both versions in exact-source receipts.
