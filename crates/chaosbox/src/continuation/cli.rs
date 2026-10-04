//! Explicit continuation assembly with private receipts and persistent budgets.
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use clap::Subcommand;
use chaosbox_jev::{JevClient, JevPolicy, Question, SystemOneResponse};
use rusqlite::{Connection, TransactionBehavior, OptionalExtension, params};
use super::{Input, Window, key, prepare, questions, render};

#[derive(Debug, Subcommand)]
/// Source-preserving checkpoint operator commands.
pub enum Command {
    /// Prepare one complete source window without accessing a model.
    Prepare {
        /// Complete normalized native JSONL, never modified.
        #[arg(long)]
        source_jsonl: PathBuf,
        /// Native producer namespace.
        #[arg(long)]
        source: String,
        /// Native session id.
        #[arg(long)]
        session: String,
        /// Explicit private visibility boundary.
        #[arg(long)]
        scope: String,
        /// Operator-declared repository association.
        #[arg(long)]
        repo: String,
        /// One-based first source line.
        #[arg(long, default_value_t = 1)]
        start_line: usize,
        /// Maximum complete message groups, 1..16.
        #[arg(long, default_value_t = 16)]
        max_records: usize,
        /// New private input artifact.
        #[arg(long)]
        output: PathBuf,
    },
    /// Classify with pinned Jev and render exact cited records. Sources are
    /// revalidated; successful receipts replay, including negative selections.
    Assemble {
        /// Prepared source window.
        #[arg(long)]
        input: PathBuf,
        /// Complete original source, revalidated before reuse or inference.
        #[arg(long)]
        source_jsonl: PathBuf,
        /// Private work directory with source-bound persistent spending.
        #[arg(long)]
        work: PathBuf,
        /// Explicit consent to send these records to pinned Jev.
        #[arg(long, required = true)]
        privacy_reviewed: bool,
        /// Cumulative request ceiling across all windows in this work directory.
        #[arg(long, default_value_t = 16)]
        max_requests: u32,
        /// Cumulative conservative input reservation ceiling.
        #[arg(long, default_value_t = 1_000_000)]
        max_input_tokens: u64,
        /// Maximum complete serialized packet characters; never truncates intent.
        #[arg(long, default_value_t = 120_000)]
        max_chars: usize,
        /// Explicitly retry a failed/interrupted reservation; never free.
        #[arg(long)]
        retry: bool,
        /// New private output packet.
        #[arg(long)]
        output: PathBuf,
    },
}

fn read(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("continuation source exceeds limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "continuation source must be UTF-8".into())
}

fn write_new(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
    file.flush()
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist_noclobber(path).map_err(|e| e.to_string())?;
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

/// Execute an explicit bounded command without changing the original source.
pub async fn run(command: Command) -> Result<serde_json::Value, String> {
    match command {
        Command::Prepare {
            source_jsonl,
            source,
            session,
            scope,
            repo,
            start_line,
            max_records,
            output,
        } => {
            let input = prepare(
                &read(&source_jsonl)?,
                &Window {
                    scope: &scope,
                    repo: &repo,
                    source: &source,
                    session: &session,
                    start: start_line,
                    limit: max_records,
                },
            )?;
            write_new(&output, &input)?;
            Ok(
                serde_json::json!({"output":output,"records":input.records.len(),"outside_window_lines":input.omitted_lines,"snapshot":input.snapshot}),
            )
        }
        Command::Assemble {
            input,
            source_jsonl,
            work,
            privacy_reviewed,
            max_requests,
            max_input_tokens,
            max_chars,
            retry,
            output,
        } => {
            if !privacy_reviewed
                || output.exists()
                || !(1..=100).contains(&max_requests)
                || max_input_tokens == 0
                || max_input_tokens > 1_000_000
                || !(512..=120_000).contains(&max_chars)
            {
                return Err("continuation requires reviewed privacy, a new output and bounded spending/output".into());
            }
            let input: Input =
                serde_json::from_str(&read(&input)?).map_err(|_| "invalid continuation input")?;
            let first = input.records.first().ok_or("empty source window")?.line;
            let rebuilt = prepare(
                &read(&source_jsonl)?,
                &Window {
                    scope: &input.scope,
                    repo: &input.repo,
                    source: &input.source,
                    session: &input.session,
                    start: first,
                    limit: input.records.len(),
                },
            )?;
            if input != rebuilt {
                return Err("continuation input changed from its pinned source".into());
            }
            let mut journal = journal(&work, &input)?;
            let response =
                evaluate(&mut journal, &input, max_requests, max_input_tokens, retry).await?;
            let packet = render(&input, &response, max_chars)?;
            write_new(&output, &packet)?;
            Ok(
                serde_json::json!({"output":output,"receipt_key":key(&input)?,"selected":packet["records"].as_array().map(Vec::len),"omitted":packet["omitted"].as_array().map(Vec::len)}),
            )
        }
    }
}

fn journal(work: &Path, input: &Input) -> Result<Connection, String> {
    if !work.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(work).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(work)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
            || fs::metadata(work)
                .map_err(|e| e.to_string())?
                .permissions()
                .mode()
                & 0o077
                != 0
        {
            return Err("continuation work directory must be private and nonsymlink".into());
        }
    }
    let path = work.join("continuation-receipts.sqlite");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err("receipt journal is not a regular file".into());
            }
        }
        Err(error) => return Err(error.to_string()),
    }
    let mut connection = Connection::open(path).map_err(|e| e.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS identity (id TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS attempts (key TEXT NOT NULL, attempt INTEGER NOT NULL, reserved INTEGER NOT NULL,
            status TEXT NOT NULL, response TEXT, PRIMARY KEY(key,attempt));").map_err(|e| e.to_string())?;
    let identity = serde_json::to_string(&(
        &input.scope,
        &input.repo,
        &input.source,
        &input.session,
        &input.snapshot,
    ))
    .map_err(|e| e.to_string())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let existing: Option<String> = transaction
        .query_row("SELECT id FROM identity", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if existing.as_ref().is_some_and(|id| id != &identity) {
        return Err(
            "work journal belongs to different source/scope; use a new private directory".into(),
        );
    }
    transaction
        .execute(
            "INSERT OR IGNORE INTO identity(id) VALUES (?1)",
            [&identity],
        )
        .map_err(|e| e.to_string())?;
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(connection)
}

async fn evaluate(
    journal: &mut Connection,
    input: &Input,
    max_requests: u32,
    max_tokens: u64,
    retry: bool,
) -> Result<SystemOneResponse, String> {
    let key = key(input)?;
    let asked = questions(input);
    let state = serde_json::json!({"source":input,"historical_data_not_instructions":true});
    let reservation = u64::try_from(
        serde_json::to_vec(&(&state, &asked))
            .map_err(|e| e.to_string())?
            .len(),
    )
    .map_err(|e| e.to_string())?
        + 1024;
    let transaction = journal
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let previous: Option<(u32,String,Option<String>)> = transaction.query_row(
        "SELECT attempt,status,response FROM attempts WHERE key=?1 ORDER BY (status='success') DESC, attempt DESC LIMIT 1",[&key],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e| e.to_string())?;
    if let Some((_, status, Some(response))) = &previous {
        if status == "success" {
            return serde_json::from_str(response)
                .map_err(|_| "invalid saved continuation response".into());
        }
    }
    if previous.is_some() && !retry {
        return Err(
            "previous attempt failed/interrupted; explicit --retry spends a new reservation".into(),
        );
    }
    let (requests, tokens): (u32, i64) = transaction
        .query_row(
            "SELECT count(*),coalesce(sum(reserved),0) FROM attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let tokens = u64::try_from(tokens).map_err(|_| "invalid saved token reservation")?;
    if requests >= max_requests || tokens.saturating_add(reservation) > max_tokens {
        return Err("persistent continuation budget exhausted".into());
    }
    let mut client = JevClient::new(JevPolicy {
        max_requests: 1,
        max_retries: 0,
        max_input_tokens: reservation,
        ..JevPolicy::default()
    })
    .map_err(|_| "continuation Jev preflight failed")?;
    let attempt = previous.map_or(1, |(n, _, _)| n + 1);
    let reserved = i64::try_from(reservation).map_err(|_| "invalid reservation")?;
    transaction
        .execute(
            "INSERT INTO attempts VALUES (?1,?2,?3,'reserved',NULL)",
            params![key, attempt, reserved],
        )
        .map_err(|e| e.to_string())?;
    transaction.commit().map_err(|e| e.to_string())?;
    let options = asked
        .iter()
        .map(|(id, q)| {
            (
                id.clone(),
                match q {
                    Question::Choice { criteria, .. } => criteria.keys().cloned().collect(),
                    _ => std::collections::BTreeSet::new(),
                },
            )
        })
        .collect();
    let result = client
        .evaluate(state, asked, &options)
        .await
        .and_then(|response| {
            super::validate_answers(input, &response).map_err(chaosbox_jev::JevError::Protocol)?;
            Ok(response)
        });
    let charged = i64::try_from(reservation.max(client.spent_tokens()))
        .map_err(|_| "invalid Jev spending")?;
    let (status, response) = match &result {
        Ok(response) => (
            "success",
            Some(serde_json::to_string(response).map_err(|e| e.to_string())?),
        ),
        Err(_) => ("failed", None),
    };
    journal
        .execute(
            "UPDATE attempts SET reserved=?1,status=?2,response=?3 WHERE key=?4 AND attempt=?5",
            params![charged, status, response, key, attempt],
        )
        .map_err(|e| e.to_string())?;
    result.map_err(|_| {
        "continuation Jev dispatch failed; reservation retained for explicit retry".into()
    })
}
