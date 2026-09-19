# Container-free Gel integration test for Chaosbox (migrate+check scope).
#
# Boots the digest-pinned Gel server in a disposable NixOS guest (no host
# containers, no registry access: the image arrives via the Nix store),
# applies Chaosbox's real committed migrations, and proves the db
# check/migrate contract matrix. Conformance suites and the cache demo are
# follow-ups. Shape follows harbor-db's test-gel.nix; bits that differ
# (fixture, commands, assertions) are Chaosbox-owned.
{
  pkgs,
  chaosboxPackage,
  gelPackage,
}:
let
  # Chaosbox-owned schema, injected as a store closure (no checkout needed).
  schemaDir = ../dbschema;
  testPassword = "chaosbox-test-admin-pw";
  serverPort = 5656;
in
pkgs.testers.nixosTest {
  name = "chaosbox-gel";

  nodes.machine =
    { ... }:
    {
      virtualisation.memorySize = 4096;
      virtualisation.cores = 4;
      # The preloaded server image unpacks ~1.5GB into /var/lib/docker on
      # top of the system closure; the test-VM default disk is too small.
      virtualisation.diskSize = 10240;
      virtualisation.docker.enable = true;
      virtualisation.oci-containers.backend = "docker";
      # Preload through the Nix store: guests have no registry egress.
      # Tag form, deliberately: `docker load` drops RepoDigests, so the
      # digest ref cannot resolve locally; identical bits, and the digest
      # pin is enforced on the default below.
      virtualisation.oci-containers.containers."chaosbox-gel-test" = {
        imageFile = pkgs.dockerTools.pullImage {
          imageName = "geldata/gel";
          imageDigest = "sha256:b7270b0973da6950d01ae0d578c6d38cd8d87fabdd6c4b75a09b74291ad6f3a8";
          finalImageName = "geldata/gel";
          finalImageTag = "7.1";
          sha256 = "sha256-LldSfgB6p/cFRcmyE+XFGDL5U/vhPchIEv7AadizzO4=";
        };
        image = "geldata/gel:7.1";
        ports = [ "127.0.0.1:${toString serverPort}:5656" ];
        volumes = [
          "/var/lib/chaosbox-gel-test:/var/lib/gel/data"
          "/etc/chaosbox-gel-pw:/run/secrets/gel-server-password:ro"
        ];
        environment = {
          GEL_SERVER_DATADIR = "/var/lib/gel/data";
          GEL_SERVER_PASSWORD_FILE = "/run/secrets/gel-server-password";
          # Required: without a cert mode (and outside insecure_dev_mode,
          # which would also disable password auth) the server has no TLS
          # material and exits at startup. Matches the harbor-db default.
          GEL_SERVER_TLS_CERT_MODE = "generate_self_signed";
          # Project-owned migrations only; container startup never applies
          # schema implicitly.
          GEL_DOCKER_APPLY_MIGRATIONS = "never";
        };
      };

      # Test-only credential material; never leaves the disposable guest.
      environment.etc."chaosbox-gel-pw".text = testPassword;
      environment.systemPackages = [
        gelPackage
        chaosboxPackage
      ];
    };

  testScript = ''
    machine.wait_for_unit("docker.service")
    machine.wait_until_succeeds(
        "docker ps --format '{{.Names}}' | grep -qx chaosbox-gel-test", timeout=180
    )
    machine.succeed("mkdir -p /tmp/cbtest /var/lib/chaosbox-gel-test")
    machine.succeed("cp -r ${schemaDir} /tmp/cbtest/dbschema && chmod -R u+rw /tmp/cbtest")
    machine.succeed(
        "printf '%s' '{\"host\":\"127.0.0.1\",\"port\":5656,\"user\":\"admin\","
        "\"password\":\"${testPassword}\",\"branch\":\"main\",\"tls_security\":\"insecure\"}'"
        " > /tmp/creds.json && chmod 600 /tmp/creds.json"
    )
    machine.succeed(
        "printf '%s' '{\"host\":\"127.0.0.1\",\"port\":5656,\"user\":\"admin\","
        "\"password\":\"wrong-pw\",\"branch\":\"main\",\"tls_security\":\"insecure\"}'"
        " > /tmp/bad.json && chmod 600 /tmp/bad.json"
    )
    # Authenticated readiness, bounded (server-started != server-ready).
    machine.wait_until_succeeds(
        "gel --credentials-file /tmp/creds.json --connect-timeout 3s query 'select 1'",
        timeout=180,
    )

    ENV = (
        "GEL_CREDENTIALS_FILE=/tmp/creds.json "
        "CHAOSBOX_GEL_CREDENTIALS_FILE=/tmp/creds.json"
    )

    # Pending before any migration has applied.
    code, _out = machine.execute(
        f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test"
    )
    assert code == 2, f"pre-migration check must be pending(2), got {code}: {_out}"

    # Migrate exits 0 only after post-apply readiness verification.
    code, out = machine.execute(
        f"cd /tmp/cbtest && {ENV} chaosbox db migrate --json --repo test"
    )
    assert code == 0, f"migrate must succeed, got {code}: {out}"

    # Fresh database, no builds published yet: check is pending (not ready).
    # Schema readiness (migrate exit 0) and application readiness (check
    # exit 0 with an active build) are deliberately distinct gates.
    code, out = machine.execute(
        f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test"
    )
    assert code == 2, f"post-migration check must be pending(2), got {code}: {out}"

    # Seed a genesis build the way a pipeline would, then check is ready.
    machine.succeed(
        "gel --credentials-file /tmp/creds.json query "
        "\"insert GraphBuild { build_id := 'genesis-test', repo := 'test', "
        "generation := 1, status := 'ready' }; insert ActiveBuildPointer "
        "{ repo := 'test', build := "
        "(select GraphBuild filter .build_id = 'genesis-test') };\""
    )
    code, out = machine.execute(
        f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test"
    )
    assert code == 0, f"post-seed check must be ready(0), got {code}: {out}"
    assert '"status":"ready"' in out.replace(" ", ""), f"ready JSON expected: {out}"

    # Idempotent re-apply stays green.
    code, _out = machine.execute(
        f"cd /tmp/cbtest && {ENV} chaosbox db migrate --json --repo test"
    )
    assert code == 0, f"re-apply must stay green, got {code}"

    # Wrong credentials are an error, never ready/pending; state untouched.
    code, _out = machine.execute(
        "cd /tmp/cbtest && GEL_CREDENTIALS_FILE=/tmp/bad.json "
        "CHAOSBOX_GEL_CREDENTIALS_FILE=/tmp/bad.json "
        "chaosbox db check --json --repo test"
    )
    assert code != 0, "bad credentials must fail"
    code, _out = machine.execute(
        f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test"
    )
    assert code == 0, "good credentials still ready after the negative case"
  '';
}
