# Optional compiler evidence

`compiler capture` runs the indexer you select and saves genuine SCIP
protobuf plus a content-bound receipt. `run --compiler` verifies the receipt
and publishes compiler facts alongside syntax facts, including with
`--no-decisions --max-candidates 0`.

```bash
chaosbox compiler capture /absolute/project --repo example \
  --context 'toolchain=rust-analyzer 1.98.1 (48a229c 2026-09-01)' \
  --context configuration=Cargo.toml \
  --context target=x86_64-unknown-linux-gnu --context features=default \
  --output /absolute/artifacts/example-index \
  -- rust-analyzer scip . --output '{index}' --exclude-vendored-libraries

chaosbox compiler inspect /absolute/artifacts/example-index --path /absolute/project
chaosbox run /absolute/project --repo example \
  --compiler /absolute/artifacts/example-index --no-decisions --max-candidates 0
```

Use a new output directory outside the project and the same `--source-paths`
arguments for capture and publication. Indexer stdout goes to stderr; stdout
stays reserved for the Chaosbox JSON response. Capture has a configurable
deadline and rejects nonzero exits or changed inputs. Queries never launch tools.

For scip-typescript 0.4.0, use `--unspecified-encoding utf16` and record the
TypeScript, Node and indexer versions, effective tsconfig, target and features.
The override applies only to encoding zero; unknown enum values fail. Other
versions need independently verified encoding semantics.

## Identity and evidence

- Decoder: exact-pinned official `scip` 0.10.0, protobuf 3.7.2. Both typed and
  legacy ranges work. Conflicting ranges, duplicate documents, escaping paths,
  code-point splits, and out-of-bounds positions reject the entire import.
- The receipt pins scoped source inventory, in-tree TOML/JSON/JSONC/YAML/Nix
  configuration, common lockfiles and explicit `--input` files. Additions and
  deletions count as drift. Symlinks and nested repositories are not traversed.
- The operator must declare `toolchain`, `configuration`, `target` and `features`
  with `--context` and can record additional external/environment inputs there.
  This does not attest to a hermetic build: capture does not discover undeclared
  external dependencies, environment or generated inputs. Before/after checks
  miss change-and-revert races during execution. Use an immutable project copy
  for stronger isolation.
- Raw index hashes use ordinary SHA-256; source hashes use the existing
  `sha256_hex` convention, including its trailing separator.
- Occurrence IDs bind snapshot, context, symbol, exact range and role. Syntax
  occurrence identities remain unchanged. Additive global anchors include repo
  and the entire package-qualified SCIP symbol. Local anchors also include
  document and analysis context. An anchor is not a rename/continuity claim.
- Definitions publish as file → compiler occurrence. References publish only
  with exactly one in-scope definition. The importer counts external, missing
  and ambiguous targets; it does not invent them from symbol-information records.
- `implements` publishes only explicit producer relationships with unique
  in-scope endpoints. Alias-definition/type-definition relationships are not
  followed. Calls remain unsupported: a function value and a call can have the
  same SCIP roles. Configuration membership and typecheck status remain unknown.
- Compiler occurrences do not create lexical inference candidates. Compiler
  evidence carries its full producer/context fingerprint and an exact source
  citation. Currently, each build supports one compiler analysis.

## Persistence and coverage

TypeDB schema **v4** adds optional compiler identity JSON to entity occurrences.
Run `chaosbox db migrate` for existing databases. Historical rows lack compiler
identity; migration does not guess at anchors or coverage.

Build coverage stores input hashes and declarations, indexed/omitted/outside-
scope documents, definitions, references, unresolved/ambiguous references,
implementations, unsupported relationships, and supplied diagnostic counts.
`structural_relations` continues to count syntax facts; compiler counts are in
`coverage.compiler`. Status/export show details for at most 100 compiler files;
storage keeps full compiler provenance and file records. Neither class depends
on decision budgets.

## Verification — 2026-09-30

The `.scip` files in `fixtures/indexer-smoke/` came from the 2026-09-29 probe
in [INDEXER_FEASIBILITY.md](INDEXER_FEASIBILITY.md). Tests use committed source
fixtures and retain the original project URLs. These indexes are normalization
fixtures, not fresh receipts.

Fresh `compiler capture` runs also succeeded:

| Producer | Definitions | Resolved references | No in-scope definition | Implementations |
| --- | ---: | ---: | ---: | ---: |
| rust-analyzer 1.98.1, default features | 18 | 22 | 13 | 0 |
| scip-typescript 0.4.0 / TypeScript 5.6.3 | 18 | 18 | 1 | 2 |

Artifacts: `/data/scratch/tmp/opencode/chaosbox-{rust,ts}-receipt-20260930`.
Workspace tests pass. The disposable TypeDB 3.13.0 gate passed all 13 live
backend tests plus the new compiler publication/fresh-reader test and the actual
CLI zero-model refresh/coverage-protection checks. Its log is
`/data/scratch/tmp/opencode/chaosbox-compiler-typedb-fixed.log`.

These results establish ingestion/publication behavior, not comparative agent
performance, complete compiler coverage, or successful memory calibration.
