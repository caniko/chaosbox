# Certified structural facts

## Delivered contract

The first evidence-foundation slice publishes source-level `defines` and
file/module `contains` relationships independently of Jev and candidate caps.
These facts cover declarations in source, including conditional ones. They do
not establish that a symbol exists in any particular compiled or evaluated
configuration.

| Source | Certified declarations | Producer |
| --- | --- | --- |
| Rust | Named functions/signatures, structs, enums, traits, type aliases, constants, statics, modules, unions | tree-sitter 0.25.10 + tree-sitter-rust 0.24.2 |
| TypeScript / TSX / MTS / CTS | Named functions/signatures, classes, interfaces, aliases, enums, methods, identifier variable declarations (including arrow-function bindings) | tree-sitter 0.25.10 + tree-sitter-typescript 0.23.2 |
| Nix | Static identifier attribute-path assignments in sets and `let` expressions | rnix 0.14.0 |

Each producer identity includes its interpretation contract:
`declarations-v2` for Rust/TS (v1 plus syntactic nesting and rationale
annotations, below) and `declarations-v1` for Nix. Grammar dependencies are
exact-pinned. Changing the certified subset requires a contract-version bump.

`declarations-v2` additionally certifies:

- **Syntactic nesting**: `contains` from a declaration to a declaration nested
  inside its syntax node (Rust `mod`/`trait`/`fn` bodies, TS class members,
  locals). Rust `impl` methods are not nested under their type: finding the
  type is name resolution.
- **Rationale annotations**: a comment line opening with `NOTE`, `WHY`,
  `TODO`, `FIXME`, `HACK`, `SAFETY`, `XXX` or `ponytail` (optionally
  `MARKER(scope):`) becomes a `rationale` entity spanning marker to end of
  line, with `contains` from the next declaration (skipping comments,
  attributes and decorators), else the enclosing declaration, else the file.
  Prose that merely mentions a marker mid-line is not an annotation.

Definitions point from the containing **file** to each declaration occurrence.
Repeated names retain separate source occurrences; a shared display name is not
lexical resolution or cross-revision identity. Macro expansion, Rust feature
selection, compiler symbols, TS destructuring, Nix dynamic/quoted attributes,
`inherit`, and module evaluation are outside this subset. Imports and lexical
mentions remain uncertified proposals; call and heritage sites resolve only
under the separate INFERRED contract below. Python, JavaScript, Markdown and
text retain their existing heuristic path.

On any syntax error, the producer withholds all certified facts for the file
and reports `parse_error`; the file remains visible. `parsed` means the
documented subset ran successfully, not that extraction was exhaustive.
Comments, literal text and unexpanded Rust macro input cannot become certified
declarations (marked comments become annotations, never declarations). Nix string interpolations and TS template interpolations can
contain real expressions; those expressions are traversed normally.

## Resolved relations (INFERRED, no decisions)

Graphify labels name-matched calls EXTRACTED. Chaosbox treats a unique lexical
match as evidence, not proof: it publishes the relationship with INFERRED
evidence, a producer and the exact use-site span, without model decisions.

| Producer | Relationship | Rule |
| --- | --- | --- |
| `unique-name-v1/<syntax producer>` | `calls` (enclosing declaration or file → definition) | Rust `f()`, `a::f()`, `f::<T>()`, `self.f()`; TS `f()`, `this.f()`, `new C()` |
| same | `inherits` / `implements` | TS `extends` / `implements` / interface `extends`; Rust `impl Trait for Type` (type resolved in the same file) |
| `citations-v1` | `references` (rationale or Markdown file → document) | `ADR-12`, `RFC 9110` → the unique captured `adr-0012*`, `rfc9110*`, or `0012-*` under `adr(s)/`, `decisions/`, `rfc(s)/` |

Resolution indexes non-compiler definitions per language. A target resolves
when it is unique in the use site's file, otherwise when unique in the whole
snapshot. `self.`/`this.`/`Self::`/`self::` sites resolve within their file
only. `Owner::name` requires `name` unique in the uniquely resolved owner's
file, or unique among module files named like the owner, so `String::new`
and `Vec::with_capacity` stay unresolved. Method calls on other receivers
(`x.len()`), calls inside macro input, recursion and ambiguous names are
never resolved; receiver typing is SCIP's job. External RFCs without a
captured document get no node (the text stays searchable on the rationale).
**A missing `calls` edge never means nothing calls a symbol.**

The resolver skips ambiguous sites instead of proposing them to Jev. When a
consenting repository needs adjudication, add a `call-site` candidate reason,
never `structural`, which materializes EXTRACTED.

## Evidence and publication

- Every direct edge has a claim and source evidence with a producer identity.
- Evidence text is the exact declaration-name range, or an empty file/module
  point. Ranges use zero-based UTF-8 byte offsets, exclusive ends, and one-based
  Unicode-scalar line/column positions.
- Publication checks endpoint repository/snapshot bindings, the supported edge
  and entity kinds, the range, and captured content identity before writing.
- No synthetic model decisions or reusable model inferences are created.
- Both stores retain evidence and claims. TypeDB persists evidence spans
  independently of entity spans, and publishes through the existing guarded
  active-pointer transaction.
- Lookup/search/export/explain return occurrence spans. Relationship evidence
  returns `producer` and `citation: {snapshot, file, sha256, span}`.
  `sha256` retains the existing snapshot hash convention (`sha256_hex`, including
  its trailing separator), rather than silently changing historical identities.

Schema v3 adds optional `producer` and `coverage-json` attributes. Run
`chaosbox db migrate` before using the new reader with an existing database.
Migration preserves old rows; missing provenance/coverage is reported as unknown.
No entity-ID migration is part of this slice.

Coverage `resolved_relations` counts the INFERRED tier separately. Publication
derives every bucket from retained evidence: `structural_relations` (+ compiler
+ catalog) must equal edges whose evidence is all producer-backed EXTRACTED,
`resolved_relations` those all producer-backed INFERRED, and
`decision_relations` the rest. Decision evidence never carries a producer, so
the no-decisions refresh gate, which reads only `decision_relations`, treats
resolved builds as structural and refreshes them freely.

Build coverage records file-level producer/outcome and counts of certified and
decision-backed relations. It covers only captured source, not ignored/unsupported
files or semantic completeness. Candidate selection/omission accounting stays in the
catalog/run report. CLI/MCP status and exports include at most 100 file detail
rows plus total/error/heuristic counts and `omitted_files`. The build retains
full file records.

`--no-decisions` refreshes a known structural-only build even when semantic
proposals are uncached. For builds containing decision-backed relationships,
or legacy relationships with unknown provenance, the conservative exit-4 rule
still protects the active view. A failed decision batch still blocks publication.

## Regression baseline

The previous cold `--no-decisions` pipeline produced zero relationships. The
exact gold suite now requires source-backed declaration edges with a zero
candidate budget, including exported TS functions and duplicate names.

- `crates/chaosbox-extract/tests/syntax.rs`: exact declaration lists; forbidden
  comment/string/macro matches; shadowed occurrences; invalid syntax;
  `.mts`/`.cts`/TSX; Unicode byte/column ranges.
- `crates/chaosbox/tests/structural.rs`: capture → zero-model publication →
  lookup/evidence; content hashes; source edits; immutable historical queries;
  rejection of extraction from another snapshot.
- `crates/chaosbox-typedb/tests/live.rs`: direct evidence/provenance/coverage
  round-trip through a fresh reader, without any model decision.
- `scripts/test-typedb.sh`: isolated TypeDB server, schema migration, full live
  suite, actual CLI cold publication and fresh-process refresh, and protection
  of a decision-bearing active build.

This is a deterministic correctness baseline. Comparative Graphify quality,
compiler/SCIP ingestion, stable logical-symbol identity, cross-repository impact
and calibrated memory utility still need their planned proving cases. The
[real indexer feasibility probe](INDEXER_FEASIBILITY.md) records the initial
Rust/TypeScript/rnix results and the constraints they establish for that work.

## Verification — 2026-09-29

- `cargo test --workspace`: passed; the existing opt-in live memory-quality
  pilot remained ignored.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `scripts/test-typedb.sh`: passed on an isolated TypeDB 3.13.0 instance at
  `127.0.0.1:18729`, including all 13 live tests with skips prohibited and the
  cold-publish/fresh-process-refresh/exit-4 CLI cases.
- `treefmt --ci` over the changed Rust files and `git diff --check`: passed.
- `cargo audit`: no vulnerability findings; one existing unmaintained-package
  warning for `rustls-pemfile` 2.2.0 (`RUSTSEC-2025-0134`).

Cargo checks used `env -u CARGO_HOME direnv exec . ...` so the project's
generated Cargo configuration was selected. The inherited host configuration
contained nightly-only flags incompatible with the selected stable compiler.
The TypeDB gate used `target/debug/chaosbox` and scratch under
`/data/scratch/tmp/opencode`; no production database was migrated.
