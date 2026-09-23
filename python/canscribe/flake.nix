{
  description = "canscribe: Audio/video transcription CLI";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    harbor-py = {
      url = "git+https://github.com/caniko/harbor-py.git?rev=9a6231b80acdaa29e2d2bd7145db08c71ddf1fd1";
      inputs.nixpkgs.follows = "nixpkgs";
    };


    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = {
    self,
    nixpkgs,
    harbor-py,
    treefmt-nix,
    git-hooks,
    ...
  }: let
    inherit (nixpkgs) lib;
    py = harbor-py.lib;

    mkRuntime = pkgs: let
      ffmpeg = py.mkFfmpegCompat {inherit pkgs;};
      opencv = pkgs.python313Packages.opencv4;
    in {
      inherit ffmpeg opencv;

      basePackages = [
        ffmpeg
        opencv
      ];

      baseLibs =
        [
          ffmpeg
          pkgs.stdenv.cc.cc.lib
          pkgs.dav1d
          pkgs.libdrm
          pkgs.libsndfile
          pkgs.zlib # triton's _C extension
          pkgs.zstd
          opencv
        ]
        ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
          pkgs.libGL
          pkgs.libxkbcommon
          pkgs.xorg.libX11
          pkgs.xorg.libXext
          pkgs.xorg.libSM
          pkgs.xorg.libICE
          pkgs.xorg.libxcb
        ];
    };

    mkPyprojectOverrides = {
      pkgs,
      python,
      runtime,
    }: final: prev: {
      insightface = prev.insightface.overrideAttrs (old: {
        nativeBuildInputs =
          (old.nativeBuildInputs or [])
          ++ [
            final.setuptools
            final.numpy
            final.cython
          ];
      });
      numba = prev.numba.overrideAttrs (old: {
        buildInputs =
          (old.buildInputs or [])
          ++ [
            pkgs.tbb
          ];
      });
      julius = py.pythonOverrides.addSetuptools final prev "julius";
      torchaudio = py.pythonOverrides.addTorchRuntime python final prev "torchaudio";
      torchcodec =
        py.pythonOverrides.addTorchCodecFfmpegRuntime {
          inherit python;
          ffmpeg = runtime.ffmpeg;
        }
        final
        prev;
      torchvision = py.pythonOverrides.addTorchRuntime python final prev "torchvision";
    };

    mkDevShells = system: let
      pkgs = py.mkPkgs {inherit system;};
      python = pkgs.python313;
      runtime = mkRuntime pkgs;
      isX86Linux = system == "x86_64-linux";
      isAarch64Darwin = system == "aarch64-darwin";
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
        };
      };

      # Diagnostic tools only. The ROCm PyTorch wheel bundles its own ROCm
      # userspace; exposing nixpkgs ROCm libs via LD_LIBRARY_PATH (or clr's
      # setup hook exporting HIP_DEVICE_LIB_PATH) shadows the wheel's
      # runtime with an incompatible version and segfaults on first kernel
      # launch. Keep clr/rocm-runtime out of the shell.
      rocmTools = with pkgs.rocmPackages; [
        rocminfo
        rocm-smi
      ];

      mkDevShell = {
        uvExtra,
        extraPackages ? [],
        extraLibs ? [],
        extraEnv ? {},
      }:
        py.mkUvDevShell {
          inherit
            pkgs
            python
            uvExtra
            extraEnv
            ;
          autoSync = false;
          basePackages = runtime.basePackages ++ [pkgs.mdbook];
          extraPackages = pre-commit-check.enabledPackages ++ extraPackages;
          baseLibs = runtime.baseLibs;
          extraLibs = extraLibs;
          pythonPathEntries = ["${runtime.opencv}/${python.sitePackages}"];
          shellHookSuffix = pre-commit-check.shellHook;
        };

      cpuShell = mkDevShell {
        uvExtra = "cpu";
      };

      appleShell = mkDevShell {
        uvExtra = "apple";
      };
    in
      {
        docs = pkgs.mkShell {
          packages = [pkgs.mdbook];
        };

        cpu = cpuShell;
        default =
          if isAarch64Darwin
          then appleShell
          else cpuShell;
      }
      // lib.optionalAttrs isAarch64Darwin {
        apple = appleShell;
      }
      // lib.optionalAttrs isX86Linux {
        amd = mkDevShell {
          uvExtra = "amd";
          extraPackages = rocmTools;
          extraEnv = {
            ROCR_VISIBLE_DEVICES = "0";
            HIP_VISIBLE_DEVICES = "0";
            CUDA_VISIBLE_DEVICES = "0";
          };
        };

        nvidia = mkDevShell {
          uvExtra = "nvidia";
          extraPackages = [
            pkgs.cudaPackages.cuda_cudart
            pkgs.cudaPackages.cuda_nvcc
            pkgs.cudaPackages.cudnn
            pkgs.ninja
            pkgs.gcc
          ];
          extraLibs = [
            pkgs.cudaPackages.cuda_cudart
            pkgs.cudaPackages.cuda_nvcc
            pkgs.cudaPackages.cudnn
            pkgs.addDriverRunpath.driverLink
          ];
          extraEnv = {
            CUDA_HOME = "${pkgs.cudaPackages.cuda_nvcc}";
          };
        };
      };

    mkPythonPackage = system: let
      pkgs = py.mkPkgs {inherit system;};
      python = pkgs.python313;
      runtime = mkRuntime pkgs;
      cpuDependencySpec = {
        canscribe = ["cpu"];
      };
    in
      py.mkUvAppPackage {
        inherit pkgs python;
        name = "canscribe-cpu";
        envName = "canscribe-cpu-env";
        workspaceRoot = ./.;
        dependencies = cpuDependencySpec;
        scripts = [
          "canscribe"
          "ct"
        ];
        runtimePathPackages = [runtime.ffmpeg];
        runtimeLibraryPackages = runtime.baseLibs;
        pyprojectOverrides = mkPyprojectOverrides {inherit pkgs python runtime;};
      };

    mkPythonCheckEnv = system: let
      pkgs = py.mkPkgs {inherit system;};
      python = pkgs.python313;
      runtime = mkRuntime pkgs;
      checkDependencySpec = {
        canscribe = [
          "cpu"
          "dev"
        ];
      };
    in
      py.mkUvCheckEnv {
        inherit pkgs python;
        name = "canscribe-cpu-check-env";
        workspaceRoot = ./.;
        dependencies = checkDependencySpec;
        pyprojectOverrides = mkPyprojectOverrides {inherit pkgs python runtime;};
      };

    mkChecks = system: let
      pkgs = py.mkPkgs {inherit system;};
      runtime = mkRuntime pkgs;
      canscribe = self.packages.${system}.canscribe-cpu;
      checkEnv = mkPythonCheckEnv system;
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      ffmpegAbiCheck = py.mkFfmpegTorchCodecAbiCheck {
        inherit pkgs;
        ffmpeg = runtime.ffmpeg;
        name = "canscribe-ffmpeg-torchcodec-abi-check";
      };
    in {
      flake-eval = pkgs.runCommand "canscribe-flake-eval" {} ''
        test -x ${canscribe}/bin/canscribe
        test -x ${runtime.ffmpeg}/bin/ffmpeg
        test -e ${ffmpegAbiCheck}/result
        mkdir -p $out
        echo ok > $out/result
      '';

      formatting = treefmtEval.config.build.check self;

      offline-tests = pkgs.runCommand "canscribe-offline-tests" {} ''
        export HOME=$TMPDIR/home
        export XDG_CACHE_HOME=$TMPDIR/cache
        export LD_LIBRARY_PATH=${lib.makeLibraryPath runtime.baseLibs}
        mkdir -p "$HOME" "$XDG_CACHE_HOME" "$out"
        cd ${./.}
        ${checkEnv}/bin/python -m pytest -p no:cacheprovider
        echo ok > $out/result
      '';

      typecheck = pkgs.runCommand "canscribe-typecheck" {} ''
        export HOME=$TMPDIR/home
        export XDG_CACHE_HOME=$TMPDIR/cache
        export LD_LIBRARY_PATH=${lib.makeLibraryPath runtime.baseLibs}
        mkdir -p "$HOME" "$XDG_CACHE_HOME" "$out"
        cd ${./.}
        ${checkEnv}/bin/python -m mypy .
        echo ok > $out/result
      '';
    };

    mkDocs = system: let
      pkgs = py.mkPkgs {inherit system;};
    in
      pkgs.stdenvNoCC.mkDerivation {
        pname = "canscribe-docs";
        version = "0.1.0";
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.maybeMissing ./docs;
        };
        nativeBuildInputs = [pkgs.mdbook];
        phases = [
          "buildPhase"
          "installPhase"
        ];
        buildPhase = ''
          cp -r --no-preserve=mode $src/docs docs
          mdbook build docs
        '';
        installPhase = ''
          cp -r docs/book $out
        '';
      };
  in {
    devShells = py.forAllSystems mkDevShells;

    packages = py.forPackageSystems (
      system: let
        canscribeCpu = mkPythonPackage system;
        docs = mkDocs system;
      in {
        canscribe-cpu = canscribeCpu;
        inherit docs;
        site = docs;
        default = canscribeCpu;
      }
    );

    apps = py.forPackageSystems (
      system: let
        canscribeCpu = self.packages.${system}.canscribe-cpu;
      in {
        canscribe-cpu = {
          type = "app";
          program = "${canscribeCpu}/bin/canscribe";
        };
        default = self.apps.${system}.canscribe-cpu;
      }
    );

    formatter = py.forAllSystems (
      system: let
        pkgs = py.mkPkgs {inherit system;};
        treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      in
        treefmtEval.config.build.wrapper
    );

    checks = py.forPackageSystems mkChecks;
  };
}
