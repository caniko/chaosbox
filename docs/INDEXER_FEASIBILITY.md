# Compiler indexer feasibility — 2026-09-29

Real Rust and TypeScript compiler-indexer smoke runs succeeded and produced
useful resolved references, with caveats for the next adapter. See
`fixtures/indexer-smoke/` for fixtures and decoder assertions, and
[COMPILER_EVIDENCE.md](COMPILER_EVIDENCE.md) for the implemented optional
importer and capture contract.

## Pinned inputs

- rust-analyzer `1.98.1 (48a229c 2026-09-01)`, installed from
  `/nix/store/34qiaf1bwi38xg4h282wy6kj9k560lv6-rust-default-1.98.1`.
- `@sourcegraph/scip-typescript` 0.4.0, package source revision
  `1962a68386220dd669c3839b69d64fb5ce34f2a6`, with TypeScript 5.6.3.
- Node 24.19.0. The run worked on Node 24, but the indexer's README lists
  Node 18/20 as supported. This shows feasibility, not a supported
  runtime matrix.
- Official SCIP protobuf schema at
  `9c4a0a3a81504a82b00397f9ede47a4ff35321af`, decoded using protobufjs 8.8.0.
  The fixture's tooling lock pins its complete npm dependency tree.
- rnix 0.14.0 through Chaosbox's implemented syntax pass.

## Observed results

| Index | Documents | Occurrences | Symbol records | Definition occurrences | Symbol relationships | Diagnostics |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust, default features | 3 | 53 | 19 | 18 | 0 | 0 |
| Rust, `extra` feature | 3 | 54 | 20 | 19 | 0 | 0 |
| TypeScript, valid project | 4 | 37 | 18 | 18 | 2 | 0 |
| TypeScript, unresolved import | 4 | 34 | 18 | 18 | 2 | 0 |

The valid TypeScript project also passed `tsc --noEmit`. On the deliberately
broken project, that check failed with TS2307 (exit 2), but scip-typescript
returned exit 0 without diagnostic records. `inspect.cjs` passed all assertions
against the four binary SCIP files, including:

1. **Aliases and shadowing:** imported `renamed` references resolve to the
   provider's `answer`; the shadowed local function has a different,
   document-local symbol. Rust also resolves the local Cargo dependency to
   `rust-analyzer cargo helper 0.2.0 dependency().`.
2. **References are not calls:** `renamed()` and `const/let reference = renamed`
   have the same symbol roles (0) and syntax kind (0) in both indexers. Calls
   need syntax classification around the resolved reference.
3. **Coordinate conversion is mandatory:** Rust declares UTF-8 positions.
   TypeScript 0.4.0 leaves `position_encoding` unspecified (0), yet its token
   after `🦀` starts at UTF-16 column 36, UTF-8 column 38. Metadata's UTF-8 file
   encoding does not describe position encoding. Any compatibility override
   must be bound to this producer/version.
4. **Analysis configuration is missing from SCIP metadata:** enabling `extra`
   adds `configured` but leaves metadata identical, including an empty tool
   argument list. The default Rust index still includes a reference inside
   that inactive function. An adapter must distinguish source references from
   active-configuration membership.
5. **Language capabilities differ:** TypeScript emits implementation links for
   `Worker → Work` and `Worker.run → Work.run`. This Rust index emits none,
    despite indexing a trait implementation. A shared format does not prove parity.
6. **Macro evidence can be partial:** Rust resolves the generated function's
   use and emits its symbol record, but provides no definition occurrence for
    it. Show the missing source location; don't synthesize it.
7. **Unresolved imports remain unresolved:** the broken TS import produces
   local-symbol references without matching definition occurrences. Dropping
   those silently or treating a local ID as a resolved definition would hide
   the failure.

The Nix fixture produced **11 direct facts across two parsed files**, including
three separate `answer` assignment occurrences. It retained `inherit`, dynamic
attributes and import/name resolution outside the certified subset. No
evaluator-backed Nix result was established in this run.

## Implications for the next slice

- Bind an optional index to source hashes, producer/schema version, exact
  invocation, package manifests/locks, effective target/features and compiler
  configuration. Chaosbox's source-only snapshot tested here does not currently
  capture TOML/JSON configuration changes, so it cannot supply this fingerprint
  on its own.
- Keep immutable source occurrences separate from logical symbol anchors.
  Scope local symbols by document **and index context**. Preserve the full
  package-qualified external symbol and version-bound dependency evidence;
  matching package names/versions alone does not identify a repository build.
- Expose indexed/omitted files, diagnostics, unresolved occurrences and absent
   definition locations. Exit status and an empty diagnostics list don't
   establish coverage; keep compiler-check outcomes separately.
- Ingest real protobuf with explicit position conversion, current typed-range
  support and legacy range support. scip-typescript's bundled generated schema
  predates position encoding and would discard fields from current Rust output.
- Treat `references`, `calls`, implementation relationships and syntax-only
  declarations as different capabilities with independent evidence gates.
- The integrated TS-plugin → Rust-command → Nix-package → Canix-consumer pilot
  still needs explicit version-bound bridge evidence and one grounded, scoped
  decision. The language indexes alone do not prove these cross-language links.

## Reproduce the bounded probe

From this checkout, run in Bash with rust-analyzer, Cargo, Node and npm. Use the
versions above to compare results. Compiler tools and all generated files stay
in the scratch copy; normal Chaosbox operation needs neither.

```bash
WORK=$(mktemp -d /data/scratch/tmp/opencode/chaosbox-indexer-spike.XXXXXX)
cp -R fixtures/indexer-smoke/{rust,typescript,nix,tooling} "$WORK/"
npm ci --prefix "$WORK/tooling" --ignore-scripts --no-audit --no-fund
curl --fail --silent --show-error --location \
  https://raw.githubusercontent.com/scip-code/scip/9c4a0a3a81504a82b00397f9ede47a4ff35321af/scip.proto \
  --output "$WORK/scip.proto"
printf '%s\n' '{"cargo":{"features":["extra"]}}' > "$WORK/rust-extra.json"

env -u CARGO_HOME direnv exec . env CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR="$WORK/rust-target" rust-analyzer scip "$WORK/rust" \
  --output "$WORK/rust-default.scip" --num-threads 2 --exclude-vendored-libraries
env -u CARGO_HOME direnv exec . env CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR="$WORK/rust-target" rust-analyzer scip "$WORK/rust" \
  --output "$WORK/rust-extra.scip" --config-path "$WORK/rust-extra.json" \
  --num-threads 2 --exclude-vendored-libraries

TS="$WORK/tooling/node_modules/@sourcegraph/scip-typescript/dist/src/main.js"
node "$WORK/tooling/node_modules/typescript/bin/tsc" -p "$WORK/typescript" --noEmit
node "$TS" index --cwd "$WORK/typescript" --output "$WORK/typescript.scip" --no-progress-bar
cp -R "$WORK/typescript" "$WORK/typescript-unresolved"
python3 - "$WORK/typescript-unresolved/src/main.ts" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
p.write_text(p.read_text().replace('@local/provider.js', '@local/missing.js'))
PY
# Expected: TS2307, exit 2. The following index command still succeeds.
node "$WORK/tooling/node_modules/typescript/bin/tsc" -p "$WORK/typescript-unresolved" --noEmit
node "$TS" index --cwd "$WORK/typescript-unresolved" \
  --output "$WORK/typescript-unresolved.scip" --no-progress-bar
node fixtures/indexer-smoke/inspect.cjs "$WORK"

target/debug/chaosbox run fixtures/indexer-smoke/nix --repo nix-smoke \
  --no-decisions --max-candidates 0
```

The inspector writes binary hashes to `$WORK/summary.json`. They vary with
`project_root` and identify artifacts, not portable gold values. Recorded artifacts:
`/data/scratch/tmp/opencode/chaosbox-indexer-spike.BtA52W`.

## Sources

- [scip-typescript 0.4.0 README](https://github.com/sourcegraph/scip-typescript/blob/1962a68386220dd669c3839b69d64fb5ce34f2a6/README.md)
  and [position construction](https://github.com/sourcegraph/scip-typescript/blob/1962a68386220dd669c3839b69d64fb5ce34f2a6/src/Range.ts).
- [SCIP schema at the decoded revision](https://github.com/scip-code/scip/blob/9c4a0a3a81504a82b00397f9ede47a4ff35321af/scip.proto):
  document encoding, local symbol scope, roles and relationships.
- [rust-analyzer SCIP implementation](https://github.com/rust-lang/rust-analyzer/blob/03fcb77246f2568adb0e9b2fa60d19c6cc1686f4/crates/rust-analyzer/src/cli/scip.rs):
  current upstream reference, not the installed binary revision. The findings
  come from running the installed binary, not assumptions about this source.
