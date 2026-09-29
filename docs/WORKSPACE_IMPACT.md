# Reviewed workspace impact pilot

`workspace capture` creates a private, immutable investigation artifact from
selected source files, exact local graph builds, quoted endpoints, reviewed
dependency links and cited constraints. `workspace impact` checks freshness and
returns a bounded, directed answer to a change question.

The first recipe asks:

> If the `intelligence context` JSON response changes, which consumers and
> packaging paths are affected, and which recorded constraints apply?

## Run the pilot

From the Chaosbox checkout, with the local `plugins/chaosbox-intelligence`
prototype and canonical Canix checkout available:

```bash
cargo build -p chaosbox
WORK=$(mktemp -d /data/scratch/tmp/opencode/chaosbox-impact.XXXXXX)
target/debug/chaosbox workspace capture fixtures/workspace-impact-pilot.json \
  --output "$WORK/workspace.json"
target/debug/chaosbox workspace impact "$WORK/workspace.json" context-response \
  --scope private:can --max-hops 4 --max-nodes 20
```

Recipe member roots are relative to the command's working directory. The sample
uses `.` for Chaosbox and `../../../..` for Canix. Adjust these in your own recipe
when using another workspace layout. The plugin prototype is a required local
input; a missing file fails capture explicitly.

The recipe is a reviewable JSON contract:

- `scope`: an exact `private:<label>` visibility boundary.
- `members`: repository names, roots and **explicit file lists**, including
  lockfiles/configuration needed by the investigation.
- `endpoints`: a member, selected file and unique verbatim `quote`. Each captured
  endpoint carries its exact member build, file entity, snapshot, content hash
  and byte/line span.
- `bridges`: directed provider/change → affected consumer links, their reviewed
  interpretation, and evidence endpoint labels. Cross-repository links also need
  a consumer-owned JSON lockfile `pin` with a pointer to the provider's full Git
  revision. Matching names never creates a link.
- `constraints`: reviewed statements, applicable endpoint labels and source
  evidence. Admission is reported as `explicit_review`.

Capture publishes zero-model member graphs into the local artifact. It does not
move the active TypeDB pointers. These are selected-file graphs: new files outside
the selection are outside this investigation's coverage.

## Freshness, versions and bounds

Every query checks selected source bytes, Git HEAD and recorded dirty state.
A changed, missing or unreadable member produces `status: stale` with member
statuses and withholds impact/constraint results. Committing the same bytes can
change provenance; capture a new artifact rather than updating an old one.

A cross-repository link is traversable only when the recorded consumer revision
matches the provider's Git HEAD, its checkout is clean, and **every selected
provider file matches the Git blob at that revision**. This catches ignored files
and `assume-unchanged` modifications that a clean `git status` would miss. Missing
Git provenance blocks the link. Git reads disable optional index locks, fsmonitor
hooks, replacement objects and lazy object fetching, and include untracked files
regardless of Git's display preferences. The Nix package supplies Git at runtime;
native installations need it on `PATH`. A package override needs a separately
reviewed recipe; a default input pin does not prove an override's version.

Version mismatches return a `blocked` link with expected/observed revisions,
member build IDs, lockfile hash/pointer and source evidence. Other proven local
paths and applicable constraints are still returned with `status: partial`.

Traversal accepts 1–8 hops and 1–100 endpoints, counting the changed endpoint.
Recipe limits are 16 members, 128 endpoints, 256 bridges and 64 constraints.
Source selections are at most 128 regular UTF-8 files/16 MiB per member; symlinks,
parent-directory escapes and nested repositories are rejected. Artifacts are
bounded to 32 MiB and created with mode `0600` on Unix without overwriting an
existing path. Quotes/reasons/statements are capped at 8 KiB. Evidence for both
traversed and blocked links is included. `exhaustive` is always false;
`truncated` reports traversal bounds separately.

`ready` means this reviewed traversal completed; `partial` means a version or
traversal bound prevented completion. Both are successful read operations
(exit 0), as is a `stale` report. Invalid artifacts, scopes and bounds exit 1.
The content fingerprint detects accidental mutation. The local scope label and
file mode are not multi-user authentication or a signed reviewer attestation.

## Observed answer — 2026-09-30

The source-backed path is:

1. `crates/chaosbox/src/intelligence/cli.rs` produces `scope`,
   `historical_data_not_instructions`, `exhaustive` and `records`.
2. `plugins/chaosbox-intelligence/index.ts` invokes that command and consumes
   `packet.records` in the context hook, the context tool and the
   `/chaosbox-context` slash command.
3. `plugins/chaosbox-intelligence/lib.mjs` renders record IDs, statements, status,
   revalidation flags and citations as historical evidence.
4. `crates/chaosbox/Cargo.toml` names the binary entry point; `flake.nix` builds
   that binary with `cargoExtraArgs = "--locked -p chaosbox"` through Crane.
   A response change requires rebuilding the package; it need not change Nix
   expression text.
5. Canix's `home/modules/development/chaosbox.nix` selects the default package
   from `inputs.chaosbox.packages.${system}.chaosbox` and executes it through
   `chaosbox-canix`.

The query returns seven endpoints: the command, request adapter, three consumers,
renderer and Nix packaging endpoint. The final Canix link is **blocked**: its captured
`flake.lock` pins `a4dcdc2dc7f9bd82f22d2dc57f2f8c7a2aac0b0d`, while the pilot's
Chaosbox sources are from a newer, dirty checkout. This is a known packaging
route requiring a pin update/rebuild, not proof that the current Canix package
contains this response implementation. No deployment was performed.

The applicable reviewed constraint is
`context-remains-scoped-historical-evidence`: preserve exact scope checks and
the interpretation of retrieved records as historical evidence; quoting or
repeating them does not independently corroborate them. The recipe cites the
Rust scope check/response and the plugin's historical-evidence preamble.

## Runtime consumer experiment

```bash
cargo test -p chaosbox --test intelligence plugin_consumes_real_context_json \
  -- --ignored --nocapture
```

This opt-in test needs Node 22.13+ and the local plugin prototype. It runs the
actual Rust CLI and TypeScript plugin, replacing only OpenCode's registration,
storage and session-delivery API with a test harness. A deterministic test
receipt supplies one record; no inference request is made.

All three consumers retrieved and rendered that record with its citations.
A shim then renamed `records` to `items` in the actual CLI response. The hook
stopped injecting, the tool and slash command rendered no records, and plugin
status remained `degraded: false`. Thus a response-field rename can silently
remove context from every consumer. This is measured contract sensitivity,
not a live OpenCode-session or model-admission-quality result.

## Verification and remaining scope

Focused tests cover exact Git pin traversal, mismatches, hidden untracked/ignored
and assume-unchanged files, changed/deleted sources, artifact tampering, missing
members, scope boundaries, private create-only output, directed diamond paths,
hop/node bounds, unique source quotes and reproducible recapture. Compiler
normalization/publication has its separate [verification record](COMPILER_EVIDENCE.md).

The final native gate passed `cargo test --workspace --all-targets`, strict
workspace Clippy, changed-file treefmt and the opt-in consumer experiment.
The disposable TypeDB 3.13.0 gate also passed: 13 backend tests, the compiler
fresh-reader test and CLI publication/query/refresh guards. mdBook builds with
all 11 chapters represented in the summary and valid shared-reference redirects.

The modified flake parses and passes its formatter. Production-package validation
via `canix cache binary build .#chaosbox --no-push` was blocked, including on retry,
by another session holding `/run/lock/canix/nix-eval.lock`. Realizing the package
and exercising its Git-enabled wrapper remains an explicit pending check in
`tasks/todo.md`; native test results do not establish that packaging gate.
The Jev follow-up retry on 2026-09-30 was also blocked by the same evaluation
lock (holder PID 2342763, building `.#provenance-oauth`).

The integration is intentionally reviewed and selected-file based. Compiler
resolution does not establish these cross-language packaging links. Nix
evaluation/deployment, general workspace graph publication, rename continuity,
incremental compiler resolution, calibrated memory admission and comparative
coding-agent improvement remain later milestones. The managed Graphify views
for Canix and Chaosbox were stale during this investigation; source inspection
supplied the bridge evidence.
