{
  pkgs,
  homeManager,
  federationModule ? import ./federation-home.nix,
}:
let
  inherit (pkgs) lib;
  project = "git:github.com/caniko/chaosbox";
  hostKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureHostKey";
  queryKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureQueryKey";
  package = pkgs.hello.overrideAttrs (_: {
    passthru.federationVersion = 1;
  });
  topology = {
    access.ssh.port = 1337;
    hosts.atlas = {
      network.lanIp = "192.0.2.10";
      links.wg-home.address = "198.51.100.10";
      management.sshPort = 2222;
      hostPubkey = hostKey;
      users.dejana.hasAccount = true;
    };
  };
  evaluate =
    extra:
    (homeManager.lib.homeManagerConfiguration {
      inherit pkgs;
      modules = [
        federationModule
        {
          home = {
            username = "can";
            homeDirectory = "/home/can";
            stateVersion = "26.05";
          };
          programs.chaosbox.federation = {
            enable = true;
            inherit package;
            identity.provider = "can-atlas";
            backend = {
              kind = "bundle";
              path = "/home/can/current.json";
            };
            fleetix = {
              enable = true;
              inherit topology;
            };
            projects.${project} = {
              repo = "chaosbox";
              grants.dejana = { };
            };
            peers.dejana-atlas = {
              identity.owner = "dejana";
              host = "atlas";
              identityFile = "/run/agenix/can-chaosbox-query";
              projects = [ project ];
            };
            authorizedKeys.dejana = [ queryKey ];
          };
        }
        extra
      ];
    }).config;
  cfg = evaluate { };
  fed = cfg.programs.chaosbox.federation;
  provider = builtins.fromJSON cfg.xdg.configFile."chaosbox/provider.json".text;
  client = builtins.fromJSON cfg.xdg.configFile."chaosbox/federation.json".text;
  peer = builtins.head client.peers;
  failures = c: map (a: a.message) (lib.filter (a: !a.assertion) c.assertions);
  invalid = evaluate {
    programs.chaosbox.federation.peers.dejana-atlas.projects = lib.mkForce [ "unmapped" ];
  };
  revoked = evaluate { programs.chaosbox.federation.projects.${project}.grants = lib.mkForce { }; };
  rejected = extra: !(builtins.tryEval (evaluate extra).home.activationPackage.drvPath).success;
  typedb = evaluate {
    programs.chaosbox.federation.backend = {
      kind = lib.mkForce "typedb";
      path = lib.mkForce null;
      username = "chaosbox-can";
      passwordFile = "/run/agenix/chaosbox-can-password";
    };
  };
  integrated =
    (import "${pkgs.path}/nixos/lib/eval-config.nix" {
      system = pkgs.stdenv.hostPlatform.system;
      modules = [
        homeManager.nixosModules.home-manager
        ./federation-nixos.nix
        {
          options.fleetix.topology = lib.mkOption {
            type = lib.types.attrs;
            default = topology;
          };
        }
        {
          networking.hostName = "atlas";
          services.chaosbox.federation.enable = true;
          users.users.can = {
            isNormalUser = true;
            home = "/home/can";
          };
          home-manager = {
            useGlobalPkgs = true;
            sharedModules = [ federationModule ];
            users.can = {
              home.stateVersion = "26.05";
              programs.chaosbox.federation = {
                enable = true;
                inherit package;
                backend = {
                  kind = "bundle";
                  path = "/home/can/current.json";
                };
                fleetix.enable = true;
                authorizedKeys.dejana = [ queryKey ];
              };
            };
          };
        }
      ];
    }).config;
in
assert failures cfg == [ ];
assert
  provider.policy.identity == {
    provider = "can-atlas";
    owner = "can";
    scope = "private:can";
  };
assert (builtins.head provider.policy.grants).mode == "selected";
assert (builtins.head provider.policy.grants).records == [ ];
assert
  provider.backend == {
    kind = "bundle";
    path = "/home/can/current.json";
  };
assert client.local == "/home/can/.config/chaosbox/provider.json";
assert peer.destination == "chaosbox-dejana-atlas";
assert peer.port == 2222;
assert peer.ssh_config == "/home/can/.config/chaosbox/federation-ssh.conf";
assert lib.hasInfix "192.0.2.10" fed.sshConfig;
assert lib.hasInfix "ControlPath none" fed.sshConfig;
assert lib.hasInfix "IdentityAgent none" fed.sshConfig;
assert lib.hasInfix "restrict,command=\"/nix/store/" (builtins.head fed.authorizedKeyEntries);
assert
  (evaluate { programs.chaosbox.federation.peers.dejana-atlas.route = "wg-home"; })
  .programs.chaosbox.federation.client.peers != [ ];
assert !(builtins.tryEval invalid.home.activationPackage.drvPath).success;
assert revoked.programs.chaosbox.federation.provider.policy.grants == [ ];
assert typedb.programs.chaosbox.federation.provider.backend == { kind = "typedb"; };
assert rejected {
  programs.chaosbox.federation.fleetix.topology = lib.mkForce (
    topology
    // {
      hosts.atlas = topology.hosts.atlas // {
        hostPubkey = null;
      };
    }
  );
};
assert rejected {
  programs.chaosbox.federation.fleetix.topology = lib.mkForce (
    topology
    // {
      hosts.atlas = topology.hosts.atlas // {
        users.dejana.hasAccount = false;
      };
    }
  );
};
assert rejected {
  programs.chaosbox.federation.projects.${project}.grants.dejana = {
    mode = "all_admitted";
    records = [ "intel:${lib.concatStrings (lib.replicate 64 "a")}" ];
  };
};
assert
  integrated.home-manager.users.can.programs.chaosbox.federation.identity.provider == "can-atlas";
assert rejected {
  programs.chaosbox.federation.authorizedKeys = lib.mkForce {
    can = [ "${queryKey} first comment" ];
    dejana = [ "${queryKey} second comment" ];
  };
};
assert integrated.home-manager.users.can.programs.chaosbox.federation.fleetix.topology == topology;
assert
  integrated.users.users.can.openssh.authorizedKeys.keys
  == integrated.home-manager.users.can.programs.chaosbox.federation.authorizedKeyEntries;
{
  passed = true;
  homeDrv = cfg.home.activationPackage.drvPath;
  integratedHomeDrv = integrated.home-manager.users.can.home.activationPackage.drvPath;
  inherit (fed) sshConfig;
  inherit (fed) provider client;
}
