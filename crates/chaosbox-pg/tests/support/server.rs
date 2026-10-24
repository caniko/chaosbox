//! Disposable PostgreSQL with pgvector on a private unix socket: one cluster
//! per test, superuser `fixture_owner`, database `chaosbox`. `PG_BIN` names
//! the directory of a server build that ships pgvector; otherwise
//! `initdb`/`pg_ctl`/`psql` come from `PATH`.
#![allow(dead_code)]

use std::process::Command;

pub const PORT: &str = "55439";

pub struct Server {
    directory: tempfile::TempDir,
}

fn bin(name: &str) -> Command {
    match std::env::var_os("PG_BIN") {
        Some(dir) => Command::new(std::path::Path::new(&dir).join(name)),
        None => Command::new(name),
    }
}

fn run(command: &mut Command) -> String {
    let out = command.output().expect("spawn PostgreSQL tool");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

impl Server {
    pub fn start() -> Self {
        let server = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        let dir = server.directory.path();
        run(bin("initdb").arg("-D").arg(dir.join("data")).args([
            "-N",
            "--auth=trust",
            "--no-locale",
            "-E",
            "UTF8",
            "-U",
            "fixture_owner",
        ]));
        let options = format!(
            "-k {} -p {PORT} -c listen_addresses='' -c fsync=off -c full_page_writes=off",
            dir.display()
        );
        run(bin("pg_ctl")
            .arg("-D")
            .arg(dir.join("data"))
            .arg("-l")
            .arg(dir.join("log"))
            .args(["-o", &options, "-w", "start"]));
        server.psql("postgres", "CREATE DATABASE chaosbox");
        server
    }

    /// Connection string for the test database.
    pub fn url(&self) -> String {
        format!(
            "host={} port={PORT} user=fixture_owner dbname=chaosbox",
            self.directory.path().display()
        )
    }

    /// Run SQL as the superuser and return unaligned tuples-only output.
    pub fn psql(&self, database: &str, sql: &str) -> String {
        run(bin("psql").args([
            "-XAt",
            "--set",
            "ON_ERROR_STOP=1",
            "--host",
            self.directory.path().to_str().unwrap(),
            "--port",
            PORT,
            "--username",
            "fixture_owner",
            "--dbname",
            database,
            "--command",
            sql,
        ]))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = bin("pg_ctl")
            .arg("-D")
            .arg(self.directory.path().join("data"))
            .args(["-m", "immediate", "-w", "stop"])
            .output();
    }
}
