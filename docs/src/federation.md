# Project-scoped intelligence federation

Federation builds a temporary read view across independently owned Chaosboxes.
For a project query, Chaosbox reads the local provider first and then consults
every configured peer for that project, including when local matches exist.
It does not publish the combined view, replicate events across users, invoke
Jev, or change either owner's records.

## Sharing contract

- Grants are directional: `can → dejana` and `dejana → can` are separate grants.
- The default mode is `selected`, with an empty record selection. No peer can
  read anything without a matching recipient/project grant.
- `all_admitted` explicitly shares all currently admitted or disputed records
  applicable to that project. Superseded and withheld records are excluded.
- Repository associations express applicability. A grant and explicit project
  mapping establish authorization; matching directory names do not.
- Each provider preserves its owner, private scope, original record ids,
  knowledge generation and source citations. The display id is
  `<provider>::<original-id>`.
- Queries and evidence reads reload the provider configuration. Removing a
  grant or record selection denies subsequent reads, including old handles.
- Peer results have no last-good cache. An unavailable or denied peer returns
  an explicit source status, while usable local results remain available.

Choose shared project keys from verified workspace/Git identity, such as
`git:github.com/caniko/chaosbox`. Canix's workspace scanner owns identity
discovery. Both owners explicitly map the key to their own existing intelligence
repository association. Forks need distinct keys unless the owners deliberately
agree on a shared applicability boundary. Local paths never appear in requests.

## Provider configuration

Place this operator-owned JSON in `~/.config/chaosbox/provider.json` for `can`:

```json
{
  "policy": {
    "version": 1,
    "identity": {
      "provider": "can-atlas",
      "owner": "can",
      "scope": "private:can"
    },
    "projects": {
      "git:github.com/caniko/chaosbox": { "repo": "chaosbox" }
    },
    "grants": [
      {
        "recipient": "dejana",
        "project": "git:github.com/caniko/chaosbox",
        "revision": 1,
        "mode": "all_admitted"
      }
    ]
  },
  "backend": { "kind": "typedb" }
}
```

For reciprocal sharing, `dejana` configures her own provider with
`provider: dejana-atlas`, `owner: dejana`, `scope: private:dejana`, and a grant
to `can`. Each provider can use a different local `repo` string for the same
shared key. Neither user shares the same-user sync root key.

A selected-record grant looks like this. Omitting `mode` means `selected`:

```json
{
  "recipient": "dejana",
  "project": "git:github.com/caniko/chaosbox",
  "revision": 2,
  "records": ["intel:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
}
```

`typedb` reads the existing private session-knowledge generations. Its
`CHAOSBOX_TYPEDB_*` connection and password-file settings belong to the provider
process. Provision a separate usable backend identity/credential for each owner;
an installed CLI or Unix account alone is not a provisioned provider. Federation
uses `TypeDbReader` and read transactions for both current and historical
knowledge. A missing database is unavailable and stays absent. Federation does
not provision database users or migrate schemas; publisher creation and migration
remain separate operator operations.

Reviewed bundle artifacts work without a database:

```json
{
  "kind": "bundle",
  "path": "/home/can/.local/state/chaosbox/current.json",
  "history": "/home/can/.local/state/chaosbox/history"
}
```

Use this object as `backend`. `history` is optional and contains immutable
`<bundle-digest>.json` artifacts (and any referenced receipt sidecars). Without
history, drill-down succeeds only while that exact artifact is still current.
TypeDB historical reads use the original scoped knowledge-build id, without
advancing the current pointer. Raw session archives are not served.

Update policy files atomically and keep them writable only by their owner or
the deployment authority. Increment a grant's positive `revision` when changing
it. Every reply also carries a content digest of the actual policy read.

## Query-only SSH endpoints

The recipient is bound by the SSH forced command, never by a field in the
request. Use a dedicated query key for each recipient/provider route; an
existing unrestricted login key does not establish a query-only endpoint.

For example, `dejana`'s authorized key entry for `can` runs a store-pinned
provider-local wrapper:

```text
restrict,command="/etc/profiles/per-user/dejana/bin/chaosbox-canix federation --config /home/dejana/.config/chaosbox/provider.json serve --caller can" ssh-ed25519 <can-query-public-key>
```

Deploy that entry through the fleet's SSH configuration. The wrapper must carry
`dejana`'s backend settings. Keep password files provider-local. Restrict
forwarding, PTYs and shell access for the query key. Configure the SSH alias
with its dedicated `IdentityFile` and `IdentitiesOnly=yes`.

The client sends the fixed original command `chaosbox federation serve`. The
server rejects another `SSH_ORIGINAL_COMMAND`. It accepts one version-1 JSON
request, serves local knowledge only and exits. Frames, record counts, output
characters and deadlines are bounded. SSH uses batch authentication, verified
host keys, no agent forwarding and no port forwarding.

## Client configuration and queries

For `can`, `~/.config/chaosbox/federation.json` can contain:

```json
{
  "version": 1,
  "local": "/home/can/.config/chaosbox/provider.json",
  "timeout_ms": 10000,
  "peers": [
    {
      "identity": {
        "provider": "dejana-atlas",
        "owner": "dejana",
        "scope": "private:dejana"
      },
      "destination": "dejana@latlas",
      "port": 1337,
      "projects": ["git:github.com/caniko/chaosbox"]
    }
  ]
}
```

Choose destination aliases and ports from Fleetix/Canix SSH routing. Peer
selection only chooses which endpoint to consult; the remote grant remains
authoritative. At most eight peers are permitted. Client routing/identity is
pinned when an MCP server starts; restart it after changing that configuration.
Provider grant changes take effect on the next read without a client restart.

An optional absolute `ssh_config` path selects an isolated SSH configuration
with `ssh -F`. Existing client files that omit it continue using their configured
SSH aliases. Managed Home Manager routes always set it, preventing ordinary login
identities, certificates or multiplexed connections from overriding a query route.

```sh
chaosbox federation --config "$HOME/.config/chaosbox/federation.json" context \
  --repo git:github.com/caniko/chaosbox --limit 5 --max-chars 12000 -- \
  'project constraints and source citations'

chaosbox federation --config "$HOME/.config/chaosbox/federation.json" evidence \
  --repo git:github.com/caniko/chaosbox --handle '<record.handle JSON from context>'
```

The context response includes each consulted source's snapshot, policy digest,
omissions and error (`denied`, `unavailable` or `invalid_response`). Successful
empty queries are distinguishable from failures. The total deadline reserves
time for peers if local retrieval stalls. Peer queries run concurrently after
the local phase; assembly interleaves local then peer records under one record
count and serialized record-array character ceiling. Source metadata is outside
that record budget and bounded by the provider-count limit.

Exact matching primary source quote/lineage is marked `same_origin`; both owners'
attributions remain visible. Different statements and known disputes are
retained. Repeated sources never become independent votes. Federation performs
no semantic reconciliation or cross-user winner selection.

## Evidence disclosure

`record.handle` contains provider, owner, original scope, project, snapshot and
original id. It is an address, not a capability. Drill-down checks current
authorization and current admission before resolving that exact historical
snapshot. A deleted, unselected or withheld record cannot be resurrected through
an old handle. Missing history returns `snapshot_unavailable`, never latest data.

Evidence responses disclose bounded supporting occurrences for that record and
safe receipt metadata. They omit raw assessment state, answers, neighboring
propositions and unrelated project associations. Relationship ids are qualified
and permission-filtered; hidden relationships have an explicit omission count.
Receipt projections identify the untouched original receipt and explicitly say
`state_omitted: true` and `is_projection: true`. They do not claim to be complete
or independently verifiable receipts.

## MCP, OpenCode and Canix

```sh
chaosbox mcp --intelligence-federation "$HOME/.config/chaosbox/federation.json"
```

`intelligence_context` keeps the explicit `repo`, `query`, `limit` and
`max_chars` arguments. In federation mode, `intelligence_evidence` requires
`repo` and the exact `handle` object, with an optional character ceiling.
Federation is mutually exclusive with the startup bundle and live sync-reader
MCP modes. Graph tools remain independently read-only.

The OpenCode V2 intelligence plugin accepts `federationConfig` instead of
`bundle` or `syncDirectory`. Configure the default project in that project's
OpenCode configuration:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "plugins": [
    {
      "package": "/path/to/chaosbox/plugins/chaosbox-intelligence",
      "options": {
        "federationConfig": "/home/can/.config/chaosbox/federation.json",
        "repo": "git:github.com/caniko/chaosbox",
        "chaosboxBin": "/etc/profiles/per-user/can/bin/chaosbox-canix"
      }
    }
  ]
}
```

Injected context retains source statuses, owner scopes and evidence handles.
The plugin reauthorizes every federation read and never replays an old peer
packet after denial or an outage. Challenge proposals remain human-reviewed
local artifacts; they do not mutate a peer's records.

Canix exposes `programs.chaosbox.mcp.intelligenceFederation` as an optional
client-config path. It checks the consumed package's `federationVersion` marker
and prevents mixing bundle and federation MCP modes. The package includes SSH
and exports `intelligencePlugin`. Enabling sharing still requires explicit
project grants, query keys and independently usable provider backends.

## Declarative Home Manager and Fleetix

The flake exports `homeModules.federation` (also
`homeManagerModules.federation`). The portable module uses
`programs.chaosbox.federation`; it works in standalone and integrated Home Manager.
Fleetix topology comes from `config.fleetix.topology`, integrated
`osConfig.fleetix.topology`, or an explicit `federation.fleetix.topology` value.
Import Fleetix's topology module when using its source/value selection:

```nix
{ config, inputs, ... }: {
  imports = [
    inputs.fleetix.homeModules.topology
    inputs.chaosbox.homeModules.federation
  ];

  # Standalone HM: use your generated Fleetix sidecar.
  # Integrated HM mirrors the NixOS Fleetix topology instead.
  fleetix.enable = true;
  fleetix.source = ./fleet-topology.nix;

  programs.chaosbox.federation = {
    enable = true;
    hostName = "atlas";
    identity.provider = "can-atlas";
    # owner defaults to home.username; scope defaults to private:<owner>.
    fleetix.enable = true;
    backend = {
      kind = "typedb";
      address = "127.0.0.1:1729";
      username = "chaosbox-can";
      database = "chaosbox";
      passwordFile = "/run/agenix/chaosbox-can-password";
    };
    projects."git:github.com/caniko/chaosbox" = {
      repo = "chaosbox";
      grants.dejana = { mode = "all_admitted"; revision = 1; };
    };
    peers.dejana-atlas = {
      identity.owner = "dejana";
      host = "atlas";
      route = "lan";
      identityFile = "/run/agenix/can-chaosbox-query";
      projects = [ "git:github.com/caniko/chaosbox" ];
    };
    # Declare only a dedicated query public key here, never a login key.
    authorizedKeys.dejana = [ (builtins.readFile ./dejana-query.pub) ];
  };
}
```

Public-key files must contain a single raw key line; a trailing newline is accepted.
Credential and private-key options accept runtime absolute string paths outside
the Nix store. The module never reads their contents during evaluation. The
example requires independently provisioned database users/password files and
dedicated query keys; it does not create them.

For each peer, `host` selects the Fleetix host. `route` selects `network.lanIp`,
`network.directLinkIp` (with mutual direct-link membership), or a named
`links.<route>.address`. The port defaults to `management.sshPort`, then
`access.ssh.port`. The host's enrolled `hostPubkey` and declared target account
are required. A deployment without Fleetix can supply `address`, `port` and
`hostKey` explicitly. Peer attribute names are the expected provider ids.

Managed files are placed beneath `$XDG_CONFIG_HOME/chaosbox`:

- `provider.json`: current identity, repository mappings and directional grants.
- `federation.json`: local provider, peer identities/routes and total deadline.
- `federation-ssh.conf` and `federation-known-hosts`: isolated query routes and
  enrolled trust; no agent identities, forwarding or connection multiplexing.

The read-only `clientFile`, `providerFile`, `runtimePackage` and
`authorizedKeyEntries` options expose the generated artifacts. Run CLI/MCP through
`runtimePackage`'s `chaosbox-federation` binary to carry the provider-local backend
settings. For example, with Fleetix's MCP Home Manager module imported:

```nix
{ config, lib, ... }: let
  fed = config.programs.chaosbox.federation;
in {
  fleetix.mcp.servers.chaosbox = {
    command = lib.getExe fed.runtimePackage;
    args = [ "mcp" "--intelligence-federation" fed.clientFile ];
  };
}
```

OpenCode's intelligence adapter uses the same `clientFile` as `federationConfig`
and the wrapper binary as `chaosboxBin`, retaining an explicit project `repo`.

For integrated Home Manager, import
`inputs.chaosbox.nixosModules.federation` and enable
`services.chaosbox.federation.enable`. This installs each enabled home's generated
recipient-bound entries into `users.users.<user>.openssh.authorizedKeys.keys`.
Standalone HM exposes the entries for the deployment's SSH authority to install.
The forced commands serve only the provider's local knowledge and pin the caller.
Grants default to `selected` with no ids; a query key alone grants no project reads.

### Shared Canix declaration

Canix's HM adapter imports the upstream module, supplies its Fleetix topology
facade and backend defaults, and connects managed `clientFile` to the existing
MCP registration. A single NixOS declaration can configure both homes and
generate project-specific reciprocal grants:

```nix
{ ... }: {
  canix.chaosbox.federation = {
    enable = true;
    projects."git:github.com/caniko/chaosbox" = {
      repos = { can = "chaosbox"; dejana = "chaosbox"; };
      mode = "all_admitted";
      revision = 1;
    };
    homes = {
      can = import ./can-chaosbox-provider.nix;
      dejana = import ./dejana-chaosbox-provider.nix;
    };
  };
}
```

Each home module declares its backend, peers and query keys using the portable
options shown above. Canix generates each owner's local mapping and a separate
grant to each other declared project participant, enables Fleetix-backed routes
and MCP, and installs the restricted endpoints. Shared project `mode` defaults
to `selected`; per-owner selected ids or directional overrides can be declared
in that home's `projects.<key>.grants.<recipient>` options. Project discovery
and verification remain Canix-owned; enrollment into code indexing does not
automatically enroll a project into sharing.

Consume a Chaosbox input exporting both federation modules before enabling this
declaration. Apply provider policies through Home Manager activation. Existing
project grants are rechecked on the next request; restart consumers after changing
client routes, identities or project mappings.

## Verification

Native gates (the adapter suite uses Node 24+):

```sh
cargo test -p chaosbox --test federation --locked
cargo test -p chaosbox --locked
node --test plugins/chaosbox-intelligence/test/*.test.mjs
python3 scripts/test-federation-gates.py -v
canix repo eval .#checks.x86_64-linux.federation-home.drvPath
bash scripts/test-typedb.sh --federation-only
```

The federation tests cover eligibility-before-ranking, selected defaults,
unauthorized projects/callers, grant revocation, exact historical drill-down,
receipt-context isolation, local-first plus peer queries, shared budgets,
provenance grouping, peer failure, deadlines and actual CLI/MCP wire contracts.
The federation-only command provisions an isolated TypeDB server and explicitly
runs `federation_typedb` with `CHAOSBOX_REQUIRE_TYPEDB=1`. It verifies that current
and historical reads cannot create missing databases, that foreign scopes are
invisible, that exact historical evidence resolves without advancing the current
pointer, and that revoked or withheld records remain denied. The native test is
ignored in ordinary unit runs, which do not provision a server. The local runner
also requires exactly one passed test with no failures or skips; an unmatched
test-name filter cannot report a successful gate.

The `typedb-integration` NixOS gate runs the same compiled regression against the
packaged TypeDB server and requires one passed test with no skips. Connection,
authentication, schema and assertion failures fail this mandatory hosted gate.

The `federation-home` check evaluates standalone and integrated Home Manager,
selected defaults, revocation, rejected routes/grants, and NixOS key installation,
then runs `scripts/test-federation-home.py` with OpenSSH and the packaged CLI.
The script verifies effective key isolation and local-first behavior during a
loopback peer outage. Canix's `tests/chaosbox-federation.nix` evaluates the real
Atlas adapters with a supplied candidate input and reciprocal project grants,
including each owner's separate credential-file wrapper and MCP registration.

The `federation-ssh` NixOS gate creates two disposable accounts with separate query
keys, enrolled host trust, different local repository mappings and reciprocal
`all_admitted` grants. It activates the real Home Manager module and installs its
generated recipient-bound forced commands through the NixOS adapter. The gate
checks successful local-plus-peer retrieval in both directions, original
attribution and evidence, exact historical reads, rejected caller/project/command
changes, grant revocation, withheld-record denial, one flushed response followed
by exit while stdin remains open, and local results during an SSH outage. It also
checks that reads preserve the authoritative bundle bytes and historical artifacts.
Its driver regressions cover zero/ignored tests, partial or unterminated frames,
extra frames and stalled exits. One deadline bounds the entire SSH response read
and process exit, including a partial frame that never reaches a newline.

`simit.toml` declares the package, unit, strict lint, scratch-plugin,
federation-home, intelligence-plugin, typedb-integration and federation-ssh
installables in the generated hosted Nix matrix. A completed green matrix
qualifies that immutable producer source; enrollment and consumer activation need
their own source/pin and runtime receipts.
