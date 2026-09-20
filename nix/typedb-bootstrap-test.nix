# TypeDB bootstrap lifecycle test (credential provisioning scope).
#
# Boots the packaged TypeDB server with the local admin endpoint enabled,
# then drives the `typedb-bootstrap` package through the full lifecycle
# with NON-DEFAULT user/database names: empty install, rotation of the
# bootstrap default, application auth, file permissions, idempotent
# re-run, interruption recovery, credential-mismatch handling, journal
# secrecy, and reboot persistence. Nothing here builds TypeDB.
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
  socketPath = "/var/lib/typedb/data/admin.sock";
  bootEnv =
    "TYPEDB_BOOTSTRAP_AUTH_DIR=${authDir} "
    + "TYPEDB_BOOTSTRAP_SOCKET=${socketPath} "
    + "TYPEDB_BOOTSTRAP_ADDR=127.0.0.1:1729 "
    + "TYPEDB_BOOTSTRAP_ADMIN_BIN=${typedbPackage}/bin/typedb_admin_bin "
    + "TYPEDB_BOOTSTRAP_CONSOLE_BIN=${typedbConsolePackage}/bin/typedb-console "
    + "TYPEDB_BOOTSTRAP_APP_USER=${appUser} "
    + "TYPEDB_BOOTSTRAP_DATABASE=${testDb} "
    + "TYPEDB_BOOTSTRAP_APP_OWNER=testuser ";
  consoleFor =
    user: pwFile:
    "${typedbConsolePackage}/bin/typedb-console --address 127.0.0.1:1729 --tls-disabled --username ${user} --password $(cat ${pwFile})";
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
        extraFlags = [ "--server.admin.enabled=true" ];
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
    };

  testScript = ''
    machine.wait_for_unit("typedb.service")
    machine.wait_for_open_port(1729)

    # Empty install provisions cleanly with non-default names.
    code, out = machine.execute(f"${bootEnv} typedb-bootstrap")
    assert code == 0, f"bootstrap must succeed, got {code}: {out}"

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

    # Bootstrap default no longer authenticates; application does.
    machine.succeed("printf '%s' 'password' > /tmp/bootstrap-pw-default && chmod 600 /tmp/bootstrap-pw-default")
    code, _out = machine.execute("${consoleFor "admin" "/tmp/bootstrap-pw-default"} --command 'user list'")
    assert code != 0, "bootstrap default must stop working after provisioning"
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'user list'")
    assert code == 0, f"application user must authenticate: {out}"
    assert "${appUser}" in out, f"application user must exist: {out}"

    # Chaosbox migrates over the provisioned non-default identity.
    code, out = machine.execute(
        "cd /tmp && CHAOSBOX_DB_BACKEND=typedb CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 "
        + "CHAOSBOX_TYPEDB_USER=${appUser} CHAOSBOX_TYPEDB_PASSWORD_FILE=${authDir}/app-password "
        + "CHAOSBOX_TYPEDB_DATABASE=${testDb} chaosbox db migrate --json --repo testboot"
    )
    assert code == 0, f"migrate must succeed as provisioned user: {out}"

    # No generated secret in the journal (patterns read from files so the
    # values never appear on any command line either).
    admin_pw = machine.succeed("cat ${authDir}/admin-password").strip()
    app_pw = machine.succeed("cat ${authDir}/app-password").strip()
    assert len(admin_pw) == 64 and len(app_pw) == 64, "generated secrets must be 32 hex bytes"
    machine.succeed("journalctl --no-pager > /tmp/journal.txt")
    code, _out = machine.execute("grep -Fq -f ${authDir}/admin-password /tmp/journal.txt")
    assert code != 0, "admin secret must not appear in the journal"
    code, _out = machine.execute("grep -Fq -f ${authDir}/app-password /tmp/journal.txt")
    assert code != 0, "application secret must not appear in the journal"

    # Idempotent re-run preserves files and state.
    code, before = machine.execute("sha256sum ${authDir}/admin-password ${authDir}/app-password")
    assert code == 0, f"checksums must read: {before}"
    code, out = machine.execute(f"${bootEnv} typedb-bootstrap")
    assert code == 0, f"re-run must succeed, got {code}: {out}"
    code, after = machine.execute("sha256sum ${authDir}/admin-password ${authDir}/app-password")
    assert code == 0 and before == after, f"re-run must preserve files:\n{before}\n{after}"

    # Interruption recovery: empty files regenerate and re-converge.
    machine.succeed(": > ${authDir}/app-password && rm ${authDir}/admin-password")
    code, out = machine.execute(f"${bootEnv} typedb-bootstrap")
    assert code == 0, f"recovery run must succeed, got {code}: {out}"
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'user list'")
    assert code == 0, f"recovered application credential must work: {out}"

    # Credential mismatch blocks migration with a clear error.
    machine.succeed("printf '%s' 'wrong-password' > /tmp/wrong-pw && chmod 600 /tmp/wrong-pw")
    code, out = machine.execute(
        "cd /tmp && CHAOSBOX_DB_BACKEND=typedb CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 "
        + "CHAOSBOX_TYPEDB_USER=${appUser} CHAOSBOX_TYPEDB_PASSWORD_FILE=/tmp/wrong-pw "
        + "CHAOSBOX_TYPEDB_DATABASE=${testDb} chaosbox db migrate --json --repo testboot"
    )
    assert code != 0, "migrate with a wrong credential must fail"
    assert out.strip() != "", "migrate failure must explain itself"

    # Reboot persistence: files, users, and data survive; re-run converges.
    machine.shutdown()
    machine.start()
    machine.wait_for_unit("typedb.service")
    machine.wait_for_open_port(1729)
    code, out = machine.execute("${consoleFor appUser "${authDir}/app-password"} --command 'user list'")
    assert code == 0, f"application auth must survive reboot: {out}"
    code, out = machine.execute(f"${bootEnv} typedb-bootstrap")
    assert code == 0, f"post-reboot re-run must succeed, got {code}: {out}"
  '';
}
