{
  description = "Chaosbox - native Rust code-graph pipeline (Gel-backed)";

  inputs = {
    harbor-rs.url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=7a3328e186258dca31f9801227bc4e6fd8db4f36";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    harbor-meta.follows = "harbor-rs/harbor-meta";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    harbor-rs,
    harbor-meta,
    treefmt-nix,
    nixpkgs,
    crane,
    ...
  }: let
    systems = ["x86_64-linux"];
    forAllSystems = f:
      nixpkgs.lib.genAttrs systems (system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import harbor-rs.inputs.rust-overlay)];
        };
        toolchain = harbor-rs.lib.mkToolchain {
          inherit pkgs;
          toolchainProfile = "stable";
        };
      in
        f {
          inherit system pkgs toolchain;
          craneLib = toolchain.craneLib;
        });
    treefmt = system: pkgs:
      (treefmt-nix.lib.evalModule pkgs {
        projectRootFile = "flake.nix";
        programs.nixfmt.enable = true;
        programs.rustfmt.enable = true;
        programs.taplo.enable = true;
      }).config.build;
  in {
    packages = forAllSystems ({pkgs, craneLib, ...}: let
      commonArgs = {
        src = craneLib.cleanCargoSource ./.;
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
      chaosbox = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
      db-check = pkgs.writeShellApplication {
        name = "chaosbox-db-check";
        text = ''exec ${pkgs.lib.getExe chaosbox} db check --json "$@"'';
      };
      db-migrate = pkgs.writeShellApplication {
        name = "chaosbox-db-migrate";
        # Pinned Gel CLI (nixpkgs, locked in flake.lock) — never ambient PATH.
        runtimeInputs = [chaosbox pkgs.gel];
        text = ''exec ${pkgs.lib.getExe chaosbox} db migrate --json "$@"'';
      };
      test-gel = pkgs.writeShellApplication {
        name = "chaosbox-test-gel";
        runtimeInputs = [chaosbox pkgs.gel];
        text = builtins.readFile ./scripts/test-gel.sh;
      };
    in {
      inherit chaosbox db-check db-migrate test-gel;
      default = chaosbox;
    });

    apps = forAllSystems ({pkgs, ...}: let
      P = self.packages.${pkgs.stdenv.hostPlatform.system};
    in {
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
    });

    devShells = forAllSystems ({pkgs, toolchain, system, ...}: let
      cross = harbor-rs.lib.mkCross {inherit pkgs system;};
    in
      (harbor-rs.lib.mkDevShells {
        inherit pkgs cross;
        inherit (toolchain) craneLib;
      })
      // {
        default = harbor-rs.lib.mkDevShell {
          inherit pkgs cross;
          inherit (toolchain) craneLib;
        };
      });

    formatter = forAllSystems (args: (treefmt args.system args.pkgs).wrapper);

    checks = forAllSystems ({pkgs, craneLib, ...}: let
      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        pname = "chaosbox";
        version = "0.1.0";
        strictDeps = true;
        cargoExtraArgs = "--locked --workspace";
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
    in {
      fmt = (treefmt pkgs.stdenv.hostPlatform.system pkgs).check self;
      lint = craneLib.cargoClippy (commonArgs
        // {inherit cargoArtifacts; cargoClippyExtraArgs = "-- --deny warnings";});
      unit = craneLib.cargoTest (commonArgs // {inherit cargoArtifacts;});
      doc = craneLib.cargoDoc (commonArgs // {inherit cargoArtifacts;});
      packaging = self.packages.${pkgs.stdenv.hostPlatform.system}.chaosbox;
      gel-integration = pkgs.runCommand "chaosbox-gel-integration" {} ''
        ${pkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.test-gel} | tee $out
      '';
    });
  };
}
