# Render-only evaluation test for the Chaosbox deployment composition.
#
# Proves the harbor-db project (schema migration ordered after
# postgresql-setup), peer-authenticated connection wiring, the pgvector
# extension, and systemd ordering evaluate correctly without booting anything.
# No secret exists: authentication is peer over the local socket.
{
  lib,
  pkgs,
  harborDbModule,
  chaosboxModule,
  chaosboxPackage,
}:
let
  # Preserve the existing source path during read-only flake qualification.
  eval = import (pkgs.path + "/nixos/lib/eval-config.nix") {
    inherit (pkgs) system;
    modules = [
      harborDbModule
      chaosboxModule
      {
        system.stateVersion = "24.11";
        # Placeholder binary: only its store path is interpolated into unit
        # scripts, which this test never executes. Using a plain package
        # (no writeShellScriptBin) keeps this evaluation free of
        # import-from-derivation.
        services.harbor-db.package = pkgs.hello;
        services.chaosbox = {
          enable = true;
          package = chaosboxPackage;
        };
      }
    ];
  };
  project = eval.config.services.harbor-db.projects.chaosbox;
  schema = project.operations.schema;
  postgresql = eval.config.services.postgresql;
  migration = eval.config.systemd.services.harbor-db-chaosbox;
  checks = [
    {
      name = "postgres-backend-operation";
      assertion = schema.backend == "postgres";
      message = "schema operation must use the postgres backend";
    }
    {
      name = "ordering-gates-setup";
      assertion =
        lib.elem "postgresql-setup.service" migration.after
        && lib.elem "postgresql-setup.service" migration.requires;
      message = "the migration must order after and require postgresql-setup";
    }
    {
      name = "contract-commands";
      assertion =
        schema.runner.args == [
          "db"
          "migrate"
          "--json"
          "--repo"
          "demo"
        ]
        &&
          schema.runner.checkArgs == [
            "db"
            "check"
            "--json"
            "--repo"
            "demo"
          ];
      message = "schema runner must invoke the v2 contract commands";
    }
    {
      name = "backend-selection-env";
      assertion =
        project.environment.CHAOSBOX_DB_BACKEND == "postgres"
        && project.environment.CHAOSBOX_DATABASE_URL == "host=/run/postgresql dbname=chaosbox";
      message = "the project must route CLI commands at the local PostgreSQL socket";
    }
    {
      name = "peer-owner";
      assertion =
        project.user == "chaosbox"
        && lib.elem "chaosbox" postgresql.ensureDatabases
        && lib.any (u: u.name == "chaosbox" && u.ensureDBOwnership) postgresql.ensureUsers;
      message = "the migration must run as the peer-authenticated database owner";
    }
    {
      name = "pgvector";
      assertion =
        postgresql.enable
        && lib.hasInfix "CREATE EXTENSION IF NOT EXISTS vector" eval.config.systemd.services.postgresql-setup.script;
      message = "postgresql-setup must create the pgvector extension as superuser";
    }
    {
      name = "migration-unit-exists";
      assertion =
        (eval.config.systemd.services ? "harbor-db-chaosbox") && migration.serviceConfig.Type == "oneshot";
      message = "the generated migration unit must exist";
    }
  ];
  failed = builtins.filter (check: !check.assertion) checks;
in
if
  failed == [ ]
# A trivial derivation: the assertions above already threw at evaluation
# time on any mismatch, so reaching the builder means the composition
# renders. CI builds this as part of nix flake check.
then
  pkgs.runCommand "chaosbox-deployment-eval" { }
    ''echo "chaosbox deployment composition renders: ${toString (builtins.length checks)} checks" > $out''
else
  throw "chaosbox deployment eval failed: ${
    lib.concatStringsSep ", " (map (check: "${check.name}: ${check.message}") failed)
  }"
