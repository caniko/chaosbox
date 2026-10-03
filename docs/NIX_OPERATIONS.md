# Nix operation provenance

`chaosbox nix add` provides reason-gated local additions, a durable private
invocation journal, offline operational intelligence and cleanup inspection.
It supports NAR additions (`nix-store --add` / `-A`, modern `nix store add`,
and `add-path`) and flat single-file additions (`--mode flat`, `add-file`).
It does not evaluate a flake or realise a derivation.

## Operator configuration and invocation

Configure `CHAOSBOX_NIX_WORK` (private existing parent, journal directory created
0700), `CHAOSBOX_NIX_SCOPE` (`private:OWNER`), `CHAOSBOX_NIX_HOST` and
`CHAOSBOX_NIX_BIN` (absolute pinned Nix executable). `CHAOSBOX_NIX_STORE` defaults
to `daemon`; isolated `local?root=ABSOLUTE` stores are supported for qualification.
The CLI cannot override these settings. A consumer facade must export them
authoritatively before granting an automatic shell allowance.

```nu
chaosbox nix add --reason "Compare this source candidate with the pinned input" --repo $env.PWD -- ./candidate
chaosbox nix add --mode flat --reason "Keep the exact fixture bytes for qualification" --repo $env.PWD -- ./fixture.txt
chaosbox nix context --repo $env.PWD candidate
chaosbox nix evidence --repo $env.PWD nix:OPERATION_ID
chaosbox nix query --path /nix/store/HASH-candidate
```

Every addition requires a nonblank reason of at most 4000 UTF-8 bytes, validated
before opening the journal or launching Nix. Arguments are typed and passed
without a shell; paths follow `--`. The local store and IFD/build policy are
fixed in argv, and ambient `NIX_*` variables are removed from the child.

Optional `--id` provides exact-retry identity. `--session`, `--message` and
`--tool-call` retain caller-supplied native associations; the standalone CLI
cannot independently authenticate those assertions. The required repository
association is explicit. Directory paths are useful when a consumer adapter
retrieves operational context for the current session location.

## Evidence, uncertainty and retry

The private SQLite journal uses synchronous FULL transactions, binds to one
scope/host/store and commits the immutable intent before process creation.
A unique operation id and canonical request digest arbitrate concurrent retries.
Observed execution is committed before optional metadata capture; settlement is
separate and append-only. A failed final write preserves the returned path and
actual outcome, and an exact retry reconciles those facts without running Nix.
Exact settled retries return the original receipt, even if the input has since
changed. Conflicting identity
reuse fails; an unsettled intent is exposed as unresolved and never rerun.

Receipts retain the required reason, executable and argv, actual exit outcome,
bounded stdout/stderr with full-stream hashes and omitted byte counts, returned
store path and recoverable NAR metadata/references. Missing metadata is explicit:
the path can be collected immediately after an unrooted addition. A spawn failure
is `not-started`; a native failure is `failed`; process signals are `interrupted`.
A timeout or capture failure after launch is `unresolved` because partial store
effects may exist. Abrupt runner death leaves a durable unsettled intent.

Each child has a bounded lifetime and kill-on-drop ownership. Settlement write
failure returns an error and leaves the durable intent and any committed
execution evidence for inspection. The
ledger and store have no shared transaction; success does not claim exhaustive
daemon-side effect capture or exclusive object ownership.

`context` and `evidence` are local read-only operational-intelligence interfaces,
independent of TypeDB and Jev. Context returns native reasons and versioned
scope/host/store/repository evidence handles. It is bounded lexical retrieval,
not a semantic conclusion or exhaustive recall. The `chaosbox-nix` OpenCode V2
adapter exposes these queries and injects selected historical evidence during
Nix-related session continuations. Retrieved material is derived evidence and
must not become independent corroboration for itself.

The structured `chaosbox_nix_add` tool delegates to the native OpenCode `shell`
leaf. The inspected producer `047c74ec64ca6029f93be4bb4d1f5797c8838b9a` preserves
native services and interruption signals through its public Promise tool bridge;
the shell leaf asserts permissions against parsed command resources before
spawning. Tool visibility alone does not grant execution. Denying the facade's
`nix add` shell resource prevents mutation even when retrieval tools are allowed.
Missing native shell support fails closed. Native session/message/call identities
are captured automatically and the request carries no arbitrary executable,
store, journal, repository or Nix-argument selectors.

## Cleanup contract v1

Additions preserve native **unrooted** retention. They do not create permanent
roots, holds, profiles or automatic release obligations. A completed session
does not imply that an object is disposable. Identical content can have multiple
invocations with independent reasons; historical receipts survive collection.

`query` returns at most 100 invocations per page, exact-path consumer filtering,
current filesystem presence (`present` / `absent` / `unknown`), unrooted retention,
empty owned roots, `needs-review` disposition and unknown registered validity/GC
eligibility. Filesystem presence is not a validity check. `cleanup_authorized`
is always false. Doty's `nix-store` command consumes the same versioned contract
for inspection only. Neither reader initializes the journal or performs store
writes, assessment, root deletion or GC.

Explicit held retention is a separate capability: it must establish continuous
temporary-root-to-permanent-root protection and transactional generation-bound
release before a cleanup consumer can support it. It is not approximated by
adding a symlink after an unrooted command exits.

## Other operation candidates

Purpose tracking should follow materialization and lifecycle effects rather than
whether a command is spelled like a query. The following inventory is an
expansion guide; only ordinary/flat addition is implemented in contract v1.

| Family | Candidates | Receipt and ownership requirements |
|---|---|---|
| Fixed-output additions | Legacy `--add-fixed`; `nix store add --hash-algo/--mode` | Preserve algorithm, addressing method, expected/observed content identity and exact returned path. |
| Direct downloads | `nix store prefetch-file`, `nix-prefetch-url` | URL, expected hash, unpack policy, final hash/path and partial-download outcomes. Distinguish fetch cache from registered store objects. |
| Flake source materialization | `nix flake prefetch`, `prefetch-inputs`, local `archive` | Exact source and locked input graph; Git filtering, lockfile-write policy, IFD/resource bounds and per-input progress. Recursive prefetch returns aggregate status without per-input JSON, so stdout alone is insufficient. |
| Archive transfers | `nix flake archive --to` | Separate local fetching from destination publication, record both store identities and destination observations. |
| Store import/transfer | `nix copy`, legacy `--import`, `--restore`, `--load-db`, `--register-validity` | Source/destination, closure, signature policy and actual registration. Flake installables can realise before transfer; an exact-path tool must reject them. NAR restoration alone does not register valid objects. |
| Derivation registration/rewriting | `nix derivation add`, `nix-instantiate`, `nix store make-content-addressed` | Preserve derivation source, registered derivation identity and old-to-new object/reference mapping. Capture dry-run versus actual registration. |
| Build and deployment producers | `nix build`, legacy realisation/build, Canix build/rebuild/Crossbow | Existing owners execute. Attach provenance to their persisted attempts, selected derivations, output receipts, GC roots, transfers and activation leases rather than building another execution engine. |
| Environments/execution | `nix develop`, `shell`, `run`, `print-dev-env`, `nix-shell`, approved direnv | Fetch/evaluation/realisation and environment-cache roots; arbitrary child execution retains its own authorization. Approved preparation is not necessarily zero store writes. |
| Profiles and registry | Profile changes, `nix-env`, registry add/pin and remote registry fetches | Generation/profile ownership, retention and replacement/deletion lifecycle. Canix's local registry listing is ordinarily local; remote-registry configuration changes that fact. |
| Explicit roots | Build out-links, profiles, legacy `--add-root` / `--indirect` | Consumer obligations and exact root identities, continuous protection and synchronized release. Never remove another producer's roots. |
| Query-shaped producers | Flake metadata/show/search/path-info, evaluation fetch builtins and `builtins.path` | Sources can be copied/fetched even with IFD disabled or a dry run. Connect guarded producer receipts and state capture; do not infer zero writes from command name. |
| Maintenance and removal | Repair, verify, GC, optimisation, signing | Separate mutating maintenance/cleanup purposes and measured outcomes. Legacy `nix-store --verify` can repair DB inconsistencies without `--repair`. Store entry count and NAR bytes do not establish reclaimable or physically freed bytes. |

Consumers must check runtime schema versions as well as package capability
metadata. Qualify exact upstream revisions before changing fleet pins or
allowances. A configured adapter is not proof that it has been activated.
