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
    adopt::{AdoptOptions, adopt},
    campaign::Campaign,
    tool::{exec_node, resolve_tool, tool_root},
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
    /// Register a store as a campaign source.
    Adopt {
        /// Campaign root holding `adoption/`; defaults to
        /// `$CHAOSBOX_SESSION_CAMPAIGN`.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Stable key the store is adopted under.
        #[arg(long, value_parser = ["v1", "canary", "staging", "target"])]
        name: String,
        /// Absolute path to the store.
        #[arg(long)]
        db: PathBuf,
        /// Admit a store that still has open file descriptors, recorded as
        /// `held: true` and excluded from count derivation.
        #[arg(long, default_value_t = false)]
        allow_held: bool,
    },
    /// Publish the staged destination to its install target.
    Install {
        /// Installer arguments, forwarded to the pinned script unchanged.
        #[command(flatten)]
        args: InstallArgs,
    },
    /// Reverse an install: store, package, and config together.
    Rollback {
        /// Rollback arguments, forwarded to the pinned script unchanged.
        #[command(flatten)]
        args: RollbackArgs,
    },
}

/// Arguments forwarded to the pinned installer unchanged. The wrapper
/// resolves the installer, verifies its digest, and forwards the exit code.
///
/// `--dry-run` never reaches the script: the installer has no dry-run mode,
/// so the wrapper prints what it would execute and stops.
#[derive(Debug, clap::Args)]
pub struct InstallArgs {
    /// Campaign root holding `tools.json`; defaults to
    /// `$CHAOSBOX_SESSION_CAMPAIGN`.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Directory holding the store being replaced.
    #[arg(long)]
    pub dir: PathBuf,
    /// Staged database to publish.
    #[arg(long)]
    pub source: PathBuf,
    /// Installer state file (restart authority).
    #[arg(long)]
    pub state: PathBuf,
    /// Expected session count; a resume supplying a different value is
    /// refused rather than silently adopted.
    #[arg(long)]
    pub expect_sessions: Option<i64>,
    /// Expected message count.
    #[arg(long)]
    pub expect_messages: Option<i64>,
    /// Expected `user_version`.
    #[arg(long)]
    pub expect_user_version: Option<i64>,
    /// Resume a state left mid-flight by a failure or a crash.
    #[arg(long, default_value_t = false)]
    pub resume: bool,
    /// Stop early after a phase, marking the state interrupted.
    #[arg(long)]
    pub stop_after: Option<String>,
    /// Print the installer and arguments without executing anything.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

/// Arguments forwarded to the pinned rollback script unchanged, including
/// `--dry-run`, which the script implements itself by walking every phase
/// without touching anything.
#[derive(Debug, clap::Args)]
pub struct RollbackArgs {
    /// Campaign root holding `tools.json`; defaults to
    /// `$CHAOSBOX_SESSION_CAMPAIGN`.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Installer state file of the install being reversed.
    #[arg(long)]
    pub state: PathBuf,
    /// Rollback record to write.
    #[arg(long)]
    pub record: PathBuf,
    /// Live-generation record the probe refreshes.
    #[arg(long)]
    pub config_live: PathBuf,
    /// Pre-cutover generation record the restore is asserted against.
    #[arg(long)]
    pub config_baseline: PathBuf,
    /// Single Home Manager command moving package and config back together.
    #[arg(long)]
    pub config_restore: String,
    /// Walk every phase, recording what would happen, touching nothing.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
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
        Command::Adopt {
            root,
            name,
            db,
            allow_held,
        } => {
            let record = adopt(&AdoptOptions {
                root,
                name,
                db,
                allow_held,
            })
            .map_err(|error| error.to_string())?;
            println!("{record}");
            Ok(0)
        }
        Command::Install { args } => run_install(args),
        Command::Rollback { args } => run_rollback(args),
    }
}

/// Publish through the pinned installer, forwarding its exit code.
///
/// # Errors
///
/// Returns a message when the campaign root or the installer cannot be
/// resolved, or when the interpreter cannot be started.
fn run_install(args: InstallArgs) -> Result<i32, String> {
    let root = tool_root(args.root)?;
    let installer = resolve_tool(&root, "install.mjs")?;
    let mut forwarded = vec![
        "--dir".to_string(),
        args.dir.display().to_string(),
        "--source".to_string(),
        args.source.display().to_string(),
        "--state".to_string(),
        args.state.display().to_string(),
    ];
    for (flag, value) in [
        ("--expect-sessions", args.expect_sessions),
        ("--expect-messages", args.expect_messages),
        ("--expect-user-version", args.expect_user_version),
    ] {
        if let Some(value) = value {
            forwarded.push(flag.to_string());
            forwarded.push(value.to_string());
        }
    }
    if args.resume {
        forwarded.push("--resume".to_string());
    }
    if let Some(phase) = args.stop_after {
        forwarded.push("--stop-after".to_string());
        forwarded.push(phase);
    }
    if args.dry_run {
        println!(
            "{}",
            json!({
                "installer": installer.display().to_string(),
                "args": forwarded,
                "state": args.state.display().to_string(),
            })
        );
        return Ok(0);
    }
    exec_node(&installer, &forwarded)
}

/// Reverse through the pinned rollback script, forwarding its exit code.
///
/// # Errors
///
/// Returns a message when the campaign root or the script cannot be
/// resolved, or when the interpreter cannot be started.
fn run_rollback(args: RollbackArgs) -> Result<i32, String> {
    let root = tool_root(args.root)?;
    let rollback = resolve_tool(&root, "rollback.mjs")?;
    let mut forwarded = vec![
        "--state".to_string(),
        args.state.display().to_string(),
        "--record".to_string(),
        args.record.display().to_string(),
        "--config-live".to_string(),
        args.config_live.display().to_string(),
        "--config-baseline".to_string(),
        args.config_baseline.display().to_string(),
        "--config-restore".to_string(),
        args.config_restore,
    ];
    if args.dry_run {
        forwarded.push("--dry-run".to_string());
    }
    exec_node(&rollback, &forwarded)
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
