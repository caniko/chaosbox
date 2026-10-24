# Project sharing belongs to Chaosbox; Fleetix supplies topology and trust facts.
{
  config,
  lib,
  osConfig ? null,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;
  cfg = config.programs.chaosbox.federation;
  label = types.strMatching "[A-Za-z0-9._-]+";
  absolutePath = types.addCheck types.str (
    s: lib.hasPrefix "/" s && !lib.hasInfix "\n" s && !lib.hasInfix "\r" s
  );
  secretFile = types.addCheck absolutePath (s: !lib.hasPrefix "/nix/store/" s);
  projectName = types.addCheck types.nonEmptyStr (
    s: builtins.stringLength s <= 256 && builtins.match ".*[^[:space:]].*" s != null
  );
  publicKey = types.strMatching "(ssh-ed25519|ssh-rsa|ecdsa-sha2-[A-Za-z0-9-]+) [A-Za-z0-9+/=]+( [^\n\r]*)?(\n)?";
  normalizeKey = lib.removeSuffix "\n";
  option =
    type: default: description:
    mkOption { inherit type default description; };
  output =
    type: description:
    mkOption {
      inherit type description;
      readOnly = true;
    };
  homeUsername = config.home.username;
  ownerIdentityType =
    provider:
    types.submodule (
      { config, ... }: {
        options = {
          provider = option label provider "Stable provider identity (at most 128 characters).";
          owner = option label homeUsername "Owner authenticated by the recipient-bound SSH endpoint.";
          scope = mkOption {
            type = types.strMatching "private:.+";
            default = "private:${config.owner}";
            defaultText = lib.literalExpression ''"private:''${identity.owner}"'';
            description = "Original owner-local private scope; never renamed by federation.";
          };
        };
      }
    );
  grantType = types.submodule {
    options = {
      mode =
        option
          (types.enum [
            "selected"
            "all_admitted"
          ])
          "selected"
          "Selected records by default; all_admitted explicitly shares this project's admitted/disputed knowledge.";
      revision =
        option types.ints.positive 1
          "Positive permission revision; increment when changing a grant.";
      records = option (types.listOf (
        types.strMatching "intel:[0-9a-f]{64}"
      )) [ ] "Original record ids selected for this recipient and project.";
    };
  };
  peerType = types.submodule (
    {
      name,
      config,
      ...
    }:
    {
      options = {
        identity =
          option (ownerIdentityType name) { }
            "Expected provider, owner and scope; responses must match exactly.";
        host =
          option (types.nullOr label) null
            "Fleetix host identity. Required when address is not supplied.";
        route =
          option label "lan"
            "Fleetix address substrate: lan, direct-link, or a named host link such as wg-home.";
        address =
          option (types.nullOr types.nonEmptyStr) null
            "Explicit SSH address for deployment without Fleetix.";
        port =
          option (types.nullOr types.port) null
            "Explicit SSH port; otherwise host management port, then Fleetix access.ssh.port.";
        hostKey =
          option (types.nullOr publicKey) null
            "Enrolled host public key; defaults to the Fleetix host's hostPubkey.";
        sshUser = mkOption {
          type = label;
          default = config.identity.owner;
          defaultText = lib.literalExpression "peer.identity.owner";
          description = "Provider's Unix account, independent of the authenticated knowledge owner.";
        };
        identityFile = mkOption {
          type = secretFile;
          description = "Runtime path to this route's dedicated private query key; never a Nix path or key contents.";
        };
        projects =
          option (types.listOf projectName) [ ]
            "Exact shared project keys for which this provider is consulted.";
      };
    }
  );
  configRoot = "${config.xdg.configHome}/chaosbox";
  hosts = cfg.fleetix.topology.hosts or { };
  routes = lib.mapAttrs (
    name: peer:
    let
      host = if cfg.fleetix.enable && peer.host != null then hosts.${peer.host} or { } else { };
      address =
        if peer.address != null then
          peer.address
        else if peer.route == "lan" then
          host.network.lanIp or null
        else if peer.route == "direct-link" then
          host.network.directLinkIp or (host.links.direct-link.address or null)
        else
          host.links.${peer.route}.address or null;
      port =
        if peer.port != null then
          peer.port
        else if (host.management.sshPort or null) != null then
          host.management.sshPort
        else
          cfg.fleetix.topology.access.ssh.port or null;
    in
    {
      inherit address port;
      key = if peer.hostKey != null then peer.hostKey else host.hostPubkey or null;
      alias = "chaosbox-${name}";
      hostIdentity = if peer.host != null then peer.host else name;
      account = host.users.${peer.sshUser}.hasAccount or false;
    }
  ) cfg.peers;
  provider = {
    policy = {
      version = 1;
      inherit (cfg) identity;
      projects = lib.mapAttrs (_: project: { inherit (project) repo; }) cfg.projects;
      grants = lib.concatLists (
        lib.mapAttrsToList (
          project: settings:
          lib.mapAttrsToList (recipient: grant: grant // { inherit recipient project; }) settings.grants
        ) cfg.projects
      );
    };
    backend = {
      inherit (cfg.backend) kind;
    }
    // lib.optionalAttrs (cfg.backend.kind == "bundle") (
      {
        inherit (cfg.backend) path;
      }
      // lib.optionalAttrs (cfg.backend.history != null) { inherit (cfg.backend) history; }
    );
  };
  client = {
    version = 1;
    local = cfg.providerFile;
    timeout_ms = cfg.timeoutMs;
    peers = lib.mapAttrsToList (name: peer: {
      inherit (peer) identity projects;
      destination = routes.${name}.alias;
      port = routes.${name}.port;
      ssh_config = cfg.sshConfigFile;
    }) cfg.peers;
  };
  environment = lib.optionalAttrs (cfg.backend.kind == "typedb") {
    CHAOSBOX_DB_BACKEND = "typedb";
    CHAOSBOX_TYPEDB_ADDR = cfg.backend.address;
    CHAOSBOX_TYPEDB_USER = cfg.backend.username;
    CHAOSBOX_TYPEDB_DATABASE = cfg.backend.database;
    CHAOSBOX_TYPEDB_PASSWORD_FILE = cfg.backend.passwordFile;
  };
  runtimePackage = pkgs.writeShellApplication {
    name = "chaosbox-federation";
    runtimeInputs = [ pkgs.openssh ];
    text =
      lib.concatStringsSep "\n" (
        lib.mapAttrsToList (name: value: "export ${name}=${lib.escapeShellArg value}") environment
      )
      + "\nexec ${lib.getExe cfg.package} \"$@\"\n";
  };
  forcedCommand =
    recipient:
    pkgs.writeShellScript "chaosbox-query-${recipient}" ''
      exec ${lib.getExe runtimePackage} federation --config ${lib.escapeShellArg cfg.providerFile} serve --caller ${lib.escapeShellArg recipient}
    '';
  # OpenSSH config quoting is distinct from shell quoting.
  quote = s: ''"${lib.replaceStrings [ "\\" "\"" ] [ "\\\\" "\\\"" ] s}"'';
  sshConfig = lib.concatStringsSep "\n" (
    lib.mapAttrsToList (
      name: peer:
      let
        route = routes.${name};
      in
      ''
        Host ${route.alias}
          HostName ${quote (if route.address == null then "invalid" else route.address)}
          Port ${toString (if route.port == null then 0 else route.port)}
          User ${peer.sshUser}
          HostKeyAlias ${route.hostIdentity}
          IdentityFile ${quote peer.identityFile}
          IdentitiesOnly yes
          IdentityAgent none
          CertificateFile none
          AddKeysToAgent no
          BatchMode yes
          StrictHostKeyChecking yes
          UserKnownHostsFile ${quote cfg.knownHostsFile}
          GlobalKnownHostsFile /dev/null
          ControlMaster no
          ControlPath none
          ControlPersist no
          ForwardAgent no
          ClearAllForwardings yes
          RequestTTY no
          PasswordAuthentication no
          KbdInteractiveAuthentication no
      ''
    ) cfg.peers
  );
  validIdentity =
    identity:
    builtins.stringLength identity.provider <= 128
    && builtins.stringLength identity.owner <= 128
    && builtins.stringLength identity.scope <= 256;
in
{
  options.programs.chaosbox.federation = {
    enable = mkEnableOption "local-first, project-scoped Chaosbox intelligence federation";
    package = mkOption {
      type = types.package;
      description = "Chaosbox package with federationVersion >= 1.";
    };
    hostName = mkOption {
      type = label;
      default = if osConfig != null then osConfig.networking.hostName else "local";
      defaultText = lib.literalExpression ''osConfig.networking.hostName or "local"'';
      description = "Local Fleetix host identity, also used by the default provider id.";
    };
    identity =
      option (ownerIdentityType "${config.home.username}-${cfg.hostName}") { }
        "Pinned owner-local identity.";
    backend = {
      kind = option (types.enum [
        "bundle"
        "typedb"
      ]) "bundle" "Owner-local source adapter.";
      path =
        option (types.nullOr absolutePath) null
          "Current reviewed bundle file (required for bundle).";
      history =
        option (types.nullOr absolutePath) null
          "Optional digest-addressed immutable bundle history directory.";
      address = option types.nonEmptyStr "127.0.0.1:1729" "Provider-local TypeDB address.";
      username =
        option (types.nullOr types.nonEmptyStr) null
          "Dedicated TypeDB application username (required for typedb).";
      database =
        option types.nonEmptyStr "chaosbox"
          "TypeDB database holding private knowledge generations.";
      passwordFile =
        option (types.nullOr secretFile) null
          "Runtime credential-file reference outside the Nix store (required for typedb); contents are never read during evaluation.";
    };
    fleetix = {
      enable = mkEnableOption "Fleetix topology-backed SSH routes and enrolled host keys";
      topology = mkOption {
        type = types.attrs;
        default =
          config.fleetix.topology or (if osConfig != null then osConfig.fleetix.topology or { } else { });
        defaultText = lib.literalExpression "config.fleetix.topology or osConfig.fleetix.topology or {}";
        description = "Evaluated Fleetix topology, from its HM module or an explicit facade value.";
      };
    };
    projects = option (types.attrsOf (
      types.submodule {
        options = {
          repo = mkOption {
            type = projectName;
            description = "Existing provider-local repository association for this shared project key.";
          };
          grants =
            option (types.attrsOf grantType) { }
              "Directional project grants keyed by authenticated recipient.";
        };
      }
    )) { } "Explicit shared project keys and owner-local mappings. Empty means no projects.";
    peers =
      option (types.attrsOf peerType) { }
        "At most eight explicitly selected providers; no transitive queries.";
    authorizedKeys =
      option (types.attrsOf (types.listOf publicKey)) { }
        "Dedicated query public keys keyed by recipient. Install authorizedKeyEntries through the deployment's SSH authority.";
    timeoutMs =
      option (types.ints.between 100 30000) 10000
        "Total bounded local-plus-peer read deadline in milliseconds.";
    providerFile = output absolutePath "Managed live provider policy path; reloaded on every read.";
    clientFile = output absolutePath "Managed client JSON path for CLI, MCP and OpenCode federationConfig.";
    sshConfigFile = output absolutePath "Isolated SSH config used with ssh -F, avoiding ambient login identities and multiplexed sessions.";
    knownHostsFile = output absolutePath "Enrolled host-key file for isolated federation routes.";
    runtimePackage = output types.package "Store-pinned CLI wrapper carrying this provider's backend file references.";
    provider = output types.attrs "Rendered version-1 provider object.";
    client = output types.attrs "Rendered version-1 client object.";
    sshConfig = output types.lines "Rendered isolated SSH configuration.";
    authorizedKeyEntries = output (types.listOf types.str) "Restricted recipient-bound authorized-key entries for deployment.";
  };

  config = mkIf cfg.enable {
    programs.chaosbox.federation = {
      inherit
        provider
        client
        runtimePackage
        sshConfig
        ;
      providerFile = "${configRoot}/provider.json";
      clientFile = "${configRoot}/federation.json";
      sshConfigFile = "${configRoot}/federation-ssh.conf";
      knownHostsFile = "${configRoot}/federation-known-hosts";
      authorizedKeyEntries = lib.concatLists (
        lib.mapAttrsToList (
          recipient: keys:
          map (key: ''restrict,command="${forcedCommand recipient}" ${normalizeKey key}'') keys
        ) cfg.authorizedKeys
      );
    };
    assertions = [
      {
        assertion = (cfg.package.federationVersion or 0) >= 1;
        message = "Chaosbox federation requires a package with federationVersion >= 1.";
      }
      {
        assertion = validIdentity cfg.identity;
        message = "Chaosbox federation identity exceeds protocol length limits.";
      }
      {
        assertion = cfg.backend.kind != "bundle" || cfg.backend.path != null;
        message = "Chaosbox bundle federation requires backend.path.";
      }
      {
        assertion =
          cfg.backend.kind != "typedb"
          || (
            cfg.backend.username != null
            && cfg.backend.username != "admin"
            && cfg.backend.passwordFile != null
            && cfg.backend.path == null
            && cfg.backend.history == null
          );
        message = "Chaosbox TypeDB federation requires an application username and passwordFile, and no bundle paths.";
      }
      {
        assertion = builtins.length (lib.attrNames cfg.peers) <= 8;
        message = "Chaosbox federation permits at most eight peers.";
      }
      {
        assertion =
          builtins.length (lib.attrNames cfg.projects) <= 1024
          && builtins.length provider.policy.grants <= 4096;
        message = "Chaosbox federation project/grant count exceeds protocol limits.";
      }
      {
        assertion =
          builtins.stringLength (builtins.toJSON provider) <= 1048576
          && builtins.stringLength (builtins.toJSON client) <= 1048576;
        message = "Chaosbox federation provider/client JSON exceeds the 1 MiB protocol limit.";
      }
      {
        assertion = lib.all projectName.check (lib.attrNames cfg.projects);
        message = "Chaosbox federation project keys must be nonempty and at most 256 characters.";
      }
      {
        assertion = lib.all (
          name: builtins.match "[A-Za-z0-9._-]+" name != null && builtins.stringLength name <= 128
        ) (lib.attrNames cfg.authorizedKeys);
        message = "Chaosbox federation query-key recipients must be valid owner labels.";
      }
      {
        assertion =
          let
            keys = map (key: lib.concatStringsSep " " (lib.take 2 (lib.splitString " " (normalizeKey key)))) (
              lib.concatLists (lib.attrValues cfg.authorizedKeys)
            );
          in
          builtins.length keys == builtins.length (lib.unique keys);
        message = "Chaosbox federation query public keys must be unique so each key binds exactly one recipient.";
      }
      {
        assertion = lib.all (
          grant:
          builtins.match "[A-Za-z0-9._-]+" grant.recipient != null
          && builtins.stringLength grant.recipient <= 128
          && builtins.length grant.records <= 100000
          && (grant.mode != "all_admitted" || grant.records == [ ])
        ) provider.policy.grants;
        message = "Chaosbox federation grants require valid recipients and bounded selections; all_admitted cannot also select records.";
      }
    ]
    ++ lib.mapAttrsToList (
      name: peer:
      let
        route = routes.${name};
      in
      {
        assertion =
          validIdentity peer.identity
          && peer.identity.provider == name
          && peer.identity.provider != cfg.identity.provider
          && route.address != null
          && builtins.match "[][A-Za-z0-9._:-]+" route.address != null
          && route.port != null
          && types.port.check route.port
          && route.port > 0
          && route.key != null
          && publicKey.check route.key
          && (!cfg.fleetix.enable || peer.host == null || (builtins.hasAttr peer.host hosts && route.account))
          && (
            peer.route != "direct-link"
            || peer.address != null
            || (
              builtins.elem peer.host (hosts.${cfg.hostName}.network.directLinkPeers or [ ])
              && builtins.elem cfg.hostName (hosts.${peer.host}.network.directLinkPeers or [ ])
            )
          )
          && peer.projects != [ ]
          && builtins.length peer.projects <= 1024
          && lib.all (p: builtins.hasAttr p cfg.projects) peer.projects;
        message = "Chaosbox federation peer ${name} requires a distinct matching identity, enrolled host key, reachable address/port, target account and mapped projects.";
      }
    ) cfg.peers;
    home.packages = [ runtimePackage ];
    xdg.configFile = {
      "chaosbox/provider.json".text = builtins.toJSON provider;
      "chaosbox/federation.json".text = builtins.toJSON client;
      "chaosbox/federation-ssh.conf".text = sshConfig;
      "chaosbox/federation-known-hosts".text =
        lib.concatStringsSep "\n" (
          lib.unique (
            lib.mapAttrsToList (
              _: route:
              "${route.hostIdentity},[${route.hostIdentity}]:${toString route.port} ${normalizeKey (toString route.key)}"
            ) routes
          )
        )
        + "\n";
    };
  };
}
