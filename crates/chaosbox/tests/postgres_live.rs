//! Disposable PostgreSQL capture using a genuinely SELECT-only login.
use std::process::{Command, Output};

struct Server {
    directory: tempfile::TempDir,
}
impl Server {
    fn start() -> Self {
        let server = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        let data = server.directory.path().join("data");
        let initialized = Command::new("initdb")
            .args(["-D"])
            .arg(&data)
            .args(["--auth=trust", "--no-locale", "-U", "fixture_owner"])
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        let options = format!(
            "-k {} -p 55439 -c listen_addresses=''",
            server.directory.path().display()
        );
        let started = Command::new("pg_ctl")
            .args(["-D"])
            .arg(&data)
            .arg("-l")
            .arg(server.directory.path().join("log"))
            .args(["-o", &options, "-w", "start"])
            .output()
            .unwrap();
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        server
    }
    fn psql(&self, role: &str, sql: &str) -> Output {
        Command::new("psql")
            .args([
                "-XAt",
                "--set",
                "ON_ERROR_STOP=1",
                "--host",
                self.directory.path().to_str().unwrap(),
                "--port",
                "55439",
                "--username",
                role,
                "--dbname",
                "postgres",
                "--command",
                sql,
            ])
            .output()
            .unwrap()
    }
    fn capture(&self, schema: &str, path: &std::path::Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_chaosbox"))
            .args([
                "postgres",
                "capture",
                "--database",
                "postgres",
                "--schema",
                schema,
                "--output",
            ])
            .arg(path)
            .env("PGHOST", self.directory.path())
            .env("PGPORT", "55439")
            .env("PGUSER", "catalog_reader")
            .output()
            .unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("pg_ctl")
            .args(["-D"])
            .arg(self.directory.path().join("data"))
            .args(["-m", "fast", "-w", "stop"])
            .output();
    }
}

#[test]
#[ignore = "requires installed PostgreSQL initdb/pg_ctl/psql; isolated Unix socket"]
fn select_only_catalog_capture_refresh_and_scope() {
    let server = Server::start();
    let setup = server.psql(
        "fixture_owner",
        r"
      CREATE ROLE catalog_reader LOGIN;
      CREATE TABLE parent (id integer PRIMARY KEY, label text NOT NULL DEFAULT 'original');
      CREATE TABLE child (id integer PRIMARY KEY, parent_id integer REFERENCES parent(id));
      CREATE VIEW child_view AS SELECT id, parent_id FROM child;
      CREATE FUNCTION public.echo(value integer) RETURNS integer LANGUAGE sql AS 'SELECT value';
      GRANT USAGE ON SCHEMA public TO catalog_reader;
      GRANT SELECT ON ALL TABLES IN SCHEMA public TO catalog_reader;
    ",
    );
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let old_fk = server.psql(
        "catalog_reader",
        "SELECT count(*) FROM information_schema.referential_constraints",
    );
    assert_eq!(String::from_utf8_lossy(&old_fk.stdout).trim(), "0");
    let path = server.directory.path().join("capture.json");
    let captured = server.capture("public", &path);
    assert!(
        captured.status.success(),
        "{}",
        String::from_utf8_lossy(&captured.stderr)
    );
    let capture = chaosbox::postgres::load_capture(&path).unwrap();
    assert_eq!(capture.catalog.role, "catalog_reader");
    assert!(capture
        .catalog
        .records
        .iter()
        .any(|r| r.kind == "constraint" && r.target.is_some() && r.details["type"] == "f"));
    assert!(capture.catalog.records.iter().any(|r| r.kind == "column"
        && r.name == "label"
        && r.details["nullable"] == false
        && r.details["default"].as_str().unwrap().contains("original")));
    assert!(capture
        .catalog
        .records
        .iter()
        .any(|r| r.kind == "view" && r.definition.as_ref().unwrap().contains("child")));
    assert!(capture
        .catalog
        .records
        .iter()
        .any(|r| r.kind == "routine" && r.definition.as_ref().unwrap().contains("SELECT value")));
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o077, 0);
    assert!(!server
        .psql("catalog_reader", "INSERT INTO parent VALUES (1, 'denied')")
        .status
        .success());
    assert!(!server.capture("public", &path).status.success());
    let altered = server.psql("fixture_owner", "ALTER TABLE child ADD COLUMN added bigint");
    assert!(altered.status.success());
    let next = server.directory.path().join("next.json");
    assert!(server.capture("public", &next).status.success());
    let refreshed = chaosbox::postgres::load_capture(&next).unwrap();
    assert_ne!(capture.digest, refreshed.digest);
    assert!(refreshed
        .catalog
        .records
        .iter()
        .any(|r| r.kind == "column" && r.name == "added"));
    assert!(!server
        .capture(
            "absent'; SELECT 1; --",
            &server.directory.path().join("missing.json")
        )
        .status
        .success());
}
