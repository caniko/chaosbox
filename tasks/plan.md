# Scoped compiler evidence and integrated impact pilot

Continue the evidence-first sequence approved in the conversation. Tasks and
verification are tracked in `tasks/todo.md`.

## Jev-first inference follow-up

User priority: replace as much general-purpose LLM consumption as possible with
Typesafe Jev. Keep source/compiler facts deterministic. Graph proposals and
memory admission already use typed Jev; migrate the remaining session-research
extraction, repository-report and cross-repository synthesis calls to finite
choices over exact source spans and reviewed candidates. Render prose locally.
Reuse the native Rust client and pinned model; persist input-bound decisions,
abstentions and work-wide attempt budgets. Preserve source scopes, reviewed
actions and reproduction requirements. Verify with protocol/behavior tests and
a synthetic live transport smoke; model-quality/outcome claims remain separate.

## Strict Jev-only follow-up

User requirement: eradicate general-purpose LLM consumption from Chaosbox.
Enforce the approved Jev endpoint and exact pinned model in the shared live
client; disable redirects. Fresh decisions, cached inferences and graph
publication must reject substitutions before committing output. Explicit
offline fixtures keep their distinct identity and cannot enter the HTTP path.
Replace the generative continuation contract with Jev-selected exact records
and deterministic Rust rendering. Add a credential-free CLI capability report
and check it before research reserves attempts. Verify transport failure,
identity, cache/publication and preflight boundaries, then realize the production
package and record any downstream adoption blocker.

## Contract

1. Optional SCIP is genuine protobuf, decoded with the official `scip` crate.
   A capture receipt binds the index to the repository, source scope, source
   hashes, in-tree configuration hashes, producer and exact command. Capture
   brackets indexer execution with input checks. Import rejects stale receipts.
2. Compiler occurrences remain immutable and context-bound. Add logical symbol
   anchors without changing existing syntax entity identities. Global anchors
   retain full package-qualified symbols; local anchors include document and
   analysis context. No rename continuity is inferred.
3. Publish only references whose target has one in-scope definition occurrence.
   Retain unresolved/ambiguous/omitted accounting. References are not calls;
   compiler configuration membership and type-check success are not inferred
   from SCIP output. Support declared UTF-8/16/32 positions, typed ranges and
   legacy ranges; the known scip-typescript 0.4.0 UTF-16 override is explicit.
4. Persist compiler provenance, anchors and coverage alongside ordinary graph
   builds. Indexers remain optional external operator tools. Query/MCP stay
   read-only and do not launch indexers.
5. The early workspace pilot is an immutable local artifact with exact member
   build IDs, source-backed reviewed cross-language bridges, explicit private
   scope and cited constraints. A bounded impact query answers the selected
   command-response change question and exposes stale members. This pilot does
   not infer cross-repository relationships from matching names, nor claim
   model-admitted memory quality from a hand-reviewed constraint.

## Verification order

First prove normalization against adversarial and actual compiler fixtures;
then prove zero-model publication and TypeDB readback. Finally exercise the
real TS plugin/Rust CLI/Nix package/Canix consumer path through the pilot and
test stale inputs, absent targets, bounded traversal and scope enforcement.

## Boundaries

Configuration capture covers in-tree manifest/configuration files and explicit
declared inputs. External toolchains and environment-dependent/generated
inputs require declared context; coverage will not claim a hermetic build.
General workspace publication, incremental resolution, symbol correspondence,
and statistically calibrated memory/agent-outcome evaluation remain later
milestones in the reviewed roadmap.
