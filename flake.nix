{
  description = "Chaosbox - native Rust code-graph pipeline (TypeDB-backed)";

  inputs = {
    harbor-rs.url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=7a3328e186258dca31f9801227bc4e6fd8db4f36";
    # Deployment/lifecycle infrastructure (TypeDB backend, server module,
    # readiness gates). Tracks trunk (harbor-db#7 merged); previously the
    # reviewed typedb-backend branch. Never a local path.
    harbor-db.url = "git+https://github.com/caniko/harbor-db.git?ref=trunk&rev=70407e3223a4b8fcf6db5986de691934f04e2028";
    harbor-db.inputs.nixpkgs.follows = "nixpkgs";
    # TEMPORARY TypeDB packages until NixOS/nixpkgs#565068 merges: the
    # packaging commits on a current master (the PR branch itself cannot be
    # rebased from here; same content, fresh toolchain). Binaries substitute
    # from the fleet cache once built; otherwise CI builds locally.
    # Removal: drop this input, use pkgs.typedb from nixpkgs.
    nixpkgs-typedb.url = "github:caniko/nixpkgs/cd2831605ded4b33c2b8d652bbb01bfdeedcb85c";
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
      nixpkgs-typedb,
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
            # Temporary TypeDB packages (see nixpkgs-typedb input): the
            # same nixpkgs revision that carries the packaging PR, so the
            # service module and the binaries agree. Substituted from the
            # fleet cache, never built here.
            typedbPkgs = import nixpkgs-typedb { inherit system; };
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
      # Cargo source plus the non-Cargo trees Rust embeds (TypeQL schema
      # via include_str!, the Gel SDL assets the gel crate packages, or
      # files read at test time (fixtures/)). cleanCargoSource alone strips
      # them and breaks nix builds while cargo works.
      workspaceSrc =
        { pkgs, craneLib }:
        pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            craneLib.filterCargoSources path type
            || pkgs.lib.hasSuffix ".tql" (toString path)
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
            # Migration runs through the driver inside chaosbox (no CLI
            # tooling needed); connection arrives via environment + the
            # credential file at runtime, never ambient PATH.
            runtimeInputs = [ chaosbox ];
            text = ''exec ${pkgs.lib.getExe chaosbox} db migrate --json "$@"'';
          };
          test-typedb = pkgs.writeShellApplication {
            name = "chaosbox-test-typedb";
            runtimeInputs = [
              chaosbox
            ];
            text = builtins.readFile ./scripts/test-typedb.sh;
          };
        in
        {
          inherit
            chaosbox
            db-check
            db-migrate
            test-typedb
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
          test-typedb = {
            type = "app";
            program = "${pkgs.lib.getExe P.test-typedb}";
          };
        }
      );

      devShells = forAllSystems (
        {
          pkgs,
          toolchain,
          system,
          typedbPkgs,
          ...
        }:
        let
          cross = harbor-rs.lib.mkCross { inherit pkgs system; };
        in
        (harbor-rs.lib.mkDevShells {
          inherit pkgs cross;
          inherit (toolchain) craneLib;
        })
        // rec {
          default = harbor-rs.lib.mkDevShell {
            inherit pkgs cross;
            inherit (toolchain) craneLib;
          };
          # Live TypeDB work (db migrate, backend tests, test-typedb.sh):
          # server + Console from the temporary packages. Opt-in so the
          # default shell (and every CI gate using it) never builds them;
          # binaries substitute from the fleet cache once review publishes.
          typedb = harbor-rs.lib.mkDevShell {
            inherit pkgs cross;
            inherit (toolchain) craneLib;
            packages = [
              typedbPkgs.typedb
              typedbPkgs.typedb-console
            ];
          };
          # Simit-generated CI builds docs via `.#docs`; same shell.
          docs = default;
        }
      );

      formatter = forAllSystems (args: (treefmt args.system args.pkgs).wrapper);

      checks = forAllSystems (
        {
          pkgs,
          craneLib,
          typedbPkgs,
          ...
        }:
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
            typedbModule = "${nixpkgs-typedb}/nixos/modules/services/databases/typedb.nix";
            chaosboxModule = self.nixosModules.chaosbox;
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
            typedbPackage = typedbPkgs.typedb;
          };
          typedb-integration = pkgs.callPackage ./nix/typedb-vm-test.nix {
            harborDbModule = harbor-db.nixosModules.default;
            typedbModule = "${nixpkgs-typedb}/nixos/modules/services/databases/typedb.nix";
            chaosboxModule = self.nixosModules.chaosbox;
            chaosboxPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
            typedbPackage = typedbPkgs.typedb;
            typedbConsolePackage = typedbPkgs.typedb-console;
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
