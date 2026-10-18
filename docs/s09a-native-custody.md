# S09a — Native custody capability matrix

Status: **intended production runtime passes the isolated lifecycle proof;
the actual serving runtime is incompatible**, 2026-10-18. S09a and S25b remain
open. The [receipt](s09a-native-custody.json) binds the executables, adapter,
schemas, configuration and retained observations.

## Observed and tested runtimes

The listener on `127.0.0.1:4096` still belongs to PID **2985490**, running
OpenCode `2.0.12+f18083c78e`. The systemd unit names a different executable,
`2.0.22+047c74e`, with MainPID 3464594. This refresh agrees with the
[S25a provenance audit](s25a-installed-provenance.md). The current global plugin
configuration has no Chaosbox compaction entry.

Both isolated tests used the same packaged Chaosbox binary and adapter from
`64d65bea35e97f6221aeca8d809bacb740fb8779`. They created fresh backend homes,
projects, SQLite archives and disposable loopback TypeDB servers. The model was
the local credential-free `custody/custody-recorder` recording provider.

| Executable | Adapter loading and complete tool/reply custody | Checkpoint reaches model | Interrupted settlement and restart recovery |
| --- | --- | --- | --- |
| Actual listener's `2.0.12+f18083c78e` package, isolated r11 | Pass | **Fail: missing `compaction.plan`** | Not reached |
| Production `2.0.22+047c74e` package, isolated r10 | Pass | Pass | Pass |
| Native-check `2.0.22+047c74e` output, earlier isolated r8 | Pass | Pass | Pass |

The production r10 package carries `environment-recovery.patch`,
`amc-governor.patch` and `session-custody.patch`. The native-check r8 package also
carries `service-readiness.patch`; the newly edited checkout additionally requests
`session-cancellation.patch`. These are distinct tuples. The r10 result does not
qualify the checkout's later patch set or establish native TypeScript checking of
the new adapter against that patch set. A subsequent native-source check does
qualify the adapter against the clean custody host described below.

In r11, the backend log explicitly reports:

```text
Chaosbox requires the OpenCode compaction.plan extension.
```

The fixture then fails at its checkpoint deadline. This is an unsupported-runtime
result, rather than successful compaction inferred from plugin activation. The
adapter fails closed instead of invoking the older lossy compaction policy.

## Runnable lifecycle proof

Canix's `tests/opencode/chaosbox-custody.mjs` runs the real packaged backend,
configured adapter and Rust custody commands. Its r10 source and shared backend
helper are frozen in `tested-consumer-source.tar`, under the evidence directory
below. After extracting that archive, run from its root:

```sh
node tests/opencode/chaosbox-custody.mjs \
  --opencode /nix/store/lwlhs766ad8pi7hq82xkg521zcn8z5d5-opencode-v2-candidate-2.0.22+047c74e/bin/opencode \
  --chaosbox /nix/store/xad4nm8sny6axp23mk3c2nsmlfwz1pr3-chaosbox-0.1.0/bin/chaosbox \
  --plugin /nix/store/w0sbnq87avzirfd5b5kd6qmih10pckr6-plugins/chaosbox-compaction \
  --typedb /nix/store/g9hwrllc0d4gqmdwyjggv674kf0jdm7x-typedb-3.13.0/bin/typedb-server
```

The proof exercises pre-bounding custody of a **72,000-byte Unicode tool result**,
including exit-code metadata; final-reply custody without another request;
Rust-verified pruning plans; the exact checkpoint in a recorded model request;
and exclusion of the full tool body from that compacted request. It observes
live text through `session.text.delta`, interrupts that turn, waits for its
complete aborted record in custody, restarts the isolated backend, then recovers
the original Unicode output through paginated evidence reads. Private directory
and SQLite file modes are checked.

An earlier production attempt, r9, exposed a test-observer race: SQLite's default
zero busy timeout could fail while the independent Rust writer held its lock.
The observer now uses SQLite's bounded five-second busy timeout. The same
production test passes as r10 after that focused fix. Earlier r7 incorrectly
expected live text in persisted context; the corrected fixture uses native text
events and checks persisted content after interruption settlement.

The successful run used approved root direnv and a user scope with two CPUs,
`MemoryHigh=2G`, `MemoryMax=4G` and no swap. Those are execution containment,
**not measured production RAM or latency acceptance limits**.

## Live assessment and consumer gates

The credential-free proof uses `liveAssessment=false`, `publish=true`. Its final
status has **zero assessments, zero admitted records and 15 pending custody
items**. Publication being enabled does not establish TypeDB publication.
The separate live Jev diagnostic returned HTTP **402**, with the provider's
explicit no-credits billing message. Live assessment, admission and model-bound
retrieval of published knowledge remain unqualified.

The initial registered `chaosbox-compaction-consumer` build and full
registry profile both failed before realization: Git's filtered snapshot omitted the
other worker's untracked `root/packages/opencode-v2-candidate/session-cancellation.patch`.
The earlier selected pin reconciliation also reached the Roborev assertion
requiring `opencodeCancellationVersion=1`, absent from the currently pinned
Toolbelt package. Its recovery completed without changing the live pins.

The recovery sequence requires the actual runtime patch and tests in the
snapshot, then a qualified cancellation-capable Toolbelt revision. Refresh the
selected reconciliation request, load the reconciled root
direnv, rerun native typing and both consumer checks, and finish the registry
profile and authorized non-activating Atlas build. A separately authorized live
transition and executable-chain recheck are still required for serving acceptance.

## Retained evidence

`/data/scratch/tmp/opencode/canix-memory-consumer-20261018/s09a/` contains the
source archive, scoped consumer patch, listener/unit observations and a **31-file**
checksum manifest. Runtime r10 evidence is in
`/data/scratch/tmp/opencode/custody-consumer-5dlKOW/`; r11 is in
`/data/scratch/tmp/opencode/custody-consumer-e3MY6T/`. The manifest also binds the
executable/adapter/schema bytes, both configurations, native/Nix failures and
the retained HTTP-402 diagnostic. All checksums pass from the Canix root:

```sh
sha256sum --check --quiet /data/scratch/tmp/opencode/canix-memory-consumer-20261018/s09a/SHA256SUMS
```

Manifest SHA-256:
`9893b0fad0d84113985fedc020fb16d6f550eb37427e12ad6412cbe1ed5ac31c`.
The seven foreign staged Canix paths remain byte-identical to their pre-test
index snapshot. No production activation, service restart or live graph write
was performed. Broader S23/S24 resource and quality qualification remains open.

## Native typing follow-up

The producer's existing `scripts/test-opencode-compaction.py` now passes with
the new adapter and packaged Rust binary against clean OpenCode source
`db48c4233c2d6a8af49bcf1d28aa3202c77e3dd1` (base `047c74ec…`). It runs the
core's `bun run typecheck` and its native Promise-host regression: **1 passed,
0 failed, 17 assertions**. The installed `session-custody.patch` also passes
`git apply --reverse --check` against this host. The checkout remains clean after
the verifier's temporary fixture is removed. The initial launcher lacked
`python3` in Chaosbox's shell; the successful retry explicitly selects the
approved root environment's immutable Python interpreter.

The eight-file `native-typecheck-followup.sha256` manifest preserves both logs,
the verifier, adapter type surface, exact host patch and clean status. Its
SHA-256 is `98729b3b3a364348aa48efd4727fe84c463851325c8aec32f82698713fb1fe24`.
This adds source-host typing evidence; the current Canix patch set still needs
its own immutable native-package gate.

During this work the cancellation owner committed Canix `9f899586977f8036beffeb5d8bb270dd3362bee6`
and Toolbelt `b871b3ac9cd49b2aefa262bec43e65daff6f3240`. The missing runtime
patch is now tracked, so that earlier snapshot blocker is resolved. The Canix
pin still selects Toolbelt `36097d8f…`; qualification and consumption of the
cancellation-capable producer remain pending. The fresh full-profile diagnostic
after that source change fails manifest evaluation at the explicit Roborev
cancellation assertion. Its log is `full-profile-after-cancellation-commit.log`
under the parent evidence directory, SHA-256
`97c5ad13ec93a5db59e55225e4b2abef88586f564ada7ec3c55436511d33a606`.
Foreign staging also advanced during
the owner's work; the original index comparison above applies to the isolated
fixture runs, rather than asserting that no other worker subsequently staged.
