# S01 — session-efficiency baseline

S01 records the starting point for the session-efficiency work. The machine-readable
[receipt](s01-efficiency-baseline.json) binds the measured binaries, frozen corpus,
complete exports and local evidence. Measurements were taken on Atlas on
2026-10-16. This is a baseline, not acceptance of a production resource budget.

## Results

| Measurement | Installed facade binary | Published memory candidate |
| --- | ---: | ---: |
| Offline pipeline peak RSS | 754,616 KiB | 340,792 KiB |
| Wall time, one observation | 3.08 s | 2.96 s |
| GNU time filesystem input blocks | 3,912 | 42,000 |
| GNU time filesystem output blocks | 70,224 | 70,224 |
| Complete export size | 35,952,413 bytes | 35,952,413 bytes |
| Exit status | 0 | 0 |

Peak RSS fell **54.8%** on the same committed Canix corpus. `cmp` passed and both
complete exports have SHA-256
`78b2a22ffe28ad445578063604479dfcde8016ef278065c35d76504f41fd60b5`.
Both contain 65,872 nodes, 45,946 edges and the same build/snapshot identity.
This run uses the memory backend and no inference; it does not measure TypeDB
publication, server memory, session capture or context assembly.

The runs were sequential, baseline first, with one sample per binary. Page cache,
binary loading and background activity were uncontrolled. The input-block increase
and small timing difference are observations, not I/O or latency improvements.
GNU time block counters are not byte counts or whole-host disk traffic.

### Normal managed MCP calls

| Call | Outcome |
| --- | --- |
| `search(repo="canix", query="RuntimeManifest", limit=5)` | Five results in 1,328 ms; JSON-serialized result is 2,300 UTF-8 bytes |
| `context(repo="canix", query="RuntimeManifest", depth=2, max_nodes=10, max_chars=6000)` | Fails: `consumer: export truncated at 10000 nodes; narrow the repo` |
| Same context request through a fresh CLI process | Same failure, exit 1, 15.66 s, 94,336 KiB peak client RSS, empty stdout |

The search includes the `RuntimeManifest` definition at
`cli/crates/canix-foundation/src/runtime.rs:17`, entity `ent:574d2717a6af`.
Its snapshot matches the observed live build `build:8ac2e28136c3`, generation 3,
snapshot `snap:cba2b1e91c07`. MCP search duration includes the tool round trip;
its serialized-result size is neither provider tokens nor transport bytes.
The fresh CLI measurement does not measure the resident MCP host's RSS.

The live build has 1,714 captured files and 45,815 structural relationships;
compiler/catalog coverage is absent. Its export caps are 10,000 nodes and
20,000 edges. The bounded-context failure is a reproducible S02 starting case;
shrinking the repository would change the question being measured.
The live build was not verified against today's dirty checkout.

### Live TypeDB and host observations

At 13:33:49 UTC, the service reported **6.84 GiB cgroup RAM**, **21.76 GiB swap**,
a **19.62 GiB historical RAM peak**, and **1,282,472,673,280 cumulative write bytes**
since its 2026-10-15 11:23:16 UTC start. No service memory/swap limits are set.
These values are observations of an existing service, not costs of the offline
comparison or this session's reads.

A later cgroup sample separates anonymous memory (3,587,751,936 bytes), file cache
(1,107,288,064 bytes) and kernel memory (2,815,606,784 bytes). Its 2,722,478,888
bytes of compressed zswap memory overlap kernel accounting; do not add them again.
The samples are non-simultaneous and cannot be reconciled as an exact allocation
profile. Process `smaps_rollup`, process I/O and database disk usage were inaccessible
to this user and remain unknown. The evidence directory retains the empty failed
process capture; it is not a zero-usage measurement.

At 13:42:06 UTC the host had 35,402,043,392 bytes available RAM, but swap use was
40,179,482,624 of 41,184,194,560 bytes. The snapshot timer's service was running
during initial inspection and inactive by the final capture; the decisions unit
was failed. Host pressure and unit snapshots are retained. These conditions make
the host unsuitable for treating a new large run as an isolated experiment.

The deployed TypeDB 3.13.0 configuration has `cache-size: 1gb`,
`write-buffers-limit: 512mb` and MVCC cleanup disabled. Those cache settings are
not process caps. This observation does not establish that changing MVCC or
cache settings would fix the footprint.

## Exact identities and coverage

- **Measured installed baseline:** the binary executed by `chaosbox-canix`,
  `/nix/store/aamrc152yl781cs8fy5nrk1nzgip19a7-chaosbox-0.1.0/bin/chaosbox`.
  Canix still declares Chaosbox `9d6c81d202aa5e9b4e0a513161891e8a4ff01e96`.
- **Measured candidate:** published revision
  `7313e6e5feaf6f50dad9595d74aeaa691c556a24`, production derivation
  `/nix/store/xwgp6bib7fysy71bd2fvnybvgzclhfij-chaosbox-0.1.0.drv`, output
  `/nix/store/kg922178hx87ydnxn2nilmyiplaa5qyj-chaosbox-0.1.0`.
- **Bare `chaosbox` on PATH:** a third binary,
  `/nix/store/h9q6shkz3pbdqffbj576nf6wih9a55v8-chaosbox-0.1.0/bin/chaosbox`.
  Its source identity was not recovered and it was not substituted for either
  measured binary. The receipt hashes all three binaries and the facade wrapper.
- **Canix corpus:** a Git archive of committed
  `d4586fb6cd1da86be3f57f84da7f93aeea298263`, tree
  `313b77c1bf031dbeddbfe61c6b46a6626cb58456`, using every configured Canix
  source path listed in the receipt. Current dirty/staged changes are not part of
  this corpus. The selection-policy bytes match the frozen copy.
- **Canonical Chaosbox checkout:** dirty at
  `42f0e64fddd50f612516e39e82d5c2ea4d2ead45`; it was not rebuilt for these
  measurements. Its pre-existing tracked diff and status were retained separately.
- **Schema/readiness:** the installed facade's explicit `db check --repo canix`
  succeeds and reports packaged schema version 4 and the live build above.
  A check without `--repo` instead checks `demo` and reports pending: no active
  build. That negative does not diagnose Canix. Baseline/candidate schema source
  bytes match; a complete fingerprint of the applied live schema is unknown.
- **OpenCode:** the installed executable is
  `opencode-v2-candidate-2.0.12+f18083c78e`, with its full store path and hash in
  the receipt. Neither Canix's separate legacy input pin `047c74ec…` nor the dirty
  canonical fork checkout at `381d1bf4…` identifies those executable bytes.
  The global config hashes, observed Ponytail plugin hash and sanitized compaction
  settings are recorded. The effective layered session configuration, active
  Chaosbox custody adapter, native-hook compatibility and archive/journal identity
  were not verified. Working MCP tools do not establish native custody support.

The frozen offline export reports 1,721 captured files, 216 heuristic files,
1,621 omitted files and zero parse errors. The extractor's `omitted_files` is
processing accounting, not a list of facts removed for the benchmark. Zero parse
errors does not certify unsupported files or compiler resolution. Baseline and
candidate have identical coverage and complete output.

The selection inventory records **247 projects, 242 snapshot-eligible**. It keeps
all recorded projects and their policy scopes; it does not freeze every project's
source bytes. The original reported 1,186,553 facts / 14,459 files / 1,517 parse
errors remain historical workload observations, not a recreated S01 corpus or a
million-fact acceptance result. A small fixture archive from candidate revision
`7313e6e5…:fixtures/indexer-smoke` is also retained for later focused checks;
it was not measured here.

## Reproduce and verify

Evidence is at `/data/scratch/tmp/opencode/chaosbox-s01-20261016/`.
Its `SHA256SUMS` covers both archives, complete exports, metrics, coverage/status,
selection, readiness, resource observations and pre-existing Git-state snapshots.
The committed receipt binds the checksum manifest. Retain these artifacts, or
relocate them with checksum verification, before scratch cleanup.

The following examples explicitly use **Bash**. Verify the existing evidence:

```bash
cd /data/scratch/tmp/opencode/chaosbox-s01-20261016
sha256sum SHA256SUMS
# Expected: b259c5164e2ccf299f02785984594e8b5411f6af3b51f4a2cd4925258dbcc9d8
sha256sum --check SHA256SUMS
cmp baseline.json candidate.json
jq -e '.build_id == "build:760ac5e00503" and .generation == 1 and .snapshots == ["snap:5f76b151537a"] and (.nodes | length) == 65872 and (.links | length) == 45946 and .coverage.file_count == 1721 and .coverage.structural_relations == 45946 and .coverage.parse_errors == 0' candidate.json
```

To recreate the Canix archive, run from its canonical checkout, writing to a
**new** evidence directory. This uses Git's committed tree, not a live filesystem
copy with mutable build/workspace state:

```bash
git archive --format=tar --output="$EVIDENCE/canix-corpus.tar" d4586fb6cd1da86be3f57f84da7f93aeea298263 flake cli home lib pkgs root website docs/src/design docs/src/guides docs/src/operations docs/src/reference
sha256sum "$EVIDENCE/canix-corpus.tar"
# Expected: e1b54a89d07dd8fac441ea5ad55d9deb0de147a567048831b61fb1c1b01839cf
mkdir "$EVIDENCE/canix-corpus"
tar -xf "$EVIDENCE/canix-corpus.tar" -C "$EVIDENCE/canix-corpus"
```

With `EVIDENCE` set to that new absolute directory, run the baseline and candidate
sequentially. Verify executable hashes against the receipt first. These transient
user scopes bound only the offline child, not TypeDB or the host. They are safety
limits for this experiment, not proposed production budgets.

```bash
systemd-run --user --scope --quiet --unit=chaosbox-s01-baseline -p MemoryHigh=1G -p MemoryMax=2G -p MemorySwapMax=0 -p CPUQuota=200% /run/current-system/sw/bin/time -v -o "$EVIDENCE/baseline.metrics" env CHAOSBOX_DB_BACKEND=memory /nix/store/aamrc152yl781cs8fy5nrk1nzgip19a7-chaosbox-0.1.0/bin/chaosbox run "$EVIDENCE/canix-corpus" --repo s01-canix --max-candidates 40 --no-decisions > "$EVIDENCE/baseline.json"
systemd-run --user --scope --quiet --unit=chaosbox-s01-candidate -p MemoryHigh=1G -p MemoryMax=2G -p MemorySwapMax=0 -p CPUQuota=200% /run/current-system/sw/bin/time -v -o "$EVIDENCE/candidate.metrics" env CHAOSBOX_DB_BACKEND=memory /nix/store/kg922178hx87ydnxn2nilmyiplaa5qyj-chaosbox-0.1.0/bin/chaosbox run "$EVIDENCE/canix-corpus" --repo s01-canix --max-candidates 40 --no-decisions > "$EVIDENCE/candidate.json"
cmp "$EVIDENCE/baseline.json" "$EVIDENCE/candidate.json"
```

The existing `crates/chaosbox/tests/pipeline_memory.rs` and
[memory verification guide](memory-performance.md) already supply a runnable
generated/real-corpus regression. S01 needs no additional benchmark implementation.
The commands above retain outputs/metrics and use the exact production binaries.

For the live read case, first capture `chaosbox-canix query status --repo canix`
and `chaosbox-canix db check --repo canix --json`. Record any build movement before
comparing observations. Then issue the search/context calls shown above through
the managed MCP tools. The corresponding CLI regression is:

```bash
chaosbox-canix query context RuntimeManifest --repo canix --depth 2 --max-nodes 10 --max-chars 6000
```

S01 expects the recorded failure on the recorded build; future read-path work must
return bounded, source-backed results on that same scope and retain honest
truncation/coverage reporting. A new build requires a new receipt.

## Handoff and acceptance boundaries

**Next: S01b, resource attribution before substantial schema/storage work.** Its
single invariant is that client allocations, TypeDB anonymous/cache/kernel/zswap
memory, historical retained generations and background writers have separately
identified costs. Start with these snapshots and bounded read cases; obtain
authorized process/disk evidence where permissions blocked it. Establish agreed
RAM, I/O, swap, latency and context acceptance criteria from representative
observations. No absolute limits have been accepted by S01.

Later gates must preserve these distinctions:

- **S02 reads:** reproduce the context-cap failure with bounded returned results,
  without changing the selected repository or erasing the coverage denominator.
- **S08 workspace:** require automatic, persisted TypeDB-backed cross-project
  resolution, fresh-process traversal, distinct provider versions and a blocked
  bridge. The existing reviewed workspace artifact cannot establish this gate.
- **Native custody:** establish the supported runtime/adapter/journal tuple,
  cursor feasibility and copied legacy-archive interruption/restoration tests
  before replacing full-history capture. No session quality/token baseline was
  fabricated from this graph-only experiment.
- **S23/S24:** freeze all selected source bytes and bind resource and quality
  receipts to the same candidate, runtime, adapter, schema, configuration,
  corpus, task set and model identity. Relevant changes invalidate both receipts.

S01 changed only these baseline documents and local evidence. It did not consume
new Canix pins, migrate a database, activate Atlas or restart a service.
