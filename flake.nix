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
    # GitHub-maintained PR merge (NixOS/nixpkgs#565068), pinned to the
    # verified merge 26996c2a9 that review-gha built green (typedb,
    # typedb-console, nixosTests.typedb). The bare PR head carries a stale
    # base toolchain (rustc 1.89; the driver needs 1.98) and cannot build.
    # Binaries substitute from the review cache; otherwise CI builds locally.
    # Removal: drop this input, use pkgs.typedb from nixpkgs.
    nixpkgs-typedb.url = "git+https://github.com/NixOS/nixpkgs.git?rev=26996c2a9def51106563a8983abe3617c76b2db7";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    harbor-meta.follows = "harbor-rs/harbor-meta";
    # github:caniko/harbor-docs redirects to the renamed harbor-projects repo.
    harbor-docs = {
      url = "github:caniko/harbor-projects/627df187070815ae286bd2061a6d0c30eaf5d6d1";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.harbor-meta.follows = "harbor-meta";
      inputs.treefmt-nix.follows = "treefmt-nix";
    };
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
      harbor-docs,
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
      # via include_str! or files read at test time (fixtures/)).
      # cleanCargoSource alone strips them and breaks nix builds while
      # cargo works.
      workspaceSrc =
        { pkgs, craneLib }:
        pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            craneLib.filterCargoSources path type
            || pkgs.lib.hasSuffix ".tql" (toString path)
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
            # `cargo test` execs pinned session tools through a node
            # interpreter; the build sandbox has no ambient node. Deployments
            # pin CHAOSBOX_NODE instead of relying on PATH.
            nativeBuildInputs = [ pkgs.nodejs ];
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
          docs = harbor-docs.lib.mkDocs {
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
          # Local TypeDB bootstrap for single-host pilots: generates
          # credentials once, converges passwords over the loopback
          # console, ensures the application user + database. Tested by
          # the typedb-bootstrap check below; consumed by host modules.
          typedb-bootstrap = pkgs.writeShellApplication {
            name = "typedb-bootstrap";
            runtimeInputs = [
              pkgs.bash
              pkgs.coreutils
              pkgs.gnugrep
              pkgs.openssl
              pkgs.util-linux
            ];
            text = builtins.readFile ./nix/typedb-bootstrap.sh;
          };
        in
        {
          inherit
            chaosbox
            docs
            db-check
            db-migrate
            test-typedb
            typedb-bootstrap
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
          docs = pkgs.mkShell { packages = [ pkgs.mdbook ]; };
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
            # Same node dependency as the package build: the unit check runs
            # `cargo test`, which execs pinned session tools.
            nativeBuildInputs = [ pkgs.nodejs ];
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
          docs = self.packages.${pkgs.stdenv.hostPlatform.system}.docs;
          docs-summary = harbor-docs.lib.mkSummaryCheck {
            inherit pkgs;
            src = ./docs;
          };
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
          # Full bootstrap lifecycle against the packaged server: empty
          # install, credential rotation, app auth, permissions, reboot,
          # interruption recovery, mismatch handling, non-default names.
          typedb-bootstrap-test = pkgs.callPackage ./nix/typedb-bootstrap-test.nix {
            typedbModule = "${nixpkgs-typedb}/nixos/modules/services/databases/typedb.nix";
            bootstrapPackage = self.packages.${pkgs.stdenv.hostPlatform.system}.typedb-bootstrap;
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
