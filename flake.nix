{
  description = "Chaosbox - native Rust code-graph pipeline (PostgreSQL + pgvector)";

  inputs = {
    # Qualified shared Rust, database lifecycle and documentation infrastructure.
    harbor.url = "git+https://github.com/caniko/harbor.git?ref=feat/harbor-monorepo-components&rev=7d99eb50c52d0a941e2996b97c469b32a7657ef4";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    # Evaluation/runtime qualification of the portable Home Manager module.
    home-manager = {
      url = "github:nix-community/home-manager/0560d64401a6d5370732a69d08bc4a6b135962d9";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      harbor,
      treefmt-nix,
      home-manager,
      nixpkgs,
      ...
    }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ (import harbor.inputs.rust-overlay) ];
            };
            toolchain = harbor.lib.rust.mkToolchain {
              inherit pkgs;
              toolchainProfile = "stable";
            };
          in
          f {
            inherit system pkgs toolchain;
            inherit (toolchain) craneLib;
            # Server build for disposable live tests: the same major version
            # and extension the deployment module enables.
            postgresql = pkgs.postgresql_18.withPackages (ps: [ ps.pgvector ]);
          }
        );
      treefmt =
        _system: pkgs:
        (treefmt-nix.lib.evalModule pkgs {
          projectRootFile = "flake.nix";
          programs.nixfmt.enable = true;
          programs.rustfmt.enable = true;
          # Match Cargo.toml (edition 2021): treefmt defaults to 2024, whose
          # overflow rules disagree with `cargo fmt` on the same toolchain,
          # making the two gates unsatisfiable simultaneously.
          programs.rustfmt.edition = "2021";
          programs.taplo.enable = true;
        }).config.build;
      # Cargo source plus the non-Cargo trees Rust embeds (SQL schemas
      # via include_str! or files read at test time (fixtures/)).
      # cleanCargoSource alone strips them and breaks nix builds while
      # cargo works.
      workspaceSrc =
        {
          pkgs,
          craneLib,
        }:
        pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            craneLib.filterCargoSources path type
            || pkgs.lib.hasSuffix ".sql" (toString path)
            || pkgs.lib.hasPrefix (toString ./plugins + "/") (toString path)
            || pkgs.lib.hasPrefix (toString ./fixtures + "/") (toString path);
        };
    in
    {
      nixosModules.chaosbox = import ./nix/chaosbox.nix;
      nixosModules.default = self.nixosModules.chaosbox;
      nixosModules.federation = import ./nix/federation-nixos.nix;
      homeModules.federation =
        {
          lib,
          pkgs,
          ...
        }:
        {
          imports = [ ./nix/federation-home.nix ];
          programs.chaosbox.federation.package =
            lib.mkOptionDefault
              self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
        };
      homeManagerModules.federation = self.homeModules.federation;

      packages = forAllSystems (
        {
          pkgs,
          craneLib,
          postgresql,
          ...
        }:
        let
          commonArgs = {
            src = workspaceSrc { inherit pkgs craneLib; };
            pname = "chaosbox";
            version = "0.2.0";
            strictDeps = true;
            # Workspace provenance tests use disposable Git repositories.
            # `cargo test` execs pinned session tools through a node
            # interpreter; the build sandbox has no ambient node. Deployments
            # pin CHAOSBOX_NODE instead of relying on PATH.
            nativeBuildInputs = [
              pkgs.nodejs
              pkgs.gitMinimal
              pkgs.openssh
              pkgs.nix
            ];
            CHAOSBOX_TEST_NIX = pkgs.lib.getExe pkgs.nix;
            cargoExtraArgs = "--locked -p chaosbox";
            meta = {
              description = "Chaosbox deterministic code-graph pipeline";
              homepage = "https://github.com/caniko/chaosbox";
              license = pkgs.lib.licenses.mit;
              mainProgram = "chaosbox";
            };
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          chaosbox = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];
              # Runtime, not just build: the binary execs pinned session tools
              # through a node interpreter. Default CHAOSBOX_NODE to the store
              # node so a standalone Nix install works with no ambient PATH;
              # --set-default keeps an explicit CHAOSBOX_NODE (the canix
              # wrapper, or an operator override) authoritative. Declared here
              # rather than in `commonArgs`, which `buildDepsOnly` also
              # consumes and in which no binary exists to wrap. Workspace
              # freshness/revision checks also need Git at runtime.
              postInstall = ''
                wrapProgram "$out/bin/chaosbox" \
                  --set-default CHAOSBOX_NODE ${pkgs.lib.getExe' pkgs.nodejs "node"} \
                  --prefix PATH : ${
                    pkgs.lib.makeBinPath [
                      pkgs.gitMinimal
                      pkgs.postgresql
                      pkgs.openssh
                    ]
                  }
              '';
              passthru.scratchPlugin = "${./plugins}/chaosbox-scratch/index.ts";
              passthru.scratchAssessmentVersion = 1;
              passthru.nixOperationVersion = 1;
              passthru.nixPlugin = "${./plugins}/chaosbox-nix/index.ts";
              passthru.federationVersion = 1;
              passthru.intelligencePlugin = "${./plugins}/chaosbox-intelligence/index.ts";
              passthru.compactionPlugin = "${./plugins}/chaosbox-compaction";
              passthru.compactionVersion = 2;
              passthru.compactionSource = self.outPath;
              doInstallCheck = true;
              installCheckPhase = ''
                runHook preInstallCheck
                CHAOSBOX_TEST_BIN="$out/bin/chaosbox" node --test plugins/chaosbox-compaction/test/runtime.test.mjs
                runHook postInstallCheck
              '';
            }
          );
          docs = harbor.lib.docs.mkDocs {
            inherit pkgs;
            src = ./docs;
            pname = "chaosbox-docs";
          };
          db-check = pkgs.writeShellApplication {
            name = "chaosbox-db-check";
            text = ''exec ${pkgs.lib.getExe chaosbox} db check --json "$@"'';
          };
          db-migrate = pkgs.writeShellApplication {
            name = "chaosbox-db-migrate";
            # Migration runs through the driver inside chaosbox (no CLI
            # tooling needed); the connection arrives via
            # CHAOSBOX_DATABASE_URL at runtime, never ambient PATH.
            runtimeInputs = [ chaosbox ];
            text = ''exec ${pkgs.lib.getExe chaosbox} db migrate --json "$@"'';
          };
          # Disposable-cluster integration gate, run from a source checkout.
          test-postgres = pkgs.writeShellApplication {
            name = "chaosbox-test-postgres";
            runtimeInputs = [
              chaosbox
              postgresql
              pkgs.nodejs
              pkgs.python3
            ];
            text = builtins.readFile ./scripts/test-postgres.sh;
          };
        in
        {
          inherit
            chaosbox
            docs
            db-check
            db-migrate
            test-postgres
            ;
          default = chaosbox;
          site = docs;
        }
      );

      apps = forAllSystems (
        { pkgs, ... }:
        let
          P = self.packages.${pkgs.stdenv.hostPlatform.system};
        in
        {
          default = {
            type = "app";
            program = "${pkgs.lib.getExe P.chaosbox}";
          };
          chaosbox = {
            type = "app";
            program = "${pkgs.lib.getExe P.chaosbox}";
          };
          db-check = {
            type = "app";
            program = "${pkgs.lib.getExe P.db-check}";
          };
          db-migrate = {
            type = "app";
            program = "${pkgs.lib.getExe P.db-migrate}";
          };
          test-postgres = {
            type = "app";
            program = "${pkgs.lib.getExe P.test-postgres}";
          };
        }
      );

      devShells = forAllSystems (
        {
          pkgs,
          toolchain,
          system,
          postgresql,
          ...
        }:
        let
          cross = harbor.lib.rust.mkCross { inherit pkgs system; };
        in
        (harbor.lib.rust.mkDevShells {
          inherit pkgs cross;
          inherit (toolchain) craneLib;
        })
        // rec {
          default =
            (harbor.lib.rust.mkDevShell {
              inherit pkgs cross;
              inherit (toolchain) craneLib;
              # postgresql: disposable clusters for `-- --ignored live_pg_`.
              packages = [
                pkgs.cargo-nextest
                postgresql
                (treefmt system pkgs).wrapper
              ];
            }).overrideAttrs
              (_: {
                CHAOSBOX_TEST_NIX = pkgs.lib.getExe pkgs.nix;
              });
          docs = pkgs.mkShell { packages = [ pkgs.mdbook ]; };
        }
      );

      formatter = forAllSystems (args: (treefmt args.system args.pkgs).wrapper);

      checks = forAllSystems (
        {
          pkgs,
          craneLib,
          postgresql,
          ...
        }:
        let
          src = workspaceSrc { inherit pkgs craneLib; };
          commonArgs = {
            inherit src;
            pname = "chaosbox";
            version = "0.2.0";
            strictDeps = true;
            # Same test dependencies as the package build: Node for session
            # tools, Git for provenance fixtures, SSH for bounded transport tests.
            nativeBuildInputs = [
              pkgs.nodejs
              pkgs.gitMinimal
              pkgs.openssh
            ];
            cargoExtraArgs = "--locked --workspace";
            CHAOSBOX_TEST_NIX = pkgs.lib.getExe pkgs.nix;
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          federationTestTools = import ./nix/federation-test-tools.nix {
            inherit
              pkgs
              craneLib
              commonArgs
              cargoArtifacts
              ;
          };
        in
        {
          fmt = (treefmt pkgs.stdenv.hostPlatform.system pkgs).check self;
          lint = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "-- --deny warnings";
            }
          );
          unit = craneLib.cargoTest (commonArgs // { inherit cargoArtifacts; });
          # Backend contract tests against disposable PostgreSQL clusters,
          # one per test on a private socket inside the build sandbox.
          postgres-live = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
              pnameSuffix = "-postgres-live";
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ postgresql ];
              cargoTestExtraArgs = "-- --ignored live_pg_";
            }
          );
          nix-plugin =
            pkgs.runCommand "chaosbox-nix-plugin"
              {
                nativeBuildInputs = [ pkgs.nodejs ];
              }
              ''
                node --test ${./plugins/chaosbox-nix}/test/*.test.mjs
                touch "$out"
              '';
          intelligence-plugin =
            pkgs.runCommand "chaosbox-intelligence-plugin"
              {
                nativeBuildInputs = [ pkgs.nodejs ];
              }
              ''
                cp -r ${./plugins/chaosbox-intelligence} plugin
                chmod -R u+w plugin
                node --experimental-strip-types --test plugin/test/*.test.mjs
                node --test ${./plugins/chaosbox-nix}/test/*.test.mjs
                touch "$out"
              '';
          scratch-plugin =
            pkgs.runCommand "chaosbox-scratch-plugin"
              {
                nativeBuildInputs = [ pkgs.nodejs ];
              }
              ''
                cp -r ${./plugins} plugins
                chmod -R u+w plugins
                node --test plugins/chaosbox-scratch/test/*.test.mjs
                touch $out
              '';
          doc = craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });
          docs = self.packages.${pkgs.stdenv.hostPlatform.system}.docs;
          docs-summary = harbor.lib.docs.mkSummaryCheck {
            inherit pkgs;
            src = ./docs;
          };
          packaging = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
          deployment-eval = pkgs.callPackage ./nix/deployment-eval.nix {
            harborDbModule = harbor.nixosModules.database-default;
            chaosboxModule = self.nixosModules.chaosbox;
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
          };
          federation-home =
            let
              test = import ./nix/federation-home-test.nix {
                inherit pkgs;
                homeManager = home-manager;
              };
              evaluation = pkgs.writeText "chaosbox-federation-evaluation.json" (
                builtins.toJSON {
                  inherit (test) sshConfig provider client;
                }
              );
            in
            pkgs.runCommand "chaosbox-federation-home"
              {
                nativeBuildInputs = [
                  pkgs.openssh
                  pkgs.python3
                ];
              }
              ''
                python3 ${./scripts/test-federation-home.py} \
                  --evaluation ${evaluation} \
                  --chaosbox ${pkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox}
                echo "Home Manager federation renders and SSH remains query-key isolated" > "$out"
              '';
          postgres-integration = pkgs.callPackage ./nix/postgres-vm-test.nix {
            harborDbModule = harbor.nixosModules.database-default;
            chaosboxModule = self.nixosModules.chaosbox;
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
          };
          federation-ssh = pkgs.callPackage ./nix/federation-ssh-test.nix {
            inherit federationTestTools;
            homeManager = home-manager;
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
          };
          # Fail if flake inputs ever point at the retired Codeberg/Codefloe
          # mirrors again (fleet migrated to github.com/caniko/*).
          host-pinning =
            let
              flakeInputs = pkgs.lib.fileset.toSource {
                root = ./.;
                fileset = pkgs.lib.fileset.unions [
                  ./flake.nix
                  ./flake.lock
                ];
              };
              # Split across literals so this file never matches its own pattern.
              staleHosts = "cod" + "eberg|cod" + "efloe";
            in
            pkgs.runCommand "chaosbox-host-pinning" { } ''
              if ${pkgs.lib.getExe pkgs.ripgrep} -q "${staleHosts}" ${flakeInputs}; then
                echo "ERROR: retired forge host in flake inputs:" >&2
                ${pkgs.lib.getExe pkgs.ripgrep} -n "${staleHosts}" ${flakeInputs} >&2 || true
                exit 1
              fi
              touch $out
            '';
        }
      );
    };
}
