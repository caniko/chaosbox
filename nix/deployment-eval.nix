# Render-only evaluation test for the Chaosbox deployment composition.
#
# Proves the harbor-db project (ready gate -> schema migration), the Gel
# instance wiring, credential references, and systemd ordering evaluate
# correctly without booting anything. Uses a dummy password FILE (never a
# value in the config); nothing here executes.
{
  lib,
  pkgs,
  harborDbModule,
  gelModule,
  chaosboxModule,
  chaosboxPackage,
}: let
  dummySecret = ../fixtures/eval-test-password;
  eval = import "${pkgs.path}/nixos/lib/eval-config.nix" {
    system = pkgs.system;
    modules = [
      harborDbModule
      gelModule
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
          adminPasswordFile = dummySecret;
          adminCredsFile = dummySecret;
        };
      }
    ];
  };
  project = eval.config.services.harbor-db.projects.chaosbox;
  schema = project.operations.schema;
  ready = project.operations.ready;
  container = eval.config.virtualisation.oci-containers.containers.harbor-db-gel-chaosbox;
  migration = eval.config.systemd.services.harbor-db-chaosbox;
  checks = [
    {
      name = "gel-backend-operations";
      assertion = schema.backend == "gel" && ready.backend == "gel";
      message = "ready and schema operations must use the gel backend";
    }
    {
      name = "readiness-gates-migration";
      assertion = schema.dependsOn == ["ready"];
      message = "schema migration must depend on the readiness probe";
    }
    {
      name = "contract-commands";
      assertion =
        schema.runner.args == ["db" "migrate" "--json" "--repo" "demo"]
        && schema.runner.checkArgs == ["db" "check" "--json" "--repo" "demo"]
        && schema.runner.credentialEnvironment.CHAOSBOX_GEL_CREDENTIALS_FILE == "admin-creds";
      message = "schema runner must invoke the v1 contract commands with credential-file delivery";
    }
    {
      name = "server-image-pinned";
      assertion = lib.hasInfix "@sha256:" container.image;
      message = "the Gel server image must stay digest-pinned";
    }
    {
      name = "listener-loopback";
      assertion = container.ports == ["127.0.0.1:56561:5656"];
      message = "the Gel port must publish on loopback";
    }
    {
      name = "migration-unit-exists";
      assertion = (eval.config.systemd.services ? "harbor-db-chaosbox") && migration.serviceConfig.Type == "oneshot";
      message = "the generated migration unit must exist";
    }
    {
      name = "ordering-gates-container";
      assertion =
        lib.elem "podman-harbor-db-gel-chaosbox.service" migration.after
        && lib.elem "podman-harbor-db-gel-chaosbox.service" migration.requires;
      message = "the migration must order after and require the Gel container unit";
    }
  ];
  failed = builtins.filter (check: !check.assertion) checks;
in
  if failed == []
  # A trivial derivation: the assertions above already threw at evaluation
  # time on any mismatch, so reaching the builder means the composition
  # renders. CI builds this as part of nix flake check.
  then pkgs.runCommand "chaosbox-deployment-eval" {} ''echo "chaosbox deployment composition renders: ${toString (builtins.length checks)} checks" > $out''
  else throw "chaosbox deployment eval failed: ${lib.concatStringsSep ", " (map (check: "${check.name}: ${check.message}") failed)}"
