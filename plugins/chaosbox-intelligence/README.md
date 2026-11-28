# chaosbox-intelligence (OpenCode V2 plugin, first slice)

This read-only OpenCode adapter injects selected evidence from one validated,
operator-pinned chaosbox intelligence bundle. It supports evidence drill-down
and structured challenges for human review.

For synchronized intelligence, set `syncDirectory` to the operator-owned Chaosbox
sync directory instead of `bundle` and keep `scope` and `repo` explicit.
On each request, `chaosbox sync context|evidence` refreshes to the newest local
TypeDB snapshot. The adapter pins the returned content digest, not filesystem
metadata. On refresh failure, the adapter retains the last-good packet for that
exact query and marks it degraded. If none exists, the session continues. See
[peer synchronization](../../docs/PEER_SYNC.md) for enrollment and the background
worker. The read-only CLI process resolves TypeDB credentials; the adapter
does not open device or model keys.

## What it does

- **Selective injection:** a `context` hook derives bounded lexical terms from
  the latest user message and shells out to `chaosbox intelligence context`
   (read-only, no credentials). It appends non-empty packets to the model call,
   marked as historical evidence. If retrieval fails, the session continues
   silently without injection.
- **Evidence drill-down:** `chaosbox_evidence` wraps `chaosbox intelligence
  evidence` (source occurrences, contradictions, supersession, typed receipts).
- **Challenges:** `chaosbox_challenge` validates and stores structured proposals
   (`incorrect` | `inapplicable` | `needs-scope-exception`). Humans resolve them
   (uphold, amend, supersede, scoped exception); challenges never change the bundle.
- **Shadow-mode capture:** when `shadowDir` is set, the adapter appends
   decision-moment excerpts to `<shadowDir>/sessions/<session>.jsonl` for later
   explicit `chaosbox intelligence extract|assess` runs. It skips excerpts
   quoting injected intelligence so plugin output never corroborates itself.
- **Status:** `chaosbox_status` reports the pinned bundle version, degraded
  state, and capture counts for pilot measurement.

## What it does not do

- No Jev calls, no credentials in the plugin, no writes to knowledge bundles,
  no TypeDB publication, no session-archive access.
- No injection into title/compaction/transient-generate requests.
- No automatic admission: shadow candidates are proposals only.

## Configure

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugins": [
    {
      "package": "./plugins/chaosbox-intelligence",
      "options": {
        "bundle": "/private/path/knowledge-1.json",
        "scope": "private:can",
        "repo": "canix",
        "limit": 5,
        "maxChars": 12000,
        "inject": true,
        "shadowDir": "/private/path/chaosbox-shadow"
      }
    }
  ]
}
```

See `opencode.example.jsonc`. Expose bundles only to callers authorized for
their scope. Scope labels are a visibility boundary, not an ACL.

## Operator workflow

1. Curate a bundle with the existing CLI (`extract` → `assess --live-jev`).
2. Pin it here; use `/chaosbox-context <terms>` for on-demand lookup.
3. The agent disputes records with `chaosbox_challenge`. Explicitly resolve
   its proposal under `<shadowDir>/challenges/`.
4. Promote shadow excerpts with the normal CLI. Before widening automatic
   injection, measure unsupported admissions, stale injections, challenge
   outcomes, latency, and spend.

## Verify

Run the adapter runtime tests with Node 24+. Nix CI runs the suite through
the `intelligence-plugin` check.

```sh
node --test test/
```

## Project federation

Set `federationConfig` to an operator-owned federation client JSON and `repo`
to the shared project key. Select exactly one of `bundle`, `syncDirectory`,
or `federationConfig`. Federation needs no plugin-wide `scope`: each record
retains its provider/owner/scope and exact snapshot in its evidence handle.

Context reads use `chaosbox federation --config ... context`. Evidence uses
the exact `handle` object context returns. Injected text includes source statuses
and shared-origin groups. Federation has no last-good packet cache;
denied or offline peers cannot replay previously shared content.

See [the federation contract](../../docs/src/federation.md) for reciprocal
all-project grants between `can` and `dejana`, provider-only SSH endpoints,
backend provisioning, CLI/MCP use, and evidence disclosure boundaries.
