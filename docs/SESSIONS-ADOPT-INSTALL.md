# `sessions adopt` / `sessions install` — interface contract

**Status: PROPOSAL — pending sign-off.** The Rust CLI surface below is *not* implemented;
the tools it wraps (`install.mjs`, `rollback.mjs`, `preserve-check.mjs`, `hm-generation.mjs`)
**are**, and every claim this contract makes about their arguments, state schema and exit
codes has been executed rather than asserted — see §6 and the companion runbook.

| Part | For sign-off | Owner |
|---|---|---|
| §1–§3 `adopt` / `install` / `rollback` | **Session D** (integration, gates) | Session C |
| §4 `journal-v3` supersession receipts | **Session A** (verifier) | Session C |

Companion document: [`OPENCODE-CUTOVER.md`](./OPENCODE-CUTOVER.md) — the runbook that
*uses* this interface.

Existing surface, unchanged by this proposal
(`crates/chaosbox/src/sessions/cli.rs`):

- `sessions status [--root]`
- `sessions verify [--root] [--limit N] [--session ID]… [--sources] [--allow-partial]`
- entry point `sessions::cli::run(command) -> Result<i32, String>`
- both subcommands are **read-only**: neither opens a database for writing

---

## 1. `sessions adopt` — register a store as a campaign source

**Purpose.** The migration reasons about several stores (v1 live, canary, staging
destination, install target). Adoption pins one of them into the campaign as a named,
digest-addressed input so downstream steps reference `staging` rather than an ad-hoc path
that may have moved.

**Read-only.** Adoption never writes to the adopted database.

```
chaosbox sessions adopt --name <key> --db <path> [--root <campaign>] [--allow-held]
```

| Arg | Meaning |
|---|---|
| `--name` | required, stable key: `v1` \| `canary` \| `staging` \| `target` |
| `--db` | required, absolute path to the store |
| `--root` | campaign root; defaults to `$CHAOSBOX_SESSION_CAMPAIGN` |
| `--allow-held` | admit a store that still has open fds, recorded as `held: true` |

**Writes** `<root>/adoption/<name>.json`:

```json
{
  "name": "staging",
  "dbPath": "/…/destination.db",
  "dev": "0x…",
  "bytes": 13493000000,
  "sha256": "…",
  "schema": { "marker": "v2", "fingerprint": "51505efa…832", "tableCount": 0, "userVersion": 0 },
  "counts": { "sessions": 7848, "messages": 596008 },
  "health": { "quickCheck": "ok", "foreignKeyViolations": 0 },
  "holders": { "clear": true, "count": 0 },
  "held": false,
  "adoptedAt": "…",
  "boundaryRecord": null
}
```

- `boundaryRecord` is set when the adoption corresponds to a `snapshot.mjs` record, so the
  adoption and the freeze evidence point at the same digest.
- When `--allow-held` is used, `held` is `true` **and the record is excluded from count
  derivation**: a store with writers open has an unstable digest and must not be summed.

**Exit codes**

| Code | Meaning |
|---|---|
| `0` | adopted |
| `1` | refused — unhealthy, unrecognised marker, holders present without `--allow-held` |
| `2` | usage |

**Verification.** Re-running `adopt` with identical inputs must reproduce an identical
`sha256`, `marker`, `fingerprint` and counts. If it does not, the store was still being
written and the run must not proceed.

---

## 2. `sessions install` — SQLite-aware publish

**Purpose.** Publish the staged destination to `~/.local/share/opencode/`.

**Recommendation: a thin wrapper, not a reimplementation.**

The ten-phase restartable installer already exists as `install.mjs` and has been rehearsed
**0 failed (51 passes)**, including its interrupted / crashed / already-complete state
transitions. Writing
a second implementation in Rust would produce two authorities for one state file and throw
away that evidence. So:

```
chaosbox sessions install --dir <target> --source <staged.db> --state <state.json>
                          [--expect-sessions N] [--expect-messages N] [--expect-user-version N]
                          [--resume] [--stop-after <phase>] [--dry-run]
```

- resolves and validates arguments, then **execs** the pinned installer,
- forwards `--resume` / `--stop-after` / `--expect-*` unchanged,
- **forwards exit codes unchanged**, and
- `--dry-run` prints the phase plan and the state file it would use, executing nothing.

### 2.1 Exit-code contract (D gates depend on these)

| Code | Meaning | `install.mjs` behaviour |
|---|---|---|
| `0` | complete, or already complete | a `done` state is checked **before any other per-run check**: it reports and exits `0` whatever else was passed, and does **not** reinstall |
| `1` | a phase refused, or `--source` is missing | holders present, `quick_check`/FK failure, count or fingerprint mismatch, checkpoint `busy != 0` or a non-empty `-wal`, an unparseable credential baseline |
| `2` | usage, or a state it will not act on | missing args; a phase left mid-flight (needs `--resume`); a `--expect-*` that **contradicts** the state file; an unknown `--stop-after` |
| `3` | stopped early on request | `--stop-after` marks the state `interrupted`, which **auto-resumes** |

An *interrupted* state resumes on its own; a state left mid-flight by a failure or a crash
refuses until `--resume`. The distinction is deliberate: `--stop-after` is a choice the
operator made, a crash is not.

A resume that supplies a *different* `--expect-*` value is refused (`2`) rather than
silently adopted — the state file is the authority for that install. Omitting the flag is
**not** a contradiction: an absent `--expect-*` inherits the recorded value.

### 2.2 State-file schema (pinned — both sides depend on it)

```json
{
  "version": 1,
  "installId": "…",
  "createdAt": "…", "updatedAt": "…",
  "directory": "…", "target": "…/opencode.db", "source": "…/staged.db",
  "expect": { "sessions": 0, "messages": 0, "userVersion": 0 },
  "phase": "preserve-baseline|holders-clear|checkpoint|backup|stage|validate-stage|install|post-validate|preserve-verify|done|interrupted",
  "steps": { "<phase>": { "sha256": "…", "path": "…", "at": "…" } },
  "log": [ "…" ],
  "failure": { "phase": "…", "message": "…", "at": "…" },
  "resumeAt": "<next phase>"
}
```

**There is no `status` field.** `phase` carries position *and* status at once: a normal run
leaves a phase name, `--stop-after` leaves `"interrupted"` with `resumeAt` naming where to
resume, and completion leaves `"done"`. The per-phase artifacts live under `steps` — not
`phases` — and `failure` / `log` are appended rather than replacing either.

Three fields are load-bearing for rollback and must not be dropped or renamed:

- `steps.backup.sha256` — what the store digested to **before** the install;
- `steps.backup.counts` / `steps.backup.schema` — the pre-install shape. Rollback restores *to
  this record*, never to whatever the backup file happens to contain at rollback time;
- `steps["preserve-baseline"].record` — the credential baseline's path. That baseline is a
  **sidecar**, `<state>.preserve.json`, holding the `auth.json` / `account.json` digests; it
  is never inlined here, and a phase that finds it present-but-unparseable **fails** rather
  than overwriting the recorded "before" digests.

A state file whose `steps.backup` carries no digest or no shape is refused by `rollback.mjs`
outright: there would be nothing to restore *to*, and guessing is the one thing a rollback
must not do.

### 2.3 Preconditions (hard gates, not warnings)

1. nothing holds `--dir/opencode.db` — install refuses, it does **not** stop writers itself;
2. staged store: `quick_check` ok, FK violations `0`, marker `v2`, pinned fingerprint
   `51505efa…832`;
3. staged session/message counts equal `--expect-*` **and** equal `final-counts.json`;
4. credentials baseline recorded by `preserve-check.mjs --phase baseline`.

**Verification.** `steps.post-validate.sha256 == steps.stage.sha256` (the installed store *is*
the staged one) and `steps.backup.sha256 != steps.stage.sha256` (the install replaced
something rather than re-renaming itself), all ten phases present under `steps`, and
`preserve-check.mjs --phase verify` exits `0`. `validate-stage` refuses anything not
v2-marked with the pinned table-set fingerprint, which is what makes the degenerate
"staged == backup" case unreachable rather than merely unlikely.

---

## 3. `sessions rollback` — one operation, three things

Rollback must cover **store, package and config together**: restoring the old database while
the service still pointed at the new package leaves a mismatched pair. Splitting it into two
commands makes the intermediate state reachable.

```
chaosbox sessions rollback --state <state.json> --record <out.json>
                           --config-live <hm-live.json> --config-baseline <hm-baseline.json>
                           --config-restore "<cmd>"
                           [--dry-run]
```

All three `--config-*` arguments are **required** (missing any one is exit `2`, checked
before the freeze stops any writers): restoring the old database while the service still
pointed at the new package is the mismatched pair this one operation exists to prevent.
`--config-live` names the live-generation record, `--config-baseline` the pre-cutover
record taken by `hm-generation.mjs --snapshot` (the thing the restore is asserted
*against*), and `--config-restore` the single Home Manager command that moves package and
configuration back together. The post-restore generation is read back by a separate probe
(`hm-generation.mjs --get`), never trusted from the restore command's own report.

Enforced order:

1. **freeze writers** — run the freeze process, then re-check `/proc/*/fd`; a holder still
   attached is a **refusal**, not a warning;
2. **store** — verify `steps.backup.sha256` still matches the backup file, stage the restore
   copy beside the target and digest it *before* the live file is touched, rename the
   current store aside then the staged copy in, delete the foreign `-wal`/`-shm`, then
   `quick_check` + FK check + counts vs `steps.backup.counts` + fingerprint vs
   `steps.backup.schema`;
3. **config + package** — run `--config-restore`, probe the live generation, and require it
   to **equal the baseline generation** — "it changed" is not the claim; "it went back to
   the one the baseline names" is;
4. **verify** — no holders, `quick_check` ok, marker equals the pre-install marker.

**Refuses when:** `state.phase != "done"`; backup or its digest missing; `steps.backup` has
no recorded counts/schema (there is nothing to restore *to*, so it must not guess); any
`--config-*` argument missing; the post-restore generation is anything other than the
baseline's.

`--dry-run` walks every phase, records what it *would* do, and touches nothing.

The record is `{installId, createdAt, dryRun, phases: {freeze, store, config}, verify,
ok}` — `phases.config` carries `before` / `expected` / `probed` / `after`, so a reviewer
can see the generation the rollback started from, the one it owed, and the one it landed
on without re-running anything.

**Verification.** `phases.store.restoredSha256` equals the boundary-3 `snapshotSha256`,
`phases.config.after.generation` equals the baseline's, and `preserve-check.mjs --phase
verify` exits `0` — credentials survived the rollback too.

---

## 4. `journal-v3` supersession receipts — for Session A

Native import is **create-only (409)**, so it cannot express *update*. The 886 changed
sessions (884 count-changed v1 primary-source + 1 count-changed canary + 1 content-only
primary, whose message count never moved but whose content did) are therefore **not
re-imported**: `journal-v2/`
keeps the base receipt and `journal-v3/` gains a receipt that supersedes it. Session count is
unchanged — these sessions are already counted in the canonical 7 644.

`effective_receipts()` already tracks `depth` (`supersessions = Σ(depth − 1)`), so the
chaining machinery is present. The additions below are what the v3 records need on top of the
existing shape.

### 4.1 Record shape

Existing fields are carried through unchanged:
`{sessionID, source, inputDigest, recoveryDigest, destinationDigest, transformation, messages, drafts}`.
A `journal-v3/` receipt adds:

```json
{
  "kind": "supersession",
  "reason": "changed-after-boundary",
  "supersedes": "<sha256 of the receipt file it replaces>",
  "provenance": {
    "boundary": "v1",
    "boundaryRecordSha256": "…",
    "addedMessages": 5,
    "removedMessages": 0
  },
  "transfer": {
    "mechanism": "row-level-merge",
    "rehearsed": "rehearsal/<file>.json",
    "destinationSha256": "…"
  }
}
```

- `supersedes` names a **receipt-file digest**, not a session id — that is what makes a chain
  walkable and a cycle detectable.
- `transfer.mechanism` is mandatory and must be one the verifier accepts. `native-import`
  is **not** acceptable for a supersession: it would 409, and if it ever succeeded it would
  silently mean "the session did not exist", which contradicts the receipt's claim that it
  changed.
- Base receipts (`journal-v2/`) gain **no** new field. They have no `supersedes`.

### 4.2 Resolution rules (§3.1 restated as testable assertions)

| Rule | Expected result |
|---|---|
| Two receipts exist for one session | **not** an error — this is the normal case |
| Effective receipt | the chain head: no other receipt declares `supersedes == this.sha256` |
| `supersedes` names a digest in neither journal | **error** (dangling) |
| A walk from any receipt revisits a receipt | **error** (cycle) |
| Destination digest ≠ effective receipt's `destinationDigest`, chain head exists | status **`superseded-unverified`** |
| …and no chain head exists | status **`failed`** |
| `sessions verify` output | each session reported **exactly once** |

**Integrity** is set only when `quick_check` **and** `foreign_key_check` pass **and** the
recomputed `sessionHash(destination, id)` equals the **effective** receipt's
`destinationDigest` — never the base receipt's.

### 4.3 ⚠️ Open dependency A and D both need to settle

`final-counts.json` currently reads **7 913 sessions / 609 542 messages**, derived as
`7 644 canonical / 560 595` (from `reconciliation.json`, i.e. **boundary state**) `+ 204
variants / 35 413 + 65 delta-new / 13 534`, with the 886 changed sessions *not counted*.
The variant term is flagged `measured: false` until Session B hands over its artifact —
the 204 / 35 413 are asserted numbers, and `--status final` refuses to run on them (see
§6, row 8).
Both files are **preliminary** and are regenerated by `build-delta-inventory.mjs` →
`build-final-counts.mjs` → `build-identity-v3.mjs`.

The changed sessions account for **+4 277 / −27 messages that are not in that 609 153** —
they happened *after* the boundary the reconciliation was built from. So:

- **If** §4.2's transfer lands the newer content in the destination, the message total must
  be recomputed to include it (the `+4 277 / −27` moves), and the effective receipts'
  `destinationDigest` must describe the destination *after* the merge.
- **If** the destination keeps boundary content, then `609 542` stands, but a v3 receipt
  whose `destinationDigest` differs from what is actually stored would report
  `superseded-unverified` forever.

Either way **`609 542` is provisional until §4.2's transfer mechanism is chosen and rehearsed
on copies** (§3.1 requires rehearsal on copies before touching the real destination). The
mechanism is deliberately left open here — what is being signed off is the *receipt shape and
resolution rules*, which do not depend on which mechanism wins.

---

## 5. Tool pinning — must be done before execution

The runbook's tools (`freeze.mjs`, `verify-holders.mjs`, `snapshot.mjs`, `install.mjs`,
`rollback.mjs`, `preserve-check.mjs`, `hm-generation.mjs`) currently live under
`/data/scratch/tmp/opencode/cutover-c/` — **scratch storage**.

A runbook that points at scratch is fragile: the tools can vanish while the runbook, state
files and digests survive. Before any execution:

1. copy the tool set to a durable location (campaign root or the chaosbox tree),
2. record `path + sha256` for each into the campaign (a `tools.json`, pinned alongside
   `identity.json`),
3. verify each digest immediately before use,
4. reject a run whose recorded tool digest does not match.

This applies equally to `install.mjs`: if §2's wrapper execs it, the wrapper must exec the
**pinned** path, not one discovered on `PATH` or under scratch.

---

## 6. Sign-off

| # | Item | Owner | Status |
|---|---|---|---|
| 1 | §1 `adopt` args, record shape, exit codes | Session D | ⬜ pending |
| 2 | §2 `install` as wrapper + exit-code + state schema | Session D | ⬜ pending |
| 3 | §3 `rollback` ordering and refusals | Session D | ⬜ pending |
| 4 | §4.1 v3 receipt fields (`supersedes`, `kind`, `transfer`) | Session A | ⬜ pending |
| 5 | §4.2 resolution rules incl. `superseded-unverified` | Session A | ⬜ pending |
| 6 | §4.3 which content the destination holds → final message total | A **and** D | ⬜ pending |
| 7 | §5 tool pinning location | Session D | ⬜ pending |
| 8 | `build-final-counts.mjs --variants <file>`: `{"sessions": N, "messages": N}` or `{"variants": {…}}`, digest-pinned, required for `--status final` | Session B (producer) + D (gate) | ⬜ pending |

Nothing in §1–§5 is implemented until its row is signed. §4.3 additionally blocks the
**final** recomputation of `final-counts.json`, which is itself a §5.1 pre-install gate in the
runbook. Row 8's file is what turns the variant term from `measured: false` to measured;
until it exists every total carries the preliminary warning and no gate may consume it.
