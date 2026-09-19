{
  description = "Chaosbox - native Rust code-graph pipeline (Gel-backed)";

  inputs = {
    harbor-rs.url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=7a3328e186258dca31f9801227bc4e6fd8db4f36";
    # Deployment/lifecycle infrastructure (Gel backend, server module,
    # readiness gates). Pinned to the reviewed Gel-support revision; moves
    # to trunk after harbor-db#5 merges. Never a local path.
    harbor-db.url = "git+https://github.com/caniko/harbor-db.git?ref=gel-support&rev=c5770090031e2a7c03144b2c739481ad0949ff42";
    harbor-db.inputs.nixpkgs.follows = "nixpkgs";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    harbor-meta.follows = "harbor-rs/harbor-meta";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      harbor-db,
      harbor-rs,
      harbor-meta,
      treefmt-nix,
      nixpkgs,
      crane,
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
              overlays = [ (import harbor-rs.inputs.rust-overlay) ];
            };
            toolchain = harbor-rs.lib.mkToolchain {
              inherit pkgs;
              toolchainProfile = "stable";
            };
          in
          f {
            inherit system pkgs toolchain;
            craneLib = toolchain.craneLib;
          }
        );
      treefmt =
        system: pkgs:
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
    # Cargo source plus the non-Cargo trees Rust embeds (dbschema via
    # include_str!) or reads at test time (fixtures/). cleanCargoSource
    # alone strips them and breaks nix builds while cargo works.
    workspaceSrc = {pkgs, craneLib}:
      pkgs.lib.cleanSourceWith {
        src = ./.;
        filter = path: type:
          craneLib.filterCargoSources path type
          || pkgs.lib.hasPrefix (toString ./dbschema + "/") (toString path)
          || pkgs.lib.hasPrefix (toString ./fixtures + "/") (toString path);
      };
  in
    {
      nixosModules.chaosbox = import ./nix/chaosbox.nix;
      nixosModules.default = self.nixosModules.chaosbox;

      packages = forAllSystems (
        { pkgs, craneLib, ... }:
        let
          commonArgs = {
            src = workspaceSrc { inherit pkgs craneLib; };
            pname = "chaosbox";
            version = "0.1.0";
            strictDeps = true;
            cargoExtraArgs = "--locked -p chaosbox";
            meta = {
              description = "Chaosbox deterministic code-graph pipeline";
              homepage = "https://github.com/caniko/chaosbox";
              license = pkgs.lib.licenses.mit;
              mainProgram = "chaosbox";
            };
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          chaosbox = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });
          db-check = pkgs.writeShellApplication {
            name = "chaosbox-db-check";
            text = ''exec ${pkgs.lib.getExe chaosbox} db check --json "$@"'';
          };
          db-migrate = pkgs.writeShellApplication {
            name = "chaosbox-db-migrate";
            # Pinned Gel CLI (nixpkgs, locked in flake.lock) — never ambient PATH.
            runtimeInputs = [
              chaosbox
              pkgs.gel
            ];
            text = ''exec ${pkgs.lib.getExe chaosbox} db migrate --json "$@"'';
          };
          test-gel = pkgs.writeShellApplication {
            name = "chaosbox-test-gel";
            runtimeInputs = [
              chaosbox
              pkgs.gel
            ];
            text = builtins.readFile ./scripts/test-gel.sh;
          };
        in
        {
          inherit
            chaosbox
            db-check
            db-migrate
            test-gel
            ;
          default = chaosbox;
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
          test-gel = {
            type = "app";
            program = "${pkgs.lib.getExe P.test-gel}";
          };
        }
      );

      devShells = forAllSystems (
        {
          pkgs,
          toolchain,
          system,
          ...
        }:
        let
          cross = harbor-rs.lib.mkCross { inherit pkgs system; };
        in
        (harbor-rs.lib.mkDevShells {
          inherit pkgs cross;
          inherit (toolchain) craneLib;
        })
        // {
          default = harbor-rs.lib.mkDevShell {
            inherit pkgs cross;
            inherit (toolchain) craneLib;
            # Gel CLI for local lifecycle work (db migrate, test-gel). Same
            # source as harbor-db's cliPackage default (pkgs.gel); the
            # harbor-db devshell also provides it upstream
            # (chaosbox/gel-cli-devshell). Expect 7.x; record exact on re-pin.
            packages = [ pkgs.gel ];
          };
        }
      );

      formatter = forAllSystems (args: (treefmt args.system args.pkgs).wrapper);

      checks = forAllSystems (
        { pkgs, craneLib, ... }:
        let
          src = workspaceSrc { inherit pkgs craneLib; };
          commonArgs = {
            inherit src;
            pname = "chaosbox";
            version = "0.1.0";
            strictDeps = true;
            cargoExtraArgs = "--locked --workspace";
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
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
          doc = craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });
          packaging = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
          deployment-eval = pkgs.callPackage ./nix/deployment-eval.nix {
            harborDbModule = harbor-db.nixosModules.default;
            gelModule = harbor-db.nixosModules.gel;
            chaosboxModule = self.nixosModules.chaosbox;
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
          };
          gel-integration = pkgs.callPackage ./nix/gel-vm-test.nix {
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
            gelPackage = pkgs.gel;
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
