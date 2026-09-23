# Chaosbox deployment on harbor-db (TypeDB-backed).
#
# Composable NixOS module: the consuming configuration imports this module
# together with harbor-db's module and the TypeDB service module:
#
#   imports = [
#     inputs.harbor-db.nixosModules.default
#     inputs.nixpkgs-typedb-typedb-module  # file: nixos/modules/services/databases/typedb.nix
#     inputs.chaosbox.nixosModules.chaosbox
#   ];
#
# Nothing activates until services.chaosbox.enable with the required secret
# files. Runtime services are attached later via services.chaosbox.runtimeUnits;
# until then the migration/check units are runnable on demand and the eval
# check (nix/deployment-eval.nix) proves the wiring renders.
#
# Server liveness comes from systemd ordering on typedb.service (there is no
# separate readiness probe unit): application readiness is the db check exit
# contract (0 ready, 2 pending), and schema migration is idempotent.
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
in
{
  options.services.chaosbox = {
    enable = mkEnableOption "Chaosbox TypeDB-backed deployment via harbor-db";

    package = mkOption {
      type = types.nullOr types.package;
      default = null;
      description = "Chaosbox package providing bin/chaosbox (db check|migrate). Set from the consuming flake's packages.";
    };

    repo = mkOption {
      type = types.str;
      default = "demo";
      description = "Repository name passed as --repo to db check|migrate.";
    };

    serverHost = mkOption {
      type = types.str;
      default = "127.0.0.1";
      description = "TypeDB driver host the CLI targets and the service listens on (loopback).";
    };

    serverPort = mkOption {
      type = types.port;
      default = 1729;
      description = "TypeDB driver port the CLI targets and the service listens on.";
    };

    database = mkOption {
      type = types.str;
      default = "chaosbox";
      description = "TypeDB database holding the Chaosbox schema and rows.";
    };

    username = mkOption {
      type = types.str;
      default = "admin";
      description = "Application username. Never the bootstrap admin in production; rotate the default credential first.";
    };

    stateDir = mkOption {
      type = types.path;
      default = "/var/lib/chaosbox";
      description = "State directory for Chaosbox runtime (writable by its future services).";
    };

    passwordFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "File holding the TypeDB application password, delivered as CHAOSBOX_TYPEDB_PASSWORD_FILE. Provision via age/sops or the typedb-bootstrap package output; never store a value here.";
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
        assertion = cfg.passwordFile != null;
        message = "services.chaosbox.passwordFile is required; it is delivered to migration/check commands, never embedded in the config";
      }
    ];

    services.typedb = {
      enable = true;
      listenHost = cfg.serverHost;
      listenPort = cfg.serverPort;
    };

    services.harbor-db.dataDirectories = [
      {
        path = cfg.stateDir;
        user = "root";
        group = "root";
        mode = "0700";
      }
    ];

    services.harbor-db.projects.chaosbox = {
      enable = true;
      description = "Chaosbox TypeDB schema migration";
      path = [ cfg.package ];
      environment.CHAOSBOX_DB_BACKEND = "typedb";
      environment.CHAOSBOX_TYPEDB_ADDR = "${cfg.serverHost}:${toString cfg.serverPort}";
      environment.CHAOSBOX_TYPEDB_USER = cfg.username;
      environment.CHAOSBOX_TYPEDB_DATABASE = cfg.database;
      operations.schema = {
        enable = true;
        backend = "typedb";
        credentials.typedb-password = cfg.passwordFile;
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
          credentialEnvironment.CHAOSBOX_TYPEDB_PASSWORD_FILE = "typedb-password";
        };
        after = [ "typedb.service" ];
        requires = [ "typedb.service" ];
      };
      runtimeUnits = cfg.runtimeUnits;
      serviceConfig.ReadWritePaths = [ cfg.stateDir ];
    };
  };
}
