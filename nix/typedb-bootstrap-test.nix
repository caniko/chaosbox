# TypeDB bootstrap lifecycle test (credential provisioning scope).
#
# Boots the packaged TypeDB server, then exercises the `typedb-bootstrap`
# package through Atlas-equivalent systemd units (hardened root
# provisioning after the server, migration with a loaded credential)
# using NON-DEFAULT user/database names: automatic first-boot
# provisioning, rotation of the bootstrap default, application auth, file
# permissions, exact-name user matching with out-of-band deletion
# recovery, idempotent re-run, partial recovery, credential-mismatch
# handling, journal secrecy, reboot persistence (units re-converge,
# migrated schema stays pending for lack of a build), and fail-closed
# credential loss. Nothing here builds TypeDB.
{
  pkgs,
  typedbModule,
  bootstrapPackage,
  chaosboxPackage,
  typedbPackage,
  typedbConsolePackage,
}:
let
  appUser = "testapp";
  testDb = "testdb";
  authDir = "/var/lib/typedb-auth";
  evilDir = "/var/lib/typedb-auth-evil";
  bootEnv =
    "TYPEDB_BOOTSTRAP_AUTH_DIR=${authDir} "
    + "TYPEDB_BOOTSTRAP_ADDR=127.0.0.1:1729 "
    + "TYPEDB_BOOTSTRAP_CONSOLE_BIN=${typedbConsolePackage}/bin/typedb-console "
    + "TYPEDB_BOOTSTRAP_APP_USER=${appUser} "
    + "TYPEDB_BOOTSTRAP_DATABASE=${testDb} "
    + "TYPEDB_BOOTSTRAP_APP_OWNER=testuser ";
  evilEnv = builtins.replaceStrings [ authDir ] [ evilDir ] bootEnv;
  # Test-only console helper: passwords travel via argv here, visible to
  # root in the disposable test VM. The product under test never does
  # this (pty stdin); the VM asserts the product's observable behavior.
  consoleFor =
    user: pwFile:
    "${typedbConsolePackage}/bin/typedb-console --address 127.0.0.1:1729 --tls-disabled --username ${user} --password $(cat ${pwFile})";
  migrateEnv =
    "CHAOSBOX_DB_BACKEND=typedb CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 "
    + "CHAOSBOX_TYPEDB_USER=${appUser} CHAOSBOX_TYPEDB_PASSWORD_FILE=${authDir}/app-password "
    + "CHAOSBOX_TYPEDB_DATABASE=${testDb} ";
in
pkgs.testers.nixosTest {
  name = "typedb-bootstrap";

  nodes.machine =
    { ... }:
    {
      imports = [ typedbModule ];
      system.stateVersion = "24.11";
      virtualisation.memorySize = 4096;
      virtualisation.cores = 4;

      services.typedb = {
        enable = true;
        package = typedbPackage;
        listenHost = "127.0.0.1";
        listenPort = 1729;
        httpListenHost = "127.0.0.1";
        httpListenPort = 8000;
        openFirewall = false;
        diagnosticsReporting = false;
        diagnosticsMonitoring = false;
      };

      users.users.testuser = {
        isNormalUser = true;
        description = "Unrelated local user for permission checks";
      };

      environment.systemPackages = [
        bootstrapPackage
        chaosboxPackage
        typedbConsolePackage
      ];

      # Atlas-equivalent service wiring under test: bootstrap runs as a
      # hardened root oneshot after the server, migration follows with the
      # application credential loaded from the auth directory. Unit names
      # are test-local; ordering, sandboxing, and credential flow mirror
      # root/hosts/atlas/server/ai/chaosbox.nix. RemainAfterExit is a
      # test-only affordance so readiness is observable.
      systemd.services.typedb-bootstrap = {
        description = "TypeDB credential provisioning (test)";
        wantedBy = [ "multi-user.target" ];
        after = [ "typedb.service" ];
        requires = [ "typedb.service" ];
        environment = {
          TYPEDB_BOOTSTRAP_AUTH_DIR = authDir;
          TYPEDB_BOOTSTRAP_ADDR = "127.0.0.1:1729";
          TYPEDB_BOOTSTRAP_CONSOLE_BIN = "${typedbConsolePackage}/bin/typedb-console";
          TYPEDB_BOOTSTRAP_APP_USER = appUser;
          TYPEDB_BOOTSTRAP_DATABASE = testDb;
          TYPEDB_BOOTSTRAP_APP_OWNER = "testuser";
        };
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          TimeoutStartSec = "5min";
          ExecStart = "${bootstrapPackage}/bin/typedb-bootstrap";
          # Created before sandbox setup: ReadWritePaths below requires
          # an existing source, and nothing else provides this directory.
          StateDirectory = "typedb-auth";
          NoNewPrivileges = true;
          PrivateTmp = true;
          ProtectHome = true;
          ProtectSystem = "strict";
          ReadWritePaths = [ authDir ];
          RestrictAddressFamilies = [
            "AF_UNIX"
            "AF_INET"
            "AF_INET6"
          ];
        };
      };

      systemd.services.chaosbox-migrate-boot = {
        description = "Chaosbox schema migration (test)";
        wantedBy = [ "multi-user.target" ];
        after = [
          "typedb.service"
          "typedb-bootstrap.service"
        ];
        requires = [
          "typedb.service"
          "typedb-bootstrap.service"
        ];
        environment = {
          CHAOSBOX_DB_BACKEND = "typedb";
          CHAOSBOX_TYPEDB_ADDR = "127.0.0.1:1729";
          CHAOSBOX_TYPEDB_USER = appUser;
          CHAOSBOX_TYPEDB_DATABASE = testDb;
          CHAOSBOX_TYPEDB_PASSWORD_FILE = "%d/typedb-password";
        };
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          TimeoutStartSec = "5min";
          WorkingDirectory = "/tmp";
          ExecStart = "${chaosboxPackage}/bin/chaosbox db migrate --json --repo testboot";
          LoadCredential = "typedb-password:${authDir}/app-password";
          NoNewPrivileges = true;
          PrivateTmp = true;
          ProtectHome = true;
          ProtectSystem = "strict";
          RestrictAddressFamilies = [
            "AF_UNIX"
            "AF_INET"
            "AF_INET6"
          ];
        };
      };
    };

  testScript = ''
    machine.wait_for_unit("typedb.service")
    machine.wait_for_open_port(1729)

    # First boot provisions and migrates automatically through the units,
    # exactly as Atlas orders them.
    machine.wait_for_unit("typedb-bootstrap.service")
    machine.wait_for_unit("chaosbox-migrate-boot.service")

    # Permissions: traversable-but-unlistable dir, owner-only files, and
    # the application credential readable by its operator owner.
    code, out = machine.execute("stat -c %a ${authDir}")
    assert code == 0 and out.strip() == "711", f"auth dir must be 0711: {out}"
    code, out = machine.execute("stat -c '%a %U' ${authDir}/admin-password")
    assert code == 0 and out.strip() == "400 root", f"admin secret must be 0400 root: {out}"
    code, out = machine.execute("stat -c '%a %U' ${authDir}/app-password")
    assert code == 0 and out.strip() == "400 testuser", f"app secret must be 0400 testuser: {out}"
    code, _out = machine.execute("su testuser -c 'ls ${authDir}'")
    assert code != 0, "unrelated listing of the auth dir must fail"
    code, _out = machine.execute("su testuser -c 'cat ${authDir}/admin-password'")
    assert code != 0, "operator must not read the admin secret"
    machine.succeed("su testuser -c 'cat ${authDir}/app-password'")

    # Bootstrap default no longer authenticates; application does. Only
    # admins may list users, so existence is asserted from the admin side
    # with an exact match while the application proves auth itself.
    machine.succeed("printf '%s' 'password' > /tmp/bootstrap-pw-default && chmod 600 /tmp/bootstrap-pw-default")
    code, _out = machine.execute("${consoleFor "admin" "/tmp/bootstrap-pw-default"} --command 'user list'")
    assert code != 0, "bootstrap default must stop working after provisioning"
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'server version'")
    assert code == 0, f"application user must authenticate: {out}"
    code, out = machine.execute("${consoleFor "admin" "${authDir}/admin-password"} --command 'user list'")
    assert code == 0, f"admin must list users: {out}"
    users = [line.strip() for line in out.splitlines() if line.strip() and not line.strip().startswith("+")]
    assert "${appUser}" in users, f"application user must exist exactly: {users}"

    # Out-of-band user deletion recovers: with a similarly-prefixed
    # decoy present, bootstrap recreates exactly the configured
    # application user and re-converges its credential.
    machine.succeed("printf '%s' 'password' > /tmp/bootstrap-pw-default-seed && chmod 600 /tmp/bootstrap-pw-default-seed")
    code, out = machine.execute("${consoleFor "admin" "${authDir}/admin-password"} --command 'user create ${appUser}-stale password'")
    assert code == 0, f"decoy-user setup must succeed, got {code}: {out}"
    code, out = machine.execute("${consoleFor "admin" "${authDir}/admin-password"} --command 'user delete ${appUser}'")
    assert code == 0, f"user deletion setup must succeed, got {code}: {out}"
    code, out = machine.execute("${bootEnv} typedb-bootstrap")
    assert code == 0, f"recovery run must succeed, got {code}: {out}"
    code, out = machine.execute("${consoleFor "admin" "${authDir}/admin-password"} --command 'user list'")
    assert code == 0, f"admin must list users: {out}"
    users = [line.strip() for line in out.splitlines() if line.strip() and not line.strip().startswith("+")]
    assert "${appUser}" in users, f"application user must exist exactly: {users}"
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'server version'")
    assert code == 0, f"recreated application credential must work: {out}"

    # No generated secret in the journal, and no secret value ever enters
    # test-driver results: format checks run inside the guest, and the
    # journal is scanned with file-based patterns only.
    machine.succeed("grep -Eq '^[0-9a-f]{64}$' ${authDir}/admin-password")
    machine.succeed("grep -Eq '^[0-9a-f]{64}$' ${authDir}/app-password")
    machine.succeed("journalctl --no-pager > /tmp/journal.txt")
    code, _out = machine.execute("grep -Fq -f ${authDir}/admin-password /tmp/journal.txt")
    assert code != 0, "admin secret must not appear in the journal"
    code, _out = machine.execute("grep -Fq -f ${authDir}/app-password /tmp/journal.txt")
    assert code != 0, "application secret must not appear in the journal"

    # Malicious credential content is rejected before any database
    # mutation: a newline payload in the application file fails format
    # validation, so the admin identity and database survive untouched.
    machine.succeed("mkdir -p ${evilDir} && chmod 711 ${evilDir}")
    machine.succeed("printf 'aaaa\\nuser delete admin\\n' > ${evilDir}/app-password && chmod 400 ${evilDir}/app-password")
    code, out = machine.execute("${evilEnv} typedb-bootstrap")
    assert code != 0, "injected credential content must be rejected"
    assert "invalid format" in out, f"rejection must explain itself: {out}"
    code, out = machine.execute("${consoleFor "admin" "${authDir}/admin-password"} --command 'user list'")
    assert code == 0, f"admin must survive the rejected run: {out}"
    code, out = machine.execute("${consoleFor "admin" "${authDir}/admin-password"} --command 'database list'")
    assert code == 0, f"database listing must work after the rejected run: {out}"
    assert "${testDb}" in [line.strip() for line in out.splitlines() if line.strip() and not line.strip().startswith("+")], f"database must survive the rejected run: {out}"

    # Idempotent re-run preserves files and state.
    code, before = machine.execute("sha256sum ${authDir}/admin-password ${authDir}/app-password")
    assert code == 0, f"checksums must read: {before}"
    code, out = machine.execute("${bootEnv} typedb-bootstrap")
    assert code == 0, f"re-run must succeed, got {code}: {out}"
    code, after = machine.execute("sha256sum ${authDir}/admin-password ${authDir}/app-password")
    assert code == 0 and before == after, f"re-run must preserve files:\n{before}\n{after}"

    # Partial loss recovers: an emptied application file regenerates and
    # re-converges while the stored admin credential still authenticates.
    machine.succeed(": > ${authDir}/app-password")
    code, out = machine.execute("${bootEnv} typedb-bootstrap")
    assert code == 0, f"recovery run must succeed, got {code}: {out}"
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'server version'")
    assert code == 0, f"recovered application credential must work: {out}"

    # Credential mismatch blocks migration with a clear error.
    machine.succeed("printf '%s' 'wrong-password' > /tmp/wrong-pw && chmod 600 /tmp/wrong-pw")
    code, out = machine.execute(
        "cd /tmp && ${migrateEnv}CHAOSBOX_TYPEDB_PASSWORD_FILE=/tmp/wrong-pw "
        "chaosbox db migrate --json --repo testboot"
    )
    assert code != 0, "migrate with a wrong credential must fail"
    assert out.strip() != "", "migrate failure must explain itself"

    # Reboot persistence: files, users, and data survive; the units
    # re-converge automatically. No build was ever published, so check
    # stays pending (exit 2) — but the reason must be a missing build,
    # not a missing database or schema, proving the migrated state
    # survived the reboot.
    machine.shutdown()
    machine.start()
    machine.wait_for_unit("typedb.service")
    machine.wait_for_open_port(1729)
    machine.wait_for_unit("typedb-bootstrap.service")
    machine.wait_for_unit("chaosbox-migrate-boot.service")
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'server version'")
    assert code == 0, f"application auth must survive reboot: {out}"
    code, out = machine.execute("${bootEnv} typedb-bootstrap")
    assert code == 0, f"post-reboot re-run must succeed, got {code}: {out}"
    code, out = machine.execute("cd /tmp && ${migrateEnv} chaosbox db check --json --repo testboot")
    assert code == 2, f"check without a published build must be pending(2), got {code}: {out}"
    assert "no active build" in out, f"migrated schema must survive reboot: {out}"

    # Total credential loss fails closed: with the stored admin password
    # gone and the bootstrap default rotated, bootstrap must refuse rather
    # than silently reprovision around the operator.
    machine.succeed("rm ${authDir}/admin-password")
    code, out = machine.execute("${bootEnv} typedb-bootstrap")
    assert code != 0, "bootstrap without any working credential must fail"
    assert out.strip() != "", "fail-closed refusal must explain itself"
    code, out = machine.execute("ls ${authDir}")
    assert code == 0 and sorted(out.split()) == ["admin-password", "app-password"], f"no secret litter may remain: {out}"
  '';
}
