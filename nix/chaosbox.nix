# Chaosbox deployment on harbor-db (PostgreSQL + pgvector).
#
# Composable NixOS module: the consuming configuration imports this module
# together with harbor-db's module:
#
#   imports = [
#     inputs.harbor.nixosModules.database-default
#     inputs.chaosbox.nixosModules.chaosbox
#   ];
#
# Nothing activates until services.chaosbox.enable. The database, its owner
# role and the pgvector extension are ensured by postgresql-setup; schema
# migration is the idempotent `chaosbox db migrate`, and application
# readiness is the `db check` exit contract (0 ready, 2 pending).
#
# Authentication is peer over the local socket: the OS user, PostgreSQL role
# and database share one name, so no password exists anywhere.
{
  config,
  lib,
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
  databaseUrl = "host=${cfg.socketDir} dbname=${cfg.database}";
in
{
  options.services.chaosbox = {
    enable = mkEnableOption "Chaosbox PostgreSQL-backed deployment via harbor-db";

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

    database = mkOption {
      type = types.str;
      default = "chaosbox";
      description = "Database name, owning PostgreSQL role and system user (peer authentication).";
    };

    socketDir = mkOption {
      type = types.str;
      default = "/run/postgresql";
      description = "PostgreSQL unix socket directory the CLI connects through.";
    };

    stateDir = mkOption {
      type = types.path;
      default = "/var/lib/chaosbox";
      description = "State directory for Chaosbox runtime (writable by its future services).";
    };

    runtimeUnits = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Runtime units gated behind a successful migration (e.g. workers/readers). Read-only consumers should use a separate role granted SELECT.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.package != null;
        message = "services.chaosbox.package must be set when services.chaosbox.enable is true";
      }
    ];

    users.users.${cfg.database} = {
      isSystemUser = true;
      group = cfg.database;
    };
    users.groups.${cfg.database} = { };

    services.postgresql = {
      enable = true;
      extensions = ps: [ ps.pgvector ];
      ensureDatabases = [ cfg.database ];
      ensureUsers = [
        {
          name = cfg.database;
          ensureDBOwnership = true;
        }
      ];
    };
    # pgvector is not a trusted extension: create it as the superuser so the
    # unprivileged owner's migration only verifies it.
    systemd.services.postgresql-setup.script = lib.mkAfter ''
      psql -v ON_ERROR_STOP=1 -d ${lib.escapeShellArg cfg.database} -c 'CREATE EXTENSION IF NOT EXISTS vector'
    '';

    services.harbor-db.dataDirectories = [
      {
        path = cfg.stateDir;
        user = cfg.database;
        group = cfg.database;
        mode = "0700";
      }
    ];

    services.harbor-db.projects.chaosbox = {
      enable = true;
      description = "Chaosbox PostgreSQL schema migration";
      path = [ cfg.package ];
      user = cfg.database;
      group = cfg.database;
      environment.CHAOSBOX_DB_BACKEND = "postgres";
      environment.CHAOSBOX_DATABASE_URL = databaseUrl;
      postgres = {
        enable = true;
        inherit databaseUrl;
        setupUnits = [ "postgresql-setup.service" ];
      };
      operations.schema = {
        enable = true;
        backend = "postgres";
        runner = {
          inherit (cfg) package;
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
        };
      };
      inherit (cfg) runtimeUnits;
      serviceConfig.ReadWritePaths = [ cfg.stateDir ];
    };
  };
}
