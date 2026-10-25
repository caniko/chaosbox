# S25a — installed consumer provenance audit

**The installed consumer uses pre-fix source, which explains the export-cap
failure.** The operator supplied the missing derivation excerpt; inspection of
its immutable source shows both CLI and MCP exporting the complete graph before
context selection. This is an early read-only slice of S25; promotion is pending.

See the [receipt](s25a-installed-provenance.json),
[S02 qualification](s02-bounded-navigation.md) and
[session workcards](../tasks/session-efficiency/todo.md).

## Actual session runtime

The native API returned session `ses_ebb5f8623ffe6QJYuMKK0fRcj1` with location
`/data/nvme0/can/canix`. Native service discovery selected
`http://127.0.0.1:4096`; the listening socket belongs to PID **2985490**.
The audit's shell processes have that same parent.

| Surface | PID | OpenCode executable version | ELF SHA-256 |
| --- | ---: | --- | --- |
| Serving this session on port 4096 | 2985490 | `2.0.12+f18083c78e` | `2d98c9d0a3ea61baf28a956601ac0ff415b094807a0fd3b65edf1aaa631f56da` |
| Observed systemd service process | 3464594 | `2.0.22+047c74e` | `b1994d029cec40c185fbf0765c0de1f82dbb1de3e4e27a1f3826eaa3a454db5c` |

S01b correctly identified its sampled service process. That process is not the
listener serving this session. Future session/custody receipts must use the
session's serving process rather than infer it from `systemctl MainPID` or PATH.
The loaded unit command and on-disk unit also differ; their separate observations
are retained without changing either.

## Configuration and executable chain

`opencode debug config` returned this location's configuration sources through
the native authenticated client. The global JSON declares Chaosbox; the global
JSONC and two ancestor/project JSONC documents contain no Chaosbox override.
`opencode api mcp.list` reports it connected for the exact Canix location.

The verified chain is:

```text
session location /data/nvme0/can/canix
  -> global opencode.json: mcp.servers.chaosbox
  -> /nix/store/5ksfi5dg17q91lda5nqfpf2sw5zhasgr-chaosbox-canix/bin/chaosbox-canix mcp
  -> /nix/store/aamrc152yl781cs8fy5nrk1nzgip19a7-chaosbox-0.1.0/bin/chaosbox
  -> /nix/store/aamrc152yl781cs8fy5nrk1nzgip19a7-chaosbox-0.1.0/bin/.chaosbox-wrapped
```

The observed Chaosbox child of the serving process is PID 2985990. Its ELF hash
is `7f4aa0f7b301b27e1677f6af29510cb653fce15c2e3cdd2b3c12f9d3853a6878`,
matching S02's installed binary. The sampled systemd process's Chaosbox child
uses the same ELF. The facade selects the loopback TypeDB database and credential
file; it does not rewrite query arguments. No credential values were collected.

The returned plugin package list has no Chaosbox compaction adapter entry. The
inspected configuration directories add none. This establishes configuration
absence for the observed sources, not native lifecycle compatibility; S09a still
requires its own proof.

## Fresh behavior check

The ordinary managed MCP call and installed facade both fail for:

```text
context(repo="canix", query="RuntimeManifest", depth=2,
        max_nodes=10, max_chars=6000)
consumer: export truncated at 10000 nodes; narrow the repo
```

The published `7313e6e5…` candidate succeeds against the same graph:
`build:8ac2e28136c3`, generation 3, snapshot `snap:cba2b1e91c07`.
Its 10-node, 5-link packet is byte-identical to S02, with SHA-256
`fd56f20814c7cd9358e8f4a971b9ee5b6b52278adc9c480ab3a87638ec594525`.
Truncation remains explicit and `exhaustive=false`.

The declared pin `9d6c81d…` and published candidate have identical navigation
source, SHA-256 `6d04a41c804e172012b0344b7fa881b69908296ab4351e9a0c47e3dec5544589`.
That proves the declared source contains the fix; it does not prove those source
bytes built the installed ELF.

## Installed source explains the mismatch

The operator-provided partial derivation JSON binds the installed output
`aamrc152…-chaosbox-0.1.0` to this source:

```text
/nix/store/9p9p6p8ds3zz8dpgvrx44kzqdc2yl3n4-source
```

Every one of its **151 files** matches the corresponding file in archived Git
revision `e6698a34c380b11c0216f3157a548868b7823f79` (2026-09-30). Its filtered
contents omit other Git paths, so this establishes byte identity for the complete
installed source without uniquely determining its original Git revision metadata.
There are no source symlinks. The selected-file manifest has SHA-256
`a72ad2fea189da5d0f99e5b8c75da1d59cf97b897350eec994fac9eedeb7a317`.

The shared failure follows directly from those source bytes:

- `crates/chaosbox/src/query_cmd.rs:177` calls `reader.export()` before the CLI
  navigation function.
- `crates/chaosbox/src/mcp_server.rs:379` does the same for MCP `context`, `stats`
  and `community`.
- `crates/chaosbox/src/reader.rs:67` sets the 10,000-node export cap.
- `reader_navigation.rs` is absent; the old pure navigation algorithm is
  `lexical-bfs-v1`, rather than the declared pin's bounded reader.

The installed source therefore predates the existing bounded fix. The current
declared pin contains that fix, but the active consumer still selects an older
realization. Reconciliation must select and verify the qualified output through
the managed consumer path; no additional navigation fix is needed for this case.

The original direct derivation inspection was denied before execution. The
operator's supplied excerpt resolved the source-attribution blocker without
retrying or bypassing that rejection. Its source/output binding is explicitly
operator-provided; the full derivation and build flags were not independently
collected.

## Evidence and handoff

`/data/scratch/tmp/opencode/chaosbox-s25a-20261016/` retains 39 checksummed
artifacts: native source projections, session/MCP state, process and socket
identities, wrappers, source copies, query results and limitations. The complete
configuration response was reduced to the relevant projection; its original
digest remains recorded. Provider configuration is not retained in that response.

Manifest SHA-256:

```text
1f759f6fa545d31cb42e74c1c7a3b8360f7a1f913d1826aaafc606b97606afb5
```

All checksums, session/configuration/MCP assertions, candidate packet comparison,
source comparison and before/after TypeDB identity/graph comparisons pass. Large
native CLI responses piped directly to `jq` were truncated; those failed captures
are recorded as limitations, not evidence. The complete file-backed configuration
response supplied the retained projection. Native default service discovery also
succeeded where an explicit unauthenticated server URL did not.

The `source-followup/` subdirectory retains **16 additional checksummed artifacts**,
including the supplied derivation excerpt, source-file manifest, immutable source
copies, Git archives, successful 151-file comparison and the original blocked
report/receipt. The original 39-artifact manifest remains unchanged. Follow-up
manifest SHA-256:

```text
6574d434953dcfe0ff35013f938e2f59282e6aab5110768b1c48ce7ca83964da
```

Reproduce the source comparison from `source-followup/revision-e669/` with
`sha256sum --check --quiet ../installed-source-files.sha256`. Both evidence
manifests and all 151 source comparisons pass.

This session changed only this report and its receipt in the checkout. Chaosbox
remains at `42f0e64f…` with its pre-existing tracked diff preserved. Another worker
committed the five Pink Raven storage-test files during the audit: Canix moved
from `5c845bc0…` to `d5613d4f…`, and its staged-diff fingerprint changed from
`42004496…` to `32ae1fa3…`. The relevant Chaosbox module/package declarations did
not change. This audit made no index writes or commits. No production service
was restarted, graph migrated, consumer promoted or pin changed. Numeric resource
limits remain deferred. Scoped treefmt passed with no formatter matching these
two documentation files; JSON assertions and `git diff --check` also pass.

Next independent workcard: **S03a**, audit the existing keyed header/membership
reads before choosing a change. S25 retains consumer qualification/promotion;
source attribution is explained. Bind S09a to the serving runtime identified here.
