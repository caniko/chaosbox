# Same-user intelligence synchronization

Chaosbox exchanges signed intelligence, bounded source capsules, lifecycle
assessments, and Jev reconciliation receipts between enrolled hosts. Each host
keeps its authoritative event ledger and current read snapshot in its own TypeDB
database. Configured peers are contacted automatically by `chaosbox sync serve`;
SSH aliases can point to LAN or VPN addresses. A peer can relay another enrolled
device's events, so the enrollment host does not need to stay online.

## Enrollment and connectivity

The user identity is an Ed25519 root public key. Device grants authorize an exact
private scope; Unix usernames and matching scope labels alone do not establish
membership. Keys and settings live in `$CHAOSBOX_SYNC_DIR`, or
`$XDG_STATE_HOME/chaosbox/sync` (default `~/.local/state/chaosbox/sync`). The directory
is private (0700), and signing files are private (0600).

On the first host:

```sh
chaosbox sync init --scope private:can
```

Save the returned public `user` and `device` identities. Keep `authority.pk8` on
the enrollment host; it signs grants, while `device.pk8` signs publications.

On another host, use the returned root public key:

```sh
chaosbox sync init --scope private:can --user ROOT_PUBLIC_KEY
```

On the enrollment host, authorize that host's returned device key:

```sh
chaosbox sync authorize NEW_DEVICE_PUBLIC_KEY \
  --output ~/.local/state/chaosbox/sync/new-device-grant.json
```

Transfer the public grant certificate to the new host, then install it:

```sh
chaosbox sync enroll /private/path/new-device-grant.json
```

Add reachable endpoints on each host that should initiate contact:

```sh
chaosbox sync peer --target can@other-host --device OTHER_DEVICE_PUBLIC_KEY
```

SSH uses batch authentication, verified host keys, and the fixed remote command
`chaosbox sync exchange`. The remote command must find the Chaosbox binary and its
operator-configured TypeDB environment. For a dedicated SSH key, a restricted
authorized-key command can invoke a local wrapper:

```sh
#!/bin/sh
export CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729
export CHAOSBOX_TYPEDB_USER=chaosbox
export CHAOSBOX_TYPEDB_DATABASE=chaosbox
export CHAOSBOX_TYPEDB_PASSWORD_FILE=/private/path/typedb-password
exec /absolute/path/chaosbox sync exchange
```

The corresponding authorized-key prefix is
`restrict,command="/absolute/path/chaosbox-peer"`. Configure SSH authentication and
known-host keys through the existing operator workflow. SSH encrypts transport;
root-signed grants and fresh session challenges independently authenticate the
peer and the original author of every relayed event.

## Publish and run

Configure the existing `CHAOSBOX_TYPEDB_ADDR`, `CHAOSBOX_TYPEDB_USER`,
`CHAOSBOX_TYPEDB_DATABASE`, and `CHAOSBOX_TYPEDB_PASSWORD_FILE` variables on each
host. Apply the packaged additive schema before starting sync:

```sh
chaosbox db migrate --json
```

Publish assessed intelligence with its original normalized source shards:

```sh
chaosbox sync publish /private/path/knowledge.json \
  --source-jsonl /private/path/transcript.ndjson
chaosbox sync once
```

The publisher reconstructs retained candidates against the original source
snapshot before signing. Only bounded source/evidence capsules travel. Legacy
receipts without replayable current-policy inputs require reassessment.

For the source-custodied compaction coordinator's admitted outbox:

```sh
chaosbox sync publish-memory --work /private/path/memory
chaosbox sync configure --archive /private/path/memory
chaosbox sync serve --interval-seconds 30
```

`serve` polls the configured archive, idempotently publishes changed outbox
content, and exchanges missing events with every configured peer. Unreachable
peers produce status reports and are retried on the next cycle. Run it under the
host's usual service supervisor for continuous operation. One process-held device
lease serializes local publication and inference; incoming SSH exchange remains
available while the worker runs. Ctrl-C releases the lease, including during a
peer connection or inference attempt.
Restart the worker after changing its peer or archive configuration.

## Jev reconciliation

Transport and retrieval do not dispatch inference. Enable the existing pinned
Jev client explicitly on the worker, or process a bounded queue once:

```sh
chaosbox sync serve --live-jev --budget reconcile-v1 \
  --max-requests 100 --max-input-tokens 1000000
chaosbox sync reconcile --live-jev --budget reconcile-v1 \
  --max-requests 100 --max-input-tokens 1000000 --max-jobs 20
```

`peer-reconciliation-v1` asks closed relationship and independent replacement
support questions over exact record versions and source capsules. Equivalent
items are deduplicated for retrieval; distinct applicability is retained;
contradictions retain both sides. Peer reconciliation replacement requires explicit user evidence
and reliable chronology within the same source/session. Host clock order and
delivery order do not pick winners.

Receipts bind record versions, prior competing decisions, complete model-visible
state, rubric, and the pinned model. Concurrent differing answers remain an
unresolved dispute until a receipt considers every maximal decision for those
inputs. Valid abstention is terminal for unchanged inputs. Changed evidence
invalidates semantic reuse; historical receipts remain inspectable.

Spending is conservatively reserved in the durable signed ledger **before** each
dispatch. A failed or interrupted request keeps its charge across restarts.
Deferred attempts do not block unrelated new jobs later in the bounded queue.
`--retry` explicitly authorizes another attempt but still obeys the cumulative
per-device budget. Changing the budget epoch authorizes a new allowance. A copied
device identity must not run independent writers: enroll a separate device for
each host. Protocol fixtures verify replay and convergence; they do not establish
the semantic accuracy of this initial rubric.

## Live consumers

```sh
chaosbox sync status
chaosbox sync context --repo canix 'source evidence' --limit 5
chaosbox sync evidence --repo canix intel:ACTUAL_RETURNED_ID
chaosbox mcp --intelligence-current ~/.local/state/chaosbox/sync
```

The live MCP reader pins one immutable snapshot per request and refreshes between
requests. If a later database read or snapshot validation fails, the process keeps
its last good snapshot and returns `degraded: true`. A cold reader reports an
error when no good snapshot is available. Item evidence includes bounded original
admission and semantic receipts, with explicit omission counts.

The OpenCode adapter accepts `syncDirectory` instead of `bundle`, plus the same
explicit `scope` and `repo` options. It pins the returned content identity, refreshes
per request, and caches last-good packets by exact repository/query/budget. Tools
cannot select another identity directory or dispatch Jev.

Snapshots describe local knowledge: `globally_current: false` reflects that an
offline peer may have unseen events. Retrieval remains bounded and lexical; an
empty packet is not evidence of exhaustive recall.

## Convergence and bounds

Events are immutable, content-addressed, signed, and causally parented. Duplicate
delivery is idempotent. Missing parents remain staged and retain the last good
snapshot. Receipt histories merge deterministically; a concurrent withholding
head remains effective until a later reassessment accounts for it. TypeDB stages
immutable views before a transactional predecessor-guarded pointer swing. Hash
identity collisions fail instead of overwriting history.

Limits are explicit: 4 MiB per event, 10,000 events and 64 MiB serialized event
history per scope, 64 MiB current snapshot, 200 events per page, and 12 MiB per
wire frame. Source imports have 32 MiB per-shard and 64 MiB aggregate limits.
Exceeding a limit refuses the operation; partition the admitted collection into
explicit scopes rather than silently truncating it. Transfer resumes by comparing
durable event inventories after reconnecting, without a central cursor or hub.

Verification uses `cargo test --workspace`, the sync tests, plugin Node tests,
Clippy, treefmt, and `scripts/test-typedb.sh`. The disposable TypeDB gate exercises
signed-event persistence and live-reader refresh in addition to isolation,
pagination, collision, restart, and publication-race checks.
The multi-peer transport test also injects a receiver outage after a committed
page, reconnects, and verifies that the durable prefix resumes without loss.
