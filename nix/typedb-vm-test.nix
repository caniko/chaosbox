# TypeDB integration test for Chaosbox (migrate+check+query scope).
#
# Boots the packaged TypeDB server in a disposable NixOS guest via the
# generic services.typedb module (binaries substitute from the fleet cache;
# nothing here builds TypeDB), applies Chaosbox's packaged schema, and
# proves the db check/migrate contract matrix plus consumer queries.
# The server arrives as a Nix package, not a container image.
{
  pkgs,
  chaosboxModule,
  harborDbModule,
  typedbModule,
  chaosboxPackage,
  typedbPackage,
  typedbConsolePackage,
  federationTestTools,
}:
let
  testPassword = "password";
  testDb = "vmtest";
in
pkgs.testers.nixosTest {
  name = "chaosbox-typedb";

  nodes.machine = { ... }: {
    imports = [
      harborDbModule
      typedbModule
      chaosboxModule
    ];
    system.stateVersion = "24.11";
    virtualisation.memorySize = 4096;
    virtualisation.cores = 4;

    services.harbor-db.package = pkgs.hello;
    services.typedb.package = typedbPackage;
    services.chaosbox = {
      enable = true;
      package = chaosboxPackage;
      repo = "test";
      database = testDb;
      passwordFile = pkgs.writeText "chaosbox-test-pw" testPassword;
    };

    environment.systemPackages = [
      chaosboxPackage
      typedbConsolePackage
    ];
  };

  testScript = ''
    import json

    machine.wait_for_unit("typedb.service")
    machine.wait_for_open_port(1729)

    ENV = (
        "CHAOSBOX_DB_BACKEND=typedb "
        "CHAOSBOX_TYPEDB_ADDR=127.0.0.1:1729 "
        "CHAOSBOX_TYPEDB_USER=admin "
        "CHAOSBOX_TYPEDB_PASSWORD_FILE=/etc/chaosbox-test-pw "
        "CHAOSBOX_TYPEDB_DATABASE=${testDb} "
    )
    machine.succeed("printf '%s' '${testPassword}' > /etc/chaosbox-test-pw && chmod 600 /etc/chaosbox-test-pw")
    # Execute the exact native regression with an authenticated live server.
    # A missing server/credential/schema is an error; zero tests cannot pass.
    code, out = machine.execute(
        f"{ENV}CHAOSBOX_REQUIRE_TYPEDB=1 ${federationTestTools}/bin/federation-typedb "
        "--ignored --exact federation_typedb_is_read_only_scoped_and_snapshot_bound"
    )
    assert code == 0, f"mandatory TypeDB federation regression failed: {out}"
    assert "1 passed; 0 failed; 0 ignored" in out, f"live regression did not execute: {out}"
    machine.succeed("mkdir -p /tmp/cbtest && cp -r ${../fixtures/demo-repo} /tmp/cbtest/demo-repo && chmod -R u+rw /tmp/cbtest")

    # Pending before any migration has applied.
    code, _out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test")
    assert code == 2, f"pre-migration check must be pending(2), got {code}: {_out}"

    # Migrate exits 0 only after the packaged schema applies.
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db migrate --json --repo test")
    assert code == 0, f"migrate must succeed, got {code}: {out}"

    # Fresh database, no builds published yet: check is pending (not ready).
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test")
    assert code == 2, f"post-migration check must be pending(2), got {code}: {out}"

    # Pipeline publishes a real graph (explicit disposable fixture
    # decisions); check turns ready.
    machine.succeed(f"cd /tmp/cbtest && {ENV} chaosbox run demo-repo --repo test --fixture-decisions > /tmp/graph.json")
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test")
    assert code == 0, f"post-run check must be ready(0), got {code}: {out}"
    assert '"status":"ready"' in out.replace(" ", ""), f"ready JSON expected: {out}"

    # Consumer queries answer from the published build.
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox query search main --repo test")
    assert code == 0, f"search must succeed, got {code}: {out}"
    assert "src/main.rs" in out, f"search must hit fixture symbols: {out}"

    # Repeat publication from a fresh process: touch a source, re-run, and
    # the new build (generation 2) becomes active instead of failing the
    # predecessor guard.
    machine.succeed("printf '// reindex\\n' >> /tmp/cbtest/demo-repo/src/main.rs")
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox run demo-repo --repo test --fixture-decisions")
    assert code == 0, f"re-run must succeed, got {code}: {out}"
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox query status --repo test")
    assert code == 0, f"status must succeed, got {code}: {out}"
    assert '"generation":2' in out.replace(" ", ""), f"re-run must publish generation 2: {out}"

    # Deterministic-only publication retains certified syntax facts, with
    # no new inference or fixture decisions. Verify the exact persisted facts,
    # including their kinds, file scopes, coverage and duplicate count.
    code, _out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox run demo-repo --repo test-nodec --no-decisions > /tmp/graph-nodec.json")
    assert code == 0, f"no-decisions run must succeed, got {code}"
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox query export --repo test-nodec")
    assert code == 0, f"export must succeed, got {code}: {out}"
    graph = json.loads(out)
    nodes = {node["id"]: node for node in graph["nodes"]}
    assert nodes, f"no-decisions build must have nodes: {out}"
    facts = set()
    for link in graph["links"]:
        source, target = nodes[link["source"]], nodes[link["target"]]
        assert source["kind"] == "file", f"syntax fact must originate from a file: {link}"
        assert source["source_file"] == target["source_file"], f"syntax fact must stay in its file: {link}"
        facts.add((link["rel_type"], target["source_file"], target["label"], target["kind"]))
    expected = {
        ("defines", "app.ts", "main", "definition"),
        ("defines", "src/main.rs", "main", "definition"),
        ("defines", "src/main.rs", "helper", "definition"),
        ("contains", "app.ts", "app", "module"),
        ("contains", "src/main.rs", "main", "module"),
    }
    assert facts == expected, f"exact certified syntax facts expected: {out}"
    assert len(graph["links"]) == len(expected), f"duplicate syntax facts: {out}"
    assert graph["coverage"]["structural_relations"] == len(expected), f"structural coverage mismatch: {out}"
    assert graph["coverage"]["decision_relations"] == 0, f"no-decisions build published decision relations: {out}"

    # Idempotent re-apply stays green.
    code, _out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db migrate --json --repo test")
    assert code == 0, f"re-apply must stay green, got {code}"

    # Wrong credentials are an error, never ready/pending; state untouched.
    machine.succeed("printf '%s' 'wrong-pw' > /tmp/bad-pw && chmod 600 /tmp/bad-pw")
    code, _out = machine.execute(
        f"cd /tmp/cbtest && {ENV}CHAOSBOX_TYPEDB_PASSWORD_FILE=/tmp/bad-pw "
        "chaosbox db check --json --repo test"
    )
    assert code != 0, "bad credentials must fail"
    code, _out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test")
    assert code == 0, "good credentials still ready after the negative case"

    # Restart persistence: reboot, same query still answers.
    machine.shutdown()
    machine.start()
    machine.wait_for_unit("typedb.service")
    machine.wait_for_open_port(1729)
    code, out = machine.execute(f"cd /tmp/cbtest && {ENV} chaosbox db check --json --repo test")
    assert code == 0, f"post-reboot check must be ready(0), got {code}: {out}"
  '';
}
