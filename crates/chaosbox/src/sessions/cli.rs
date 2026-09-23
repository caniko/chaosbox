//! The `chaosbox sessions` command surface.
//!
//! Both subcommands are read-only: `status` reports what a campaign currently
//! claims, and `verify` recomputes the digests behind those claims. Neither
//! opens a database for writing, so they are safe to run against a campaign
//! that is still being built or that has already been frozen for handoff.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::sessions::{
    campaign::Campaign,
    verify::{verify, VerifyOptions},
};

/// `chaosbox sessions` subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Report campaign progress, journals, and destination integrity.
    Status {
        /// Campaign staging root; defaults to `$CHAOSBOX_SESSION_CAMPAIGN`.
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Recompute effective receipt digests against the destination database.
    Verify {
        /// Campaign staging root; defaults to `$CHAOSBOX_SESSION_CAMPAIGN`.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Bound the pass to this many sessions; `0` checks every receipt.
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// Restrict the pass to these session ids.
        #[arg(long = "session")]
        sessions: Vec<String>,
        /// Also recompute source and recovery digests against the snapshots.
        #[arg(long, default_value_t = false)]
        sources: bool,
        /// Exit `0` when a bounded pass finds no failures.
        #[arg(long, default_value_t = false)]
        allow_partial: bool,
    },
}

/// Run one subcommand and return the process exit code.
///
/// # Errors
///
/// Returns a message when the campaign cannot be read or a digest cannot be
/// recomputed; in that case no report was produced and the code is `1`.
pub fn run(command: Command) -> Result<i32, String> {
    match command {
        Command::Status { root } => {
            let report = status(root)?;
            println!("{report}");
            Ok(0)
        }
        Command::Verify {
            root,
            limit,
            sessions,
            sources,
            allow_partial,
        } => {
            let campaign = Campaign::open(root).map_err(|error| error.to_string())?;
            let options = VerifyOptions {
                limit,
                sessions,
                sources,
                allow_partial,
            };
            let report = verify(&campaign, &options).map_err(|error| error.to_string())?;
            println!("{}", json!(report));
            Ok(i32::from(!report.succeeded(allow_partial)))
        }
    }
}

/// Build the read-only status report for a campaign.
fn status(root: Option<PathBuf>) -> Result<Value, String> {
    let campaign = Campaign::open(root).map_err(|error| error.to_string())?;
    let progress = campaign.progress().map_err(|error| error.to_string())?;
    let identities = campaign
        .identities()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|identity| {
            json!({
                "journal": identity.journal,
                "file": identity.file,
                "body": identity.body,
            })
        })
        .collect::<Vec<Value>>();

    let effective = campaign
        .effective_receipts()
        .map_err(|error| error.to_string())?;
    let supersessions: usize = effective.iter().map(|entry| entry.depth - 1).sum();

    let ready_for_cutover = progress.get("readyForCutover").and_then(Value::as_bool);

    Ok(json!({
        "root": campaign.root().display().to_string(),
        "progress": progress,
        "identities": identities,
        "receipts": {
            "sessions": effective.len(),
            "supersessions": supersessions,
        },
        "destination": {
            "path": campaign.destination().display().to_string(),
            "integrity": integrity(&campaign)?,
        },
        "readyForCutover": ready_for_cutover,
    }))
}

/// Counts and integrity results for the destination database.
fn integrity(campaign: &Campaign) -> Result<Value, String> {
    let connection = open_read_only(&campaign.destination())?;
    let mut counts = serde_json::Map::new();
    for table in [
        "session_v2",
        "session_message",
        "project_directory",
        "worktree",
    ] {
        let sql = format!("SELECT count(*) FROM {table}");
        let count = connection
            .prepare(&sql)
            .and_then(|mut statement| statement.query_row([], |row| row.get::<_, i64>(0)))
            .map_err(|error| format!("counting {table}: {error}"))?;
        counts.insert(table.to_string(), json!(count));
    }

    let quick_check = connection
        .prepare("PRAGMA quick_check")
        .and_then(|mut statement| statement.query_row([], |row| row.get::<_, String>(0)))
        .map_err(|error| format!("quick_check: {error}"))?;
    let violations = foreign_key_violations(&connection)?;

    Ok(json!({
        "counts": Value::Object(counts),
        "quickCheck": quick_check,
        "foreignKeyViolations": violations,
    }))
}

/// Rows returned by `PRAGMA foreign_key_check`.
fn foreign_key_violations(connection: &rusqlite::Connection) -> Result<usize, String> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|error| format!("foreign_key_check: {error}"))?;
    let mut rows = statement
        .query([])
        .map_err(|error| format!("foreign_key_check: {error}"))?;
    let mut violations = 0_usize;
    while rows
        .next()
        .map_err(|error| format!("foreign_key_check: {error}"))?
        .is_some()
    {
        violations += 1;
    }
    Ok(violations)
}

/// Open a database read-only so a status report cannot perturb a campaign.
fn open_read_only(path: &std::path::Path) -> Result<rusqlite::Connection, String> {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))
}
