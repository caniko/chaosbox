# Two disposable Unix accounts, real OpenSSH, and the production HM endpoints.
{
  pkgs,
  homeManager,
  chaosboxPackage,
  federationTestTools,
}:
let
  root = "/var/lib/chaosbox-federation-test";
  project = "git:github.com/caniko/chaosbox";
  keys = import "${pkgs.path}/nixos/tests/ssh-keys.nix" pkgs;
  # Public test key from nixpkgs' borgbackup test; never a deployment key.
  hostPrivateKey = pkgs.writeText "federation-test-host-key" ''
    -----BEGIN OPENSSH PRIVATE KEY-----
    b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
    QyNTUxOQAAACBx8UB04Q6Q/fwDFjakHq904PYFzG9pU2TJ9KXpaPMcrwAAAJB+cF5HfnBe
    RwAAAAtzc2gtZWQyNTUxOQAAACBx8UB04Q6Q/fwDFjakHq904PYFzG9pU2TJ9KXpaPMcrw
    AAAEBN75NsJZSpt63faCuaD75Unko0JjlSDxMhYHAPJk2/xXHxQHThDpD9/AMWNqQer3Tg
    9gXMb2lTZMn0pelo8xyvAAAADXJzY2h1ZXR6QGt1cnQ=
    -----END OPENSSH PRIVATE KEY-----
  '';
  hostPublicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHHxQHThDpD9/AMWNqQer3Tg9gXMb2lTZMn0pelo8xyv";
  queryKeys = {
    can = {
      private = keys.snakeOilPrivateKey;
      public = keys.snakeOilPublicKey;
    };
    dejana = {
      private = keys.snakeOilEd25519PrivateKey;
      public = keys.snakeOilEd25519PublicKey;
    };
  };
  homes = pkgs.lib.genAttrs [ "can" "dejana" ] (
    owner:
    let
      peer = if owner == "can" then "dejana" else "can";
    in
    {
      home.stateVersion = "26.05";
      programs.chaosbox.federation = {
        enable = true;
        package = chaosboxPackage;
        hostName = "fixture";
        backend = {
          kind = "bundle";
          path = "${root}/${owner}/current.json";
          history = "${root}/${owner}/history";
        };
        fleetix = {
          enable = true;
          topology = {
            access.ssh.port = 2222;
            hosts.fixture = {
              network.lanIp = "127.0.0.1";
              hostPubkey = hostPublicKey;
              users = {
                can.hasAccount = true;
                dejana.hasAccount = true;
              };
            };
          };
        };
        projects.${project} = {
          repo = "${owner}-local";
          grants.${peer}.mode = "all_admitted";
        };
        peers."${peer}-fixture" = {
          identity.owner = peer;
          host = "fixture";
          identityFile = "${root}/${owner}/query-key";
          projects = [ project ];
        };
        authorizedKeys.${peer} = [ queryKeys.${peer}.public ];
      };
    }
  );
in
pkgs.testers.nixosTest {
  name = "chaosbox-federation-ssh";
  nodes.machine = {
    imports = [
      homeManager.nixosModules.home-manager
      ./federation-nixos.nix
    ];
    system.stateVersion = "26.05";
    virtualisation.memorySize = 1536;
    services.openssh = {
      enable = true;
      ports = [ 2222 ];
      hostKeys = [
        {
          path = "/etc/ssh/ssh_host_ed25519_key";
          type = "ed25519";
        }
      ];
      settings = {
        PasswordAuthentication = false;
        KbdInteractiveAuthentication = false;
      };
    };
    environment.etc."ssh/ssh_host_ed25519_key" = {
      source = hostPrivateKey;
      mode = "0600";
    };
    users.users = pkgs.lib.genAttrs [ "can" "dejana" ] (owner: {
      isNormalUser = true;
      home = "${root}/${owner}";
      hashedPassword = "";
    });
    home-manager = {
      useGlobalPkgs = true;
      useUserPackages = true;
      sharedModules = [ ./federation-home.nix ];
      users = homes;
    };
    services.chaosbox.federation.enable = true;
    environment.systemPackages = [
      pkgs.python3
      pkgs.openssh
      pkgs.util-linux
    ];
  };
  testScript = ''
    machine.succeed(
        "python3 ${../scripts/test-federation-gates.py} "
        "--ssh-gate ${../scripts/test-federation-ssh.py} "
        "--typedb-gate ${../scripts/test-typedb-federation.sh} -v"
    )
    machine.wait_for_unit("sshd.service")
    machine.wait_for_open_port(2222)
    for owner in ("can", "dejana"):
        machine.wait_for_unit(f"home-manager-{owner}.service")
    machine.succeed("install -m 600 -o can ${queryKeys.can.private} ${root}/can/query-key")
    machine.succeed("install -m 600 -o dejana ${queryKeys.dejana.private} ${root}/dejana/query-key")
    command = (
        "python3 ${../scripts/test-federation-ssh.py} --root ${root} "
        "--fixtures ${federationTestTools}/fixtures "
    )
    machine.succeed(command + "--phase connected")
    machine.succeed("systemctl stop sshd.service")
    machine.succeed(command + "--phase outage")
  '';
}
