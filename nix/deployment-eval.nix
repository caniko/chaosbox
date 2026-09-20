# Render-only evaluation test for the Chaosbox deployment composition.
#
# Proves the harbor-db project (schema migration ordered after the TypeDB
# service), the service wiring, credential references, and systemd ordering
# evaluate correctly without booting anything. Uses a dummy password FILE
# (never a value in the config); nothing here executes.
{
  lib,
  pkgs,
  harborDbModule,
  typedbModule,
  chaosboxModule,
  chaosboxPackage,
  typedbPackage,
}:
let
  dummySecret = ../fixtures/eval-test-password;
  eval = import "${pkgs.path}/nixos/lib/eval-config.nix" {
    system = pkgs.system;
    modules = [
      harborDbModule
      typedbModule
      chaosboxModule
      {
        system.stateVersion = "24.11";
        # Placeholder binary: only its store path is interpolated into unit
        # scripts, which this test never executes. Using a plain package
        # (no writeShellScriptBin) keeps this evaluation free of
        # import-from-derivation.
        services.harbor-db.package = pkgs.hello;
        services.typedb.package = typedbPackage;
        services.chaosbox = {
          enable = true;
          package = chaosboxPackage;
          passwordFile = dummySecret;
        };
      }
    ];
  };
  project = eval.config.services.harbor-db.projects.chaosbox;
  schema = project.operations.schema;
  typedb = eval.config.services.typedb;
  migration = eval.config.systemd.services.harbor-db-chaosbox;
  checks = [
    {
      name = "typedb-backend-operation";
      assertion = schema.backend == "typedb";
      message = "schema operation must use the typedb backend";
    }
    {
      name = "ordering-gates-service";
      assertion =
        lib.elem "typedb.service" migration.after && lib.elem "typedb.service" migration.requires;
      message = "the migration must order after and require the TypeDB service unit";
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
          ]
        && schema.runner.credentialEnvironment.CHAOSBOX_TYPEDB_PASSWORD_FILE == "typedb-password";
      message = "schema runner must invoke the v2 contract commands with credential-file delivery";
    }
    {
      name = "backend-selection-env";
      assertion = project.environment.CHAOSBOX_DB_BACKEND == "typedb";
      message = "the project must route CLI commands at the TypeDB backend";
    }
    {
      name = "service-loopback";
      assertion = typedb.enable && typedb.listenHost == "127.0.0.1" && typedb.listenPort == 1729;
      message = "the TypeDB service must be enabled on loopback";
    }
    {
      name = "migration-unit-exists";
      assertion =
        (eval.config.systemd.services ? "harbor-db-chaosbox") && migration.serviceConfig.Type == "oneshot";
      message = "the generated migration unit must exist";
    }
    {
      name = "credential-mapping";
      assertion = schema.credentials.typedb-password == dummySecret;
      message = "the password file must map through credentials, never values";
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
