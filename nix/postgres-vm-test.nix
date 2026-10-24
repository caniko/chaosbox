# PostgreSQL integration test for Chaosbox (module + migrate/check/query).
#
# Boots the NixOS PostgreSQL service through the Chaosbox module (pgvector,
# peer-authenticated owner role, postgresql-setup extension creation),
# applies the packaged schema with the packaged CLI, and proves the db
# check/migrate contract matrix, consumer queries, the read-view smoke and
# restart persistence. Backend contract tests run hermetically in the
# `postgres-live` check; this guest proves the deployment wiring.
{
  pkgs,
  chaosboxModule,
  harborDbModule,
  chaosboxPackage,
}:
let
  testDb = "vmtest";
in
pkgs.testers.nixosTest {
  name = "chaosbox-postgres";

  nodes.machine = { ... }: {
    imports = [
      harborDbModule
      chaosboxModule
    ];
    system.stateVersion = "24.11";
    virtualisation.memorySize = 2048;
    virtualisation.cores = 4;

    services.harbor-db.package = pkgs.hello;
    services.chaosbox = {
      enable = true;
      package = chaosboxPackage;
      repo = "test";
      database = testDb;
    };

    environment.systemPackages = [ chaosboxPackage ];
  };

  testScript = ''
    import json

    machine.wait_for_unit("postgresql-setup.service")

    ENV = (
        "CHAOSBOX_DB_BACKEND=postgres "
        "CHAOSBOX_DATABASE_URL='host=/run/postgresql dbname=${testDb}' "
    )
    # Peer authentication: the OS user is the database owner role.
    AS = "runuser -u ${testDb} -- env "
    def cb(command):
        return machine.execute(f"cd /tmp/cbtest && {AS}{ENV}{command}")

    machine.succeed("mkdir -p /tmp/cbtest && cp -r ${../fixtures/demo-repo} /tmp/cbtest/demo-repo")
    machine.succeed("chown -R ${testDb}: /tmp/cbtest && chmod -R u+rw /tmp/cbtest")

    # Pending before any migration has applied.
    code, out = cb("chaosbox db check --json --repo test")
    assert code == 2, f"pre-migration check must be pending(2), got {code}: {out}"

    # Migrate exits 0 only after the packaged schema applies (pgvector was
    # created by postgresql-setup; the owner only verifies it).
    code, out = cb("chaosbox db migrate --json --repo test")
    assert code == 0, f"migrate must succeed, got {code}: {out}"

    # Fresh database, no builds published yet: check is pending (not ready).
    code, out = cb("chaosbox db check --json --repo test")
    assert code == 2, f"post-migration check must be pending(2), got {code}: {out}"

    # Pipeline publishes a real graph (explicit disposable fixture
    # decisions); check turns ready.
    code, out = cb("chaosbox run demo-repo --repo test --fixture-decisions")
    assert code == 0, f"run must succeed, got {code}: {out}"
    code, out = cb("chaosbox db check --json --repo test")
    assert code == 0, f"post-run check must be ready(0), got {code}: {out}"
    assert '"status":"ready"' in out.replace(" ", ""), f"ready JSON expected: {out}"

    # Consumer queries answer from the published build.
    code, out = cb("chaosbox query search main --repo test")
    assert code == 0, f"search must succeed, got {code}: {out}"
    assert "src/main.rs" in out, f"search must hit fixture symbols: {out}"

    # Repeat publication from a fresh process publishes generation 2.
    machine.succeed("printf '// reindex\\n' >> /tmp/cbtest/demo-repo/src/main.rs")
    code, out = cb("chaosbox run demo-repo --repo test --fixture-decisions")
    assert code == 0, f"re-run must succeed, got {code}: {out}"
    code, out = cb("chaosbox query status --repo test")
    assert code == 0, f"status must succeed, got {code}: {out}"
    assert '"generation":2' in out.replace(" ", ""), f"re-run must publish generation 2: {out}"

    # Deterministic-only publication retains exactly the certified syntax facts.
    code, out = cb("chaosbox run demo-repo --repo test-nodec --no-decisions")
    assert code == 0, f"no-decisions run must succeed, got {code}: {out}"
    code, out = cb("chaosbox query export --repo test-nodec")
    assert code == 0, f"export must succeed, got {code}: {out}"
    graph = json.loads(out)
    nodes = {node["id"]: node for node in graph["nodes"]}
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
    assert graph["coverage"]["decision_relations"] == 0, f"no-decisions build published decision relations: {out}"

    # Reader acceptance: historical pinning, newer publication, revocation,
    # call exhaustion and expiry with observable transport output.
    # The smoke rewrites app.ts as ${testDb}, so chown after root creates it.
    machine.succeed("mkdir -p /tmp/cbtest/reader/syntax && printf 'export function before() {}\\nexport const value = 1;\\n' > /tmp/cbtest/reader/syntax/app.ts && chown -R ${testDb}: /tmp/cbtest/reader")
    assert cb("chaosbox run /tmp/cbtest/reader/syntax --repo syntax --no-decisions --max-candidates 0")[0] == 0
    machine.succeed("printf 'export function after() {}\\nexport const value = 2;\\n' > /tmp/cbtest/reader/syntax/app.ts")
    assert cb("chaosbox run /tmp/cbtest/reader/syntax --repo syntax --no-decisions --max-candidates 0")[0] == 0
    assert cb("sh -c 'chaosbox query status --repo syntax > /tmp/cbtest/reader/status.json'")[0] == 0
    code, out = cb("${pkgs.python3}/bin/python3 ${../scripts/tests/read_view_smoke.py} /tmp/cbtest/reader")
    assert code == 0, f"read-view smoke failed: {out}"

    # Idempotent re-apply stays green.
    assert cb("chaosbox db migrate --json --repo test")[0] == 0, "re-apply must stay green"

    # A role the database does not know is an error, never ready/pending.
    code, out = machine.execute(f"cd /tmp && runuser -u nobody -- env {ENV}chaosbox db check --json --repo test")
    assert code == 1, f"foreign role must be an error(1), got {code}: {out}"

    # Restart persistence: reboot, same query still answers.
    machine.shutdown()
    machine.start()
    machine.wait_for_unit("postgresql-setup.service")
    code, out = cb("chaosbox db check --json --repo test")
    assert code == 0, f"post-reboot check must be ready(0), got {code}: {out}"
  '';
}
