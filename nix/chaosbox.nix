# Chaosbox deployment on harbor-db (Gel-backed).
#
# Composable NixOS module: the consuming configuration imports this module
# together with harbor-db's own modules:
#
#   imports = [
#     inputs.harbor-db.nixosModules.default
#     inputs.harbor-db.nixosModules.gel
#     inputs.chaosbox.nixosModules.chaosbox
#   ];
#
# Nothing activates until services.chaosbox.enable with the required secret
# files. Runtime services are attached later via services.chaosbox.runtimeUnits;
# until then the migration/check units are runnable on demand and the eval
# check (nix/deployment-eval.nix) proves the wiring renders.
{
  config,
  lib,
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
  cfg = config.services.chaosbox;
  gelUnit = config.services.harbor-db.gel.instances.chaosbox.systemdUnit;
in
{
  options.services.chaosbox = {
    enable = mkEnableOption "Chaosbox Gel-backed deployment via harbor-db";

    package = mkOption {
      type = types.nullOr types.package;
      default = null;
      description = "Chaosbox package providing bin/chaosbox (db check|migrate). Set from the consuming flake's packages.";
    };

    gelPackage = mkOption {
      type = types.package;
      default = pkgs.gel;
      defaultText = lib.literalExpression "pkgs.gel";
      description = "Gel CLI package (must stay major 7 with the pinned server).";
    };

    repo = mkOption {
      type = types.str;
      default = "demo";
      description = "Repository name passed as --repo to db check|migrate.";
    };

    gelPort = mkOption {
      type = types.port;
      default = 56561;
      description = "Host port for the Gel instance (loopback publish).";
    };

    gelDataDir = mkOption {
      type = types.path;
      default = "/var/lib/harbor-db-gel/chaosbox";
      description = "Persistent host directory for Gel instance data.";
    };

    stateDir = mkOption {
      type = types.path;
      default = "/var/lib/chaosbox";
      description = "State directory for Chaosbox runtime (writable by its future services).";
    };

    adminPasswordFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "File holding the Gel admin password (server bootstrap + readiness probe). Provision via age/sops; never store a value here.";
    };

    adminCredsFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "Gel credentials file for migration/check commands (CHAOSBOX_GEL_CREDENTIALS_FILE). Separate artifact from the password file.";
    };

    runtimeUnits = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Runtime units gated behind a successful migration (e.g. workers/readers). Empty until Chaosbox ships them as units; runtime credentials must then be a separate least-privilege role, never the migration credential.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.package != null;
        message = "services.chaosbox.package must be set when services.chaosbox.enable is true";
      }
      {
        assertion = cfg.adminPasswordFile != null;
        message = "services.chaosbox.adminPasswordFile is required; the server must start authenticated, never trust-auth";
      }
      {
        assertion = cfg.adminCredsFile != null;
        message = "services.chaosbox.adminCredsFile is required for migration/check commands";
      }
    ];

    services.harbor-db.gel.instances.chaosbox = {
      enable = true;
      port = cfg.gelPort;
      bindAddress = "127.0.0.1";
      dataDir = cfg.gelDataDir;
      passwordFile = cfg.adminPasswordFile;
    };

    services.harbor-db.dataDirectories = [
      {
        path = cfg.gelDataDir;
        user = "root";
        group = "root";
        mode = "0700";
      }
      {
        path = cfg.stateDir;
        user = "root";
        group = "root";
        mode = "0700";
      }
    ];

    services.harbor-db.projects.chaosbox = {
      enable = true;
      description = "Chaosbox Gel schema migration";
      # Generated units run with a minimal PATH; chaosbox shells out to
      # the gel CLI, so both must resolve here (plus an absolute fallback
      # via CHAOSBOX_GEL_BIN below).
      path = [
        cfg.package
        cfg.gelPackage
      ];
      environment.CHAOSBOX_GEL_BIN = "${cfg.gelPackage}/bin/gel";
      operations.ready = {
        enable = true;
        backend = "gel";
        credentials.admin-pw = cfg.adminPasswordFile;
        runner = {
          command = ''${config.services.harbor-db.gel.readyCheck}/bin/harbor-db-gel-ready --host 127.0.0.1 --port ${toString cfg.gelPort} --user admin --password-file "$READY_PW_FILE" --timeout 300s'';
          checkCommand = ''${config.services.harbor-db.gel.readyCheck}/bin/harbor-db-gel-ready --host 127.0.0.1 --port ${toString cfg.gelPort} --user admin --password-file "$READY_PW_FILE" --timeout 60s'';
          credentialEnvironment.READY_PW_FILE = "admin-pw";
        };
        after = [ gelUnit ];
        requires = [ gelUnit ];
      };
      operations.schema = {
        enable = true;
        backend = "gel";
        credentials.admin-creds = cfg.adminCredsFile;
        runner = {
          package = cfg.package;
          executable = "bin/chaosbox";
          args = [
            "db"
            "migrate"
            "--json"
            "--repo"
            cfg.repo
          ];
          checkArgs = [
            "db"
            "check"
            "--json"
            "--repo"
            cfg.repo
          ];
          credentialEnvironment.CHAOSBOX_GEL_CREDENTIALS_FILE = "admin-creds";
        };
        after = [ gelUnit ];
        requires = [ gelUnit ];
        dependsOn = [ "ready" ];
      };
      runtimeUnits = cfg.runtimeUnits;
      serviceConfig.ReadWritePaths = [ cfg.stateDir ];
    };
  };
}
