# Project-scoped intelligence federation

Federation builds a temporary read view across independently owned Chaosboxes.
Chaosbox reads the local provider first, then every peer configured for the
queried project, even with local matches. Federation never
publishes the view, replicates events across users, calls Jev or changes either
owner's records.

## Sharing contract

- Grants are directional: `can → dejana` and `dejana → can` are separate grants.
- The default mode is `selected`, with an empty record selection. No peer can
  read anything without a matching recipient/project grant.
- `all_admitted` explicitly shares all currently admitted or disputed records
  applicable to that project. Superseded and withheld records are excluded.
- Repository associations express applicability. A grant and explicit project
  mapping establish authorization; matching directory names do not.
- Each provider preserves its owner, private scope, original record ids,
  knowledge generation and source citations. Display ids use
  `<provider>::<original-id>`.
- Queries and evidence reads reload provider configuration. Removing a grant
  or record selection denies later reads, even with old handles.
- Peer results have no last-good cache. An unavailable or denied peer returns
  an explicit source status, while usable local results remain available.

Choose shared project keys from verified workspace/Git identity, such as
`git:github.com/caniko/chaosbox`. Canix's workspace scanner owns identity
discovery. Both owners explicitly map the key to their existing intelligence
repository association. Forks need distinct keys unless owners deliberately
agree on a shared applicability boundary. Requests never include local paths.

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
  "backend": { "kind": "postgres" }
}
```

For reciprocal sharing, `dejana` configures her own provider with
`provider: dejana-atlas`, `owner: dejana`, `scope: private:dejana`, and a grant
to `can`. Each provider can use a different local `repo` string for the same
shared key. Neither user shares the same-user sync root key.

A selected-record grant can omit `mode`; it defaults to `selected`:

```json
{
  "recipient": "dejana",
  "project": "git:github.com/caniko/chaosbox",
  "revision": 2,
  "records": ["intel:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
}
```

`postgres` reads private session-knowledge generations. The provider process owns
`CHAOSBOX_DATABASE_URL` (unix socket or loopback; peer authentication as the owner).
An installed CLI or Unix account isn't enough: provision a separate database and
role per owner. Federation reads current and historical knowledge in read-only
`PgReader` sessions; missing databases stay missing and are reported unavailable.
Operators create publishers and migrate separately. Federation doesn't provision
roles or migrate schemas, and ranks lexically without embedding queries.

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
PostgreSQL historical reads use the original scoped knowledge-build id, without
advancing the current pointer. Raw session archives are not served.

Update policy files atomically, and keep them writable only by their owner or
the deployment authority. Increment the positive `revision` on each grant
change. Every reply carries a content digest of the policy it read.

## Query-only SSH endpoints

The SSH forced command binds the recipient, never a request field. Use a
dedicated query key per recipient/provider route. An unrestricted login key
doesn't establish a query-only endpoint.

For example, `dejana`'s authorized key entry for `can` runs a store-pinned
provider-local wrapper:

```text
restrict,command="/etc/profiles/per-user/dejana/bin/chaosbox-canix federation --config /home/dejana/.config/chaosbox/provider.json serve --caller can" ssh-ed25519 <can-query-public-key>
```

Deploy that entry through the fleet's SSH configuration. The wrapper must carry
`dejana`'s backend settings. Keep the database connection provider-local. Restrict
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

Choose aliases and ports from Fleetix/Canix SSH routing. Peer selection routes
requests; authorization still requires the remote grant and explicit project
mapping. Configure at most eight peers. MCP pins client routing/identity at
startup; restart it after changing those settings. Grant changes take effect on
the next read without a client restart.

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

Context reports each consulted source's snapshot, policy digest, omissions and
error (`denied`, `unavailable` or `invalid_response`). An empty success is distinct
from failure. The total deadline reserves peer time if local retrieval stalls.
Peers run concurrently after the local phase. The response interleaves local
then peer records under shared limits on record count and serialized record-array
characters. Source metadata doesn't count toward that budget; the provider-count
limit bounds it.

Federation marks exact primary source quote/lineage matches `same_origin` and
retains both owners' attributions. Different statements and known disputes stay
visible. Repeated sources aren't independent votes. Federation doesn't reconcile
meaning or choose a winner across users.

## Evidence disclosure

`record.handle` contains provider, owner, original scope, project, snapshot and
original id. It's an address, not a capability. Drill-down checks current
authorization and current admission before resolving the exact historical
snapshot. Old handles can't recover deleted, unselected or withheld records.
Missing history returns `snapshot_unavailable`, never the latest data.

Evidence responses disclose bounded supporting occurrences for the record and
safe receipt metadata. They omit raw assessment state, answers, neighboring
propositions and unrelated project associations. They qualify relationship ids,
filter them by permission and count hidden relationships explicitly. Receipt
projections identify the untouched original and report `state_omitted: true`
and `is_projection: true`; they don't claim completeness or independent
verifiability.

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
The plugin reauthorizes every federation read and never replays old peer packets
after denial or outage. Challenge proposals stay local and human-reviewed; they
don't change peer records.

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
      kind = "postgres";
      databaseUrl = "host=/run/postgresql dbname=chaosbox_can";
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

Public-key files must contain one raw key line, with an optional trailing newline.
Set credential and private-key options to runtime absolute string paths outside
the Nix store. The module never reads those files during evaluation. Before
using this example, provision each owner's database, role and dedicated query
keys separately.

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

### Shared project declaration

With integrated Home Manager, one NixOS declaration renders each owner's local
mapping and a separate grant to every other declared participant:

```nix
{ ... }: {
  services.chaosbox.federation = {
    enable = true;
    projects."git:github.com/caniko/chaosbox" = {
      repos = { can = "chaosbox"; dejana = "chaosbox"; };
      mode = "all_admitted";
      revision = 1;
    };
  };
}
```

Each owner's home still enables `programs.chaosbox.federation` and declares its
backend, peers and query keys using the portable options shown above. Shared
project `mode` defaults to `selected`; per-owner selected ids or directional
overrides can be declared in that home's `projects.<key>.grants.<recipient>`
options. Canix adds its Fleetix topology facade, backend defaults and MCP
registration on top. Enrollment into code indexing does not automatically enroll
a project into sharing.

Import both federation modules before enabling this declaration. Apply provider
policies through Home Manager activation. The next request rechecks existing
project grants. Restart consumers after changing client routes, identities or
project mappings.

## Verification

Native gates (the adapter suite uses Node 24+):

```sh
cargo test -p chaosbox --test federation --locked
cargo test -p chaosbox --locked
node --test plugins/chaosbox-intelligence/test/*.test.mjs
python3 scripts/test-federation-gates.py -v
canix repo eval .#checks.x86_64-linux.federation-home.drvPath
cargo test -p chaosbox --test federation_pg --locked -- --ignored live_pg_
```

The federation tests cover eligibility-before-ranking, selected defaults,
unauthorized projects/callers, grant revocation, exact historical drill-down,
receipt-context isolation, local-first plus peer queries, shared budgets,
provenance grouping, peer failure, deadlines and actual CLI/MCP wire contracts.
The `live_pg_` federation test starts a disposable PostgreSQL cluster. It checks
that current and historical reads can't create missing databases, that foreign
scopes remain invisible, that revoked or withheld records remain denied, and
that exact historical evidence resolves without advancing the current pointer.
Ordinary unit runs skip it.
The hermetic `postgres-live` Nix check runs every `live_pg_` test in the build
sandbox and fails on connection, schema or assertion errors.

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
Its driver regressions cover partial or unterminated frames,
extra frames and stalled exits. One deadline bounds the entire SSH response read
and process exit, including a partial frame that never reaches a newline.

`simit.toml` declares the package, unit, strict lint, scratch-plugin,
federation-home, intelligence-plugin, postgres-live, postgres-integration and
federation-ssh installables in the generated hosted Nix matrix. A completed
green matrix qualifies that immutable producer source. Enrollment and consumer
activation need their own source/pin and runtime receipts.
