# S05a — versioned multi-project qualification corpus

S05a's source/oracle preparation gate is complete in signed local commit
`73b20702cf62f0dab3ecc4a748dd162cb01f4b37`. The immutable synthetic corpus is
[`fixtures/workspace-corpus/v1/`](../fixtures/workspace-corpus/v1/README.md), with
separate source/task inputs and independently specified expected results.
See the [receipt](s05a-workspace-corpus.json).

## Corpus contract

Four exact member versions represent three logical projects: `fixture-api`
1.0.0 and 2.0.0, `fixture-app` 1.0.0, and restricted `fixture-private-api` 1.0.0.
Their eight Rust/manifest files total 1,050 bytes. The caller's missing 3.0.0
dependency is intentional; neither available API version may replace it.

| Task | Independently expected result |
| --- | --- |
| `impact-v1` | Only the caller of API 1.0.0 is impacted |
| `resolve-v2` | The 2.0.0 call resolves to its exact provider |
| `ambiguous-context` | Both public versions are candidates; no binding is selected |
| `unavailable-v3` | The absent 3.0.0 provider remains unresolved |
| `wrong-provider-version` | Offering 2.0.0 to a 1.0.0 call produces a version mismatch |
| `scope-blocked` | Reader scope omits the restricted definition and source canary |
| `maintainer-private` | Maintainer scope resolves the same restricted call |

`corpus.json` binds each member's exact version, visibility, raw file hashes and
member content-manifest hash. `expected.json` contains eleven exact source quotes
and half-open byte spans with line/column coordinates. Resolved bindings cite the
consumer, exact dependency declaration and provider definition. Negative cases
record forbidden substitutions, consumers or private evidence where applicable.

The oracle was specified from source and declarations before a resolver exists;
it is not an exported graph, reviewed bridge list or model answer. Future S07/S08
qualification must supply its own discovered results and compare them with this
oracle. Input hashes use standard SHA-256 over raw bytes, separately from existing
Chaosbox identities and their trailing-separator hashing contract.

## Verification

Three focused checks validate the frozen input/oracle hashes, all source hashes
and evidence spans, exact dependency/version bindings and expected evidence
visibility, and native syntax ingestion. A retained copy of the test executable
also passes all three checks against the retained corpus copy. Eight existing
workspace-pilot regression tests pass separately; those are compatibility checks,
not the automatic-resolution qualification for this corpus.

A copied provider source was changed from `records:v1` to `records:x1` without
changing its byte length. The frozen-source check rejected its SHA-256 mismatch
with exit 101. The canonical corpus remained unchanged. Warnings-denied Clippy,
treefmt CI and whitespace checks pass; an initial Clippy allocation finding was
fixed using the standard `Write` trait before final qualification.

Current native `Snapshot::capture` selects each member's Rust source, yielding
four parsed files, seven definitions and distinct v1/v2 snapshot and definition
IDs. Package manifests are independently hash-bound corpus inputs; the current
syntax snapshot does not ingest their package/dependency semantics. These checks
therefore establish corpus and ingestion integrity, not implemented automatic
bridge resolution, persisted workspace views, or production scope enforcement.
S05–S08 retain those separate acceptance gates.

Reproduce in the approved project environment with the bounded worker settings
recorded in the receipt:

```sh
cargo test --locked --offline -p chaosbox --test workspace_corpus --test workspace --jobs 2 -- --nocapture
```

## Provenance and handoff

The source archive includes the preserved dirty checkout plus the corpus and
tests, excluding `docs/` and `tasks/`. `source-corpus.tar` SHA-256 is
`b548b217d5afe427e146781e2f06703973401e6bdc0ffe95dce157209ab46b3e`.
The corpus-only archive is independently retained as `corpus-v1.tar`; the
source, corpus and executable digests are in the receipt. Evidence lives at
`/data/scratch/tmp/opencode/chaosbox-s01b1-s05a-20261019/` with its command record,
qualification manifest and finalization. Tested source bytes remain unchanged
after the commit, the staged/committed patches match, and foreign tracked edits
are preserved. This is local scoped qualification, not a clean release.

Keep v1 fixed and add a new corpus version for source or oracle changes. The
next prerequisite is S01b's authorized allocation/physical-storage evidence,
now that [S01b.1](s01b1-retained-build-inventory.md) provides the logical history
inventory. Only after the parent gate closes may S05.1/S05.2 proceed, with S05.3
separately qualifying the multi-snapshot migration. No numeric production RAM
policy was adopted by these fixture/worker checks.
