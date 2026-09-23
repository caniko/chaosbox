# OpenCode v1 → v2 Cutover Runbook

Operational runbook for the OpenCode store and Home Manager transition.

> **This is not `docs/CUTOVER.md`.** That file covers the Gel → TypeDB migration and is
> owned elsewhere. This runbook covers OpenCode's v1 → v2 store transition only.

**Status: prepared and rehearsed. Not executed.**
Production cutover belongs to the integration session and is gated on G1–G5.
`readyForCutover` stays `false` until G4.

- Prepared by: Session C
- Requires sign-off before implementation: [`SESSIONS-ADOPT-INSTALL.md`](./SESSIONS-ADOPT-INSTALL.md)
  (Session D for `adopt`/`install`, Session A for journal-v3 receipts)
- Authoritative design: `ProjectState/opencode-migration/2026-09-22-v2/PLAN-REVISION.md`

---

## 0. What and where

| Thing | Path |
|---|---|
| **Install target (restore destination)** | `~/.local/share/opencode/` |
| Target database | `~/.local/share/opencode/opencode.db` |
| Credentials that must survive **in place** | `~/.local/share/opencode/auth.json`, `account.json` |
| v1 live store (35.35 G + 0.48 G WAL) | same path as target — v1 *is* what is installed today |
| Canary store (isolated XDG) | `~/.local/share/opencode-v2-canary/opencode/opencode.db` |
| Campaign root | `ProjectState/opencode-migration/2026-09-22-v2/` |
| Staged build | `…/canonical-staging-FP6mrj/destination.db` (12.57 G) |
| Immutable reconciliation | `…/reconciliation.json` |
| Rehearsed tooling | `/data/scratch/tmp/opencode/cutover-c/` |

Tools in `cutover-c/`, one job each, each its own process:

| Tool | Does |
|---|---|
| `freeze.mjs` | stops writers, records what it stopped |
| `verify-holders.mjs` | pure check of `/proc/*/fd`; exit 1 names the holders |
| `snapshot.mjs` | checkpoints WAL, copies db only, records digest/counts/schema |
| `preserve-check.mjs` | `baseline` / `verify` for `auth.json` + `account.json` |
| `install.mjs` | restartable SQLite-aware publish from its own state file |
| `rollback.mjs` | one operation: freeze → store → package/config → verify |
| `hm-generation.mjs` | records / reverts the Home Manager generation |
| `hold.mjs` | rehearsal-only writer used to prove the holder checks fire |
| `build-delta-inventory.mjs` | measured delta: importable-new + changed, from read-only store reads |
| `build-final-counts.mjs` | sums canonical + variants + delta-new; never a headline figure + delta |
| `build-identity-v3.mjs` | pins the delta and counts, refuses if the chain of digests disagrees |
| `test-schema-guard.sh` | runs the guard *extracted from `default.nix`* against the real stores |

### Constraints that shape every step

1. **`user_version` is 0 on every store** — v1 live, canary and staging destination alike.
   It cannot discriminate. The schema discriminator is the **table set**: the v1-only tables
   are `message`, `part`, `session`, `session_share`, `todo`.
   Pinned fingerprints (per store, never global): v1 `bddf3a89…84af`,
   v2/canary `51505efa…832`. Note `stable`/`local`/`old-backup` are marker-`v1` but
   fingerprint differently — a fingerprint pins *one* store.
2. **Native import is create-only (409)** with `MAX_IMPORT_MESSAGES = 8192`. It cannot
   express an *update*, which is why changed sessions become supersession receipts.
3. **Counts are never arithmetic on `7848 + delta`.** They are recomputed from
   `reconciliation.json` plus a fresh delta taken at the final boundary.
4. **The canary is written by four concurrent workstreams.** It grew 15 → 28 sessions and
   1720 → 2483 messages *during a single preparation session*, and delta-new moved 63 → 65
   over the same window. Every figure below is point-in-time and must be recomputed after
   the freeze.
5. **`auth.json` and `account.json` are never rewritten.** The install replaces the database
   file and nothing else; a baseline/verify pair proves it on both sides.

---

## 1. Preconditions

| # | Check | Command | Expect |
|---|---|---|---|
| 1.1 | Tooling | `command -v node flock sha256sum alejandra nix-instantiate` | all present |
| 1.2 | Disk headroom | `df -h /home /data/scratch` | ≥ 30 G free on `/home` (target holds 12.57 G + backup) |
| 1.3 | Interfaces signed off | Session D on `adopt`/`install`, Session A on journal-v3 | signed |
| 1.4 | Variant IDs deterministic | Session B, §3.5 assertions | 204 materialised, ids disjoint |
| 1.5 | Verifier resolves chains | Session A, §3.1 both directions | each session reported once |
| 1.6 | `reconciliation.json` digest | `sha256sum` | `7ecd9b6e3f979d0981cc5963b3db9787d89edad9dc911a6788c67f6456112049` |
| 1.7 | Frozen driver byte-identical | `sha256sum assemble-canonical-history.mjs` | `62f6fb3544875b4f22c63d3df315e2c7d5106eff21bedc26c7e1fd696b9f6604` |
| 1.8 | `sessionHash` behaviour unchanged | `history-transfer-support.mjs` review | unchanged |
| 1.9 | Credentials baseline | `node preserve-check.mjs --dir ~/.local/share/opencode --record … --phase baseline` | exit 0 |
| 1.10 | Staging destination healthy | read-only `quick_check` + `foreign_key_check` | `ok`, `0` violations |
| 1.11 | Nobody holds staging | `node verify-holders.mjs --db …/destination.db` | exit 0 (Session A's verifier must be stopped first) |

**Verify 1.9** is the credential guarantee for the whole run: it writes the baseline
`auth.json`/`account.json` digests that §6 re-verifies after install and §8 re-verifies after
rollback. If it cannot record a baseline, stop — a missing credential is a condition to fix
before installing, not to discover afterwards.

---

## 2. Freeze sequence — three boundaries

Each boundary is **three externally-observable processes in order**, never steps buried in a
script that also does other work:

```
freeze.mjs     → stops the writers, records what it stopped     (its own exit code)
verify-holders.mjs → proves no fd still holds the database      (exit 1 names pid + fds)
snapshot.mjs   → checkpoints, copies, records the digest        (refuses if held or unhealthy)
```

The freeze and the check are deliberately separate: a freeze that also "checked itself"
would be the only witness to its own success.

### Boundary 1 — v1 store

```bash
C=/data/scratch/tmp/opencode/cutover-c
V1=~/.local/share/opencode/opencode.db

# 1.1 Freeze. opencode.service has Restart=always, so STOP it — killing the pid
#     would have systemd restart it and reopen the database.
node $C/freeze.mjs --db $V1 --record b1-freeze.json --boundary v1 --mode unit --unit opencode.service
# 1.2 Verify, independently.
node $C/verify-holders.mjs --db $V1 --json > b1-holders.json          # exit 0 required
# 1.3 Snapshot, chained onto the previous boundary if there is one.
node $C/snapshot.mjs --db $V1 --out b1.db --record b1.json --boundary v1
```

**Verify 2.1** — `b1-holders.json` has `"clear": true`, and `b1.json` carries
`snapshotSha256`, `schema.marker = "v1"`, `quickCheck = "ok"`, `holdersBefore = 0`,
`holdersAfter = 0`. `snapshot.mjs` exits non-zero if a holder appears *during* the copy, if
the source and copy byte counts differ, or if the copy's `quick_check` is not `ok`.

### Boundary 2 — canary store

```bash
CAN=~/.local/share/opencode-v2-canary/opencode/opencode.db
node $C/freeze.mjs --db $CAN --record b2-freeze.json --boundary canary \
     --mode match --match "opencode" --wait-ms 10000
node $C/verify-holders.mjs --db $CAN --json > b2-holders.json
node $C/snapshot.mjs --db $CAN --out b2.db --record b2.json --boundary canary \
     --supersedes b1.json
```

> ⚠️ **The canary freeze terminates every canary session — including the session running
> this cutover preparation.** The canary store backs the interactive `opencode` sessions of
> all four workstreams. This boundary is therefore an *integration-session* action taken
> after the other workstreams have handed off, never something run mid-session.

**Verify 2.2** — `b2.json` has `supersedes.boundary = "v1"` and a distinct
`snapshotSha256`; `b2-holders.json` is `clear`.

### Boundary 3 — install target (last look before replacement)

```bash
node $C/freeze.mjs  --db $V1 --record b3-freeze.json --boundary target --mode none
node $C/verify-holders.mjs --db $V1 --json > b3-holders.json
node $C/snapshot.mjs --db $V1 --out b3.db --record b3.json --boundary target --supersedes b2.json
```

`--mode none` records that the boundary is already frozen by hand and still verifies — it
performs no action, it does not skip the check.

**Verify 2.3** — the supersede chain `v1 ← canary ← target` resolves (`b3.supersedes`
points at `b2`, `b2.supersedes` at `b1`), and every record carries `createdAt`,
`snapshotSha256`, `schema`, `counts`, `quickCheck`. (Confirmed against the rehearsal
records: `b1.supersedes = null`, `b2.supersedes = b1`, `b3.supersedes = b2`.)

> Boundary 3's digest is what §8 rollback restores *to*. It is recorded before the install,
> not observed afterwards — the file present at rollback time is the one being replaced.

---

## 3. Delta inventory and final counts

The delta is computed from **fresh snapshots of the live stores**, never from
`work/primary.db` (stale: 7422 sessions) and never from `snapshots/primary.db`
(883 mismatches against reconciliation).

Prepared figures — **PRELIMINARY**, measured before the final freeze (recomputed 2026-09-23
with content detection on; the message total moved 609 153 → 609 542 between runs because
the stores were still growing — the point of recomputing at the boundary):

| | |
|---|---|
| Canonical (reconciliation) | 7 644 sessions / 560 595 messages |
| Divergent variants to materialise | 204 / 35 413 messages (`measured: false` until Session B's artifact — §5.1 gates on this) |
| Delta-new (union of v1 ∪ canary, overlap 0) | 65 sessions / 13 534 messages |
| Already in staging destination | 0 ⇒ **no 409** for new imports |
| Changed since the boundary | 886 (885 count-changed: 884 v1 primary + 1 canary; **plus 1 content-only**: same count, rewritten content — invisible to every count comparison before `--deep`) |
| Changed added/removed messages | +4 277 / −27 |
| Reference skew (reported, never merged) | primary snapshot postdates reconciliation by +9 / +5 482; canary exact |
| **Total** | **7 913 sessions / 609 542 messages** |

Composition: `7644 + 204 + 65`. Never `7848 + delta`.

These figures are **derived, not transcribed** — `build-final-counts.mjs` sums the three
terms out of `reconciliation.json` and `delta-inventory.json`, and the sum is checked against
the file afterwards. Regenerating the whole chain is three commands:

```bash
C=/data/scratch/tmp/opencode/cutover-c
R=/data/nvme0/can/ProjectState/opencode-migration/2026-09-22-v2

# --deep is not optional for final: without content comparison a rewritten session
# with an unchanged message count is reported as unchanged, which is how the
# earlier 885 missed one. --status final additionally requires --boundaries whose
# records actually claim the inputs (by snapshot path, digest-verified by hashing
# the file, not trusted from the record) -- see test-delta-gates.sh (30/30).
node $C/build-delta-inventory.mjs --reconciliation $R/reconciliation.json \
     --v1 ~/.local/share/opencode/opencode.db \
     --canary ~/.local/share/opencode-v2-canary/opencode/opencode.db \
     --staging $R/canonical-staging-FP6mrj/destination.db \
     --v1-ref b1.db --canary-ref b2.db --deep \
     --out delta-inventory.json --status final --boundaries b1.json,b2.json,b3.json

# --variants is Session B's artifact (gate G2), the only evidence for that term:
# --status final refuses command-line numbers, and refuses a preliminary delta.
node $C/build-final-counts.mjs --reconciliation $R/reconciliation.json \
     --delta delta-inventory.json --out final-counts.json \
     --variants variants-b.json \
     --status final --boundaries b1.json,b2.json,b3.json

node $C/build-identity-v3.mjs --identity $R/canonical-staging-FP6mrj/journal-v2/identity.json \
     --reconciliation $R/reconciliation.json \
     --delta delta-inventory.json --final-counts final-counts.json \
     --out identity-v3.json --status pinned
```

`build-identity-v3.mjs` refuses to pin if any link in that chain disagrees: it verifies
`reconciliation.json` is still `7ecd9b6e…`, that the delta was built against *that*
reconciliation, and that `final-counts.json` was built against *that* delta. A total
assembled from mismatched inputs is precisely the failure this guards against.

**Changed sessions are not added to the count** — they become supersession receipts
(§4.2). The earlier "1111 changed" figure compared non-primary sources and is superseded.

Write the recomputed figures to `final-counts.json` with `status` naming the boundary
snapshot they were derived from.

**Verify 3.1** — `final-counts.json`:
- `derivedFrom` names `reconciliation.json` **and** the boundary record digest;
- `finalCounts.sessions == canonical + variants + unionNew`, each term present;
- `variants.measured == true` with a `source` digest for final; `false` with a pending
  reason for preliminary — and no gate may consume a total whose variant term is unmeasured;
- `changedSessions.total` appears **only** under `boundaryDelta`, never in `finalCounts`;
- cross-check against staging `api-validation.json` + `schema-validation.json` plus the delta.

**Verify 3.2** — recompute twice, after boundary 1 and after boundary 3, and require the two
`finalCounts` to match. A count that moves between boundaries was measured against a store
that was still growing.

---

## 4. Delta import

### 4.1 New sessions (65)

Native import, create-only. Because `newIdsAlreadyInStagingDestination == 0`, no 409 is
expected. Sessions over `MAX_IMPORT_MESSAGES = 8192` use the oversize inventory path.

**Verify 4.1** — for every imported id, a receipt exists in `journal-v2/`; re-importing the
same id is *expected* to 409, which confirms create-only semantics were honoured rather
than silently overwriting.

### 4.2 Changed sessions (886) → `journal-v3/`

Native import cannot express an update (409), so a changed session is **not** re-imported.
It is superseded: `journal-v2/` keeps base receipts, `journal-v3/` gains a supersession
receipt, and the verifier resolves exactly one effective receipt per session by following
the chain to its head.

Two receipts for one session is normal. A **cycle** or a **dangling pointer** is an error.

> ⚠️ **Open design — do not run against the real destination yet.** The transfer *mechanism*
> for the newer content still needs a rehearsed §3.1 procedure. What is settled is the
> receipt shape and the verifier contract, which are in
> [`SESSIONS-ADOPT-INSTALL.md`](./SESSIONS-ADOPT-INSTALL.md) pending Session A's sign-off.

**Verify 4.2** — rehearse on **copies** first (§3.1 requires it): the supersession receipt is
written, and the verifier resolves correctly from both the old head and the new head.

**Verify 4.3** — `chaosbox sessions verify` reports each of the 7 913 sessions **exactly
once**, with `integrityVerified` set only when `quick_check` and `foreign_key_check` both
pass and the recomputed `sessionHash(destination, id)` equals the *effective* receipt's
`destinationDigest`.

---

## 5. Pre-install gate

All must pass before §6 runs. This is the last point at which proceeding is cheap to reverse.

| # | Gate | Expect |
|---|---|---|
| 5.1 | `final-counts.json` recomputed at boundary 3 | matches boundary 1 count (Verify 3.2) |
| 5.2 | Staging destination | `quick_check` `ok`, FK violations `0` |
| 5.3 | Staged schema | marker `v2`, fingerprint `51505efa…832` |
| 5.4 | Session A's verifier stopped | `verify-holders.mjs --db destination.db` exit 0 |
| 5.5 | Nothing holds the install target | `verify-holders.mjs --db ~/.local/share/opencode/opencode.db` exit 0 |
| 5.6 | Credentials baseline recorded | `preserve-check.mjs --phase baseline` exit 0 |
| 5.7 | Boundary 3 record exists and is chained | Verify 2.3 |
| 5.8 | Rollback input present | `hm-generation.mjs --record … --read` exit 0 |
| 5.9 | Interfaces signed | Sessions D and A |

**Verify 5** — every row above exits 0. Any non-zero stops the run here, where nothing has
been replaced yet.

---

## 6. SQLite-aware publish

`install.mjs` runs phases in order, **writing the state file before the next phase starts**,
so an interrupted install resumes from its own state rather than from whatever partial files
happen to be on disk. A completed phase is skipped only when its recorded artifact still
exists *and* still digests the same way.

```bash
node $C/install.mjs \
  --dir ~/.local/share/opencode \
  --source …/canonical-staging-FP6mrj/destination.db \
  --state ~/.local/share/opencode.install-state.json \
  --expect-sessions 7913 --expect-messages 609542 --expect-user-version 0
```

The `--expect-*` figures are transcribed from the §5.1 recompute, not from this
document: if the recompute moves (the stores grow until the freeze), the command moves
with it and the state file — not this page — is the authority afterwards. A resume that
supplies a different `--expect-*` is refused rather than silently adopted.

| Phase | What it does |
|---|---|
| `preserve-baseline` | records `auth.json` / `account.json` digests |
| `holders-clear` | refuses if anything holds the target — **does not stop writers itself** |
| `checkpoint` | `PRAGMA wal_checkpoint(TRUNCATE)` on the existing store |
| `backup` | copies the existing store aside; records digest **+ pre-install counts and schema** |
| `stage` | copies the source onto the **target filesystem** (records `dev`) |
| `validate-stage` | read-only `quick_check`, `foreign_key_check`, marker `v2`, pinned fingerprint, `user_version`, session/message counts vs expectations |
| `install` | records intent, then same-filesystem renames: aside, then in |
| `post-validate` | installed digest == staged digest, health re-checked, counts re-read |
| `preserve-verify` | `auth.json` / `account.json` digests unchanged |
| `done` | |

Why the ordering matters:

- **Stage on the target filesystem**, then rename — preparing elsewhere and copying across
  filesystems silently breaks the assumption that rename is atomic.
- **The backup records the pre-install shape** because that is the last moment it exists.
  Rollback restores to *that*, not to whatever the file contains afterwards.
- **Intent is persisted before the first rename**, so a crash between the two renames is
  resumable from state instead of guessed at from disk.

**Verify 6.1** — state file has all ten phases; `post-validate.sha256 == stage.sha256`;
`backup.sha256 != stage.sha256`.

**Verify 6.2** — restartability, three ways:
1. `--stop-after stage` exits `3`, state marked `interrupted`, credential baseline present;
2. rewrite `state.phase` to a mid-flight value (a *crash*) → rerun without `--resume` exits
   `2` (refuses to guess); with `--resume` it completes;
3. a third run reports `already complete; nothing to do` and does **not** reinstall.

**Verify 6.3** — `preserve-check.mjs --phase verify` exits 0 (`preserved: …` for both files).

**Verify 6.4** — installed store reads back: marker `v2`, `quick_check` `ok`,
session count == `final-counts.json`.

---

## 7. Home Manager transition

One flag moves package, service and launcher **together**, and is `false` until G4 — so a
rebuild before cutover changes nothing.

```nix
opencodeV2Cutover = false;   # gate G4 → true
```

| Change | File | Effect |
|---|---|---|
| `opencodeBase` → v2 candidate | `home/profiles/development/default.nix` | repoints `programs.opencode.package` **and** the `opencode.service` `ExecStart` (which already passes `--hostname`/`--port`, both accepted by v2) |
| Canary XDG override retired | `opencode-v2-canary.nix` | `launcherXdg` = standard XDG ⇒ store lands at `~/.local/share/opencode/` |
| `opencode` shadow retired | `opencode-v2-canary.nix` | `ln -s … opencode` is emitted only pre-cutover; `opencode` then resolves to `programs.opencode.package`, ending the CLI/store split |
| `opencode-db-prune` retired | `default.nix` | gated on `!opencodeV2Cutover`; its SQL updates `part`, absent on v2 |
| Schema guard added | `default.nix` | runs before any prune/compaction, refuses an unrecognised shape |

> **Correction to PLAN-REVISION §3.3:** it claims `programs.opencode.package` already points
> at the v2 candidate. It does not — line 130 resolves to `pkgs.opencode`, and the live
> service runs `opencode-1.18.31`. The repoint is real work, not a no-op.

**Verify 7.1** — `nix-instantiate --parse` on both files, `alejandra --check` on both.

**Verify 7.2** — schema guard against the real stores (already rehearsed: **15/15**):

```bash
node $C/test-schema-guard.sh
# v1 store: compact + prune permitted
# v2 destination / canary: prune REFUSED ("part" absent)
# mixed part+session_v2 shape: refused, not guessed at
# missing db / unknown action / no args: fail closed
# invocation: the guard is called as ${opencodeSchemaGuard} "$db" (writeShellScript
#   output *is* the script file -- the old .../bin/... spelling cannot resolve,
#   and two assertions prove both spellings' fate)
```

**Verify 7.3** — after flipping the flag and rebuilding:
```bash
readlink -f "$(command -v opencode)"     # must NOT be *opencode-v2-canary-launcher*
systemctl --user is-active opencode      # active, running the v2 candidate
```

**Verify 7.4** — `git status` in canix shows **no** change to `.envrc`,
`.skills/atlas-crash-postmortem/SKILL.md`, `flake/dev_shells.nix`,
`root/hosts/atlas/hardware/boot.nix`, or `docs/CUTOVER.md`.

**Verify 7.5** — both switch states evaluate and the affected unit builds
(`eval-both-switch-states.sh`, **11/11**): the `false` state yields the prune unit whose
built `ExecStart` invokes the guard without `/bin/` (the referenced store path exists and
is executable); the `true` state evaluates to a different derivation with the prune unit
absent (retired — its v1 SQL must never run on a v2 store). A full `activationPackage`
build fails in *both* states on the unrelated `canix-0.1.0` Rust crate (exit 101) and is
therefore not the gate; the unit build above is.

---

## 8. Validation

| # | Check | Expect |
|---|---|---|
| 8.1 | Service health | `opencode.service` active, `/api/health` 200 |
| 8.2 | `quick_check` on installed store | `ok` |
| 8.3 | `foreign_key_check` | `0` |
| 8.4 | Session count | equals `final-counts.json` |
| 8.5 | Message count | equals `final-counts.json` |
| 8.6 | Known session reads back through the **native API** | paginates to completion |
| 8.7 | Variants readable natively | all 204, not merely counted |
| 8.8 | `chaosbox sessions verify` | each session exactly once (Verify 4.3) |
| 8.9 | **Credentials** | `preserve-check.mjs --phase verify` exit 0 |
| 8.10 | Store location | `~/.local/share/opencode/opencode.db`, no `-v2-canary` path |

8.6 and 8.7 are deliberately *native reads*: a row that exists but cannot be paginated back
is not materialised.

---

## 9. Rollback

One operation covering **package, config and store together**. Restoring the old database
while the service still pointed at the new package would leave a mismatched pair — which is
why §7 puts all three behind one flag and this command reverses them together.

New writes are frozen **before** any restore starts; a writer still attached is a hard
refusal, not a warning.

```bash
node $C/rollback.mjs \
  --state  ~/.local/share/opencode.install-state.json \
  --record rollback.json \
  --config-live hm-live.json --config-baseline hm-baseline.json \
  --config-restore "home-manager switch --switch-generation <N>"
```

`--config-baseline` is recorded *before* the cutover (`hm-generation.mjs --snapshot`);
`--config-live` names the record the probe refreshes. All three `--config-*` arguments
are required — a store-only rollback that reported ok would leave the service pointed at
the wrong package, which is the mismatched pair this one operation exists to prevent.

Order, enforced:

1. **freeze** — invoke the freeze process, then re-check holders; refuse if any remain
2. **store** — verify the backup still digests as the install recorded it, and that the
   state records a pre-install shape; stage the restore copy beside the target and digest
   it *before* the live file is touched, rename current aside then staged in, delete the
   foreign `-wal`/`-shm`, then `quick_check`, FK check, counts vs **recorded pre-install
   counts**, fingerprint vs **recorded pre-install fingerprint**
3. **config + package** — run the recorded restore command, probe the live generation
   separately, and require it to **equal the baseline generation** ("it changed" is not the
   claim; "it went back to the one the baseline names" is)
4. **verify** — no holders, `quick_check` ok, marker equals the pre-install marker

Refuses when: the install state is not `done`; the backup or its digest is missing; the
state records no pre-install shape (there is nothing to restore *to*, so it will not guess).

**Verify 9.1** — rollback record `ok == true`, and
`phases.store.restoredSha256 == b3.json snapshotSha256` — the digest proves the store is
byte-identical to the pre-install boundary.

**Verify 9.2** — `preserve-check.mjs --phase verify` exit 0 (credentials survived rollback).

**Verify 9.3** — `phases.config.after.generation` **equals** the baseline generation (not
merely "changed from the cutover one" — a restore that lands anywhere else is refused).

**Verify 9.4** — `--dry-run` first when unsure: it walks every phase, records what it *would*
do, and touches nothing.

---

## 10. Evidence index

Rehearsed on private copies only; the live stores and campaign snapshots were re-digested
afterwards to prove they were untouched.

| Artefact | Result |
|---|---|
| `rehearse.sh` — three boundaries, install/interrupt/crash-resume, rollback, negatives | **0 failed** (51–52 passes, see below) |
| `test-schema-guard.sh` — guard extracted verbatim from `default.nix` | **15 / 15** |
| `test-delta-gates.sh` — content-vs-count detection + `--status final` refusals on fixtures | **30 / 30** |
| `eval-both-switch-states.sh` — both flag states eval, unit builds, restore byte-identical | **11 / 11** |
| `final-counts.json` | 7 913 / 609 542, marked PRELIMINARY, derived not transcribed (`variants.measured: false`) |
| `delta-inventory.json` | 65 importable new / 886 changed (885 count + 1 content-only), shapes `v1`/`v2`/`v2` detected, reference skew disclosed |
| `identity-v3.json` | DRAFT; pins delta + final-counts, carries `reconciliationDigest` unchanged |
| Boundary records with digests + supersede chain | `rehearsal/boundaries/b{1,2,3}.json` |
| Structural no-live-write proof | every absolute path in every record is under the rehearsal root |
| `nix-instantiate --parse` + `alejandra --check` on both edited nix files | clean |
| `cargo clippy -p chaosbox --all-targets -- --deny warnings` | see note |

> **Clippy note (2026-09-23):** `cargo clippy -p chaosbox --all-targets -- --deny warnings`
> exits `0` with zero warnings — the `chaosbox-typedb` refactor landed, and there are no
> findings in `crates/chaosbox/src/sessions/` either. Re-run before execution: another
> session's in-flight work can re-break it, and this gate belongs to the moment of use,
> not to the moment of writing.

### Rehearsal-only observations worth keeping

- The canary digest changed *between* two rehearsal runs — direct evidence the delta is a
  moving target and why the count must be taken after the freeze.
- **The count is 51 passed plus one INFO line when the canary moves mid-run (52 when it
  holds still), and never a failure.** One assertion (`rehearse.sh:337`) checks the live
  canary digest and deliberately degrades to an `INFO` line when that digest has moved,
  because the canary is written concurrently by the other workstreams and a rehearsal is
  not entitled to call that a failure. Quote **0 failed**, not a pass count, when citing
  this suite.
- The canary digest moving is **not** evidence of a live write by the rehearsal: four
  workstreams write it concurrently. The applied guarantee is structural (all recorded paths
  are under the rehearsal root), plus digest checks on the genuinely immutable inputs.
- `verify-holders` found the real holders directly: v1 pid 3935 (fds 14,15,16,22,23),
  canary pid 32361 (fds 13,14,15) — no `fuser`/`lsof` available, `/proc` scanning is the gate.

---

## 11. Handoff

Production cutover execution belongs to the **integration session**.

- Gates G1–G5; `readyForCutover` stays `false` until G4.
- G4 is the single `opencodeV2Cutover` flip (§7) — it moves package, service and launcher
  together, which is exactly what §9 reverses together.
- Blocked on: Session D sign-off (`adopt`/`install`), Session A sign-off (journal-v3),
  Session B variant IDs, and the changed-session transfer rehearsal (§4.2, open design).
