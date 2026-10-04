# chaosbox-intelligence (OpenCode V2 plugin, first slice)

Read-only, selectively injecting OpenCode adapter over one operator-pinned,
validated chaosbox intelligence bundle, with evidence drill-down and
structured human-in-the-loop challenges.

For synchronized intelligence, set `syncDirectory` to the operator-owned Chaosbox
sync directory instead of `bundle`, retaining the explicit `scope` and `repo`.
Retrieval uses `chaosbox sync context|evidence` and refreshes the newest local
TypeDB snapshot per request. The adapter pins the returned content digest rather
than filesystem metadata. Refresh failures retain an exact-query last-good packet
marked degraded; the session continues if no packet is available. See
[peer synchronization](../../docs/PEER_SYNC.md) for enrollment and the background
worker. TypeDB credentials are resolved by the read-only CLI process; the adapter
does not open device or model keys.

## What it does

- **Selective injection:** a `context` hook derives bounded lexical terms from
  the latest user message and shells out to `chaosbox intelligence context`
  (read-only, no credentials). Non-empty packets are appended to the outgoing
  model call as explicitly marked historical evidence. Retrieval failures
  degrade silently; the session continues without injection.
- **Evidence drill-down:** `chaosbox_evidence` wraps `chaosbox intelligence
  evidence` (source occurrences, contradictions, supersession, typed receipts).
- **Challenges:** `chaosbox_challenge` validates a structured proposal
  (`incorrect` | `inapplicable` | `needs-scope-exception`) and stores it for
  human resolution (uphold, amend, supersede, scoped exception). A challenge
  never mutates the bundle.
- **Shadow-mode capture:** when `shadowDir` is set, decision-moment excerpts
  are appended to `<shadowDir>/sessions/<session>.jsonl` for later explicit
  `chaosbox intelligence extract|assess` runs. Excerpts quoting injected
  intelligence are skipped so plugin output never corroborates itself.
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

See `opencode.example.jsonc`. Only expose a bundle to callers authorized to
read its scope. Scope labels are a visibility boundary, not an ACL.

## Operator workflow

1. Curate a bundle with the existing CLI (`extract` → `assess --live-jev`).
2. Pin it here; use `/chaosbox-context <terms>` for on-demand lookup.
3. When the agent disputes a record, it calls `chaosbox_challenge`; resolve
   the proposal file under `<shadowDir>/challenges/` explicitly.
4. Promote shadow excerpts with the normal CLI; measure unsupported
   admissions, stale injections, challenge outcomes, latency, and spend before
   widening automatic injection.

## Verify

Use Node 24+ for the adapter runtime tests. Nix CI also runs this suite through
the `intelligence-plugin` check.

```sh
node --test test/
```

## Project federation

Set `federationConfig` to an operator-owned federation client JSON and `repo`
to the shared project key. Select exactly one of `bundle`, `syncDirectory`,
or `federationConfig`. Federation needs no single plugin `scope`: every record
retains its provider/owner/scope and exact snapshot in its evidence handle.

Context reads use `chaosbox federation --config ... context`. Evidence uses
the exact `handle` object returned by context. Source statuses and shared-origin
groups are included in injected text. Federation has no last-good packet cache;
denied or offline peers cannot replay previously shared content.

See [the federation contract](../../docs/src/federation.md) for reciprocal
all-project grants between `can` and `dejana`, provider-only SSH endpoints,
backend provisioning, CLI/MCP use, and evidence disclosure boundaries.
