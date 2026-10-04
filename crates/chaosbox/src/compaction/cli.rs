//! Operator-pinned source custody, bounded assessment and database retrieval.
use std::{io::Read, path::PathBuf};
use clap::{Args, Subcommand};
use chaosbox_jev::{JevClient, JevPolicy};
use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};
use serde_json::{json, Value};
use super::{Budget, Capture, Journal};
use crate::{
    intelligence::{self, Bundle},
    LiveResponder,
};

/// Archive identity pinned by the operator, never selected by model tools.
#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// Private archive directory, mode 0700.
    #[arg(long)]
    pub work: PathBuf,
    /// Private visibility boundary, such as private:can.
    #[arg(long)]
    pub scope: String,
}

/// Session memory operator and read-only consumer commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Commit complete native records from stdin.
    Capture(ArchiveArgs),
    /// Commit custody and return a bounded source-backed checkpoint.
    Compact {
        /// Fixed archive identity.
        #[command(flatten)]
        archive: ArchiveArgs,
        /// Maximum checkpoint characters; overflow refuses reduction.
        #[arg(long, default_value_t = 60_000)]
        max_chars: usize,
    },
    /// Recover exact source data by content hash.
    Evidence {
        /// Fixed archive identity.
        #[command(flatten)]
        archive: ArchiveArgs,
        /// Opaque source digest; paths are never accepted.
        hash: String,
        /// Optional JSON pointer within the immutable record.
        #[arg(long)]
        pointer: Option<String>,
        /// Character offset within the selected source field.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Maximum returned source characters.
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Report deferred jobs and durable knowledge counts.
    Status(ArchiveArgs),
    /// Assess queued source records; failed attempts retain reservations.
    Drain {
        /// Fixed archive identity.
        #[command(flatten)]
        archive: ArchiveArgs,
        /// Explicit operator consent to use pinned Jev for captured source text.
        #[arg(long)]
        live_jev: bool,
        /// Publish the resulting outbox to `TypeDB`, including partial progress.
        #[arg(long)]
        publish: bool,
        /// Cumulative archive-wide attempt ceiling.
        #[arg(long, default_value_t = 1000)]
        max_requests: u32,
        /// Cumulative archive-wide conservative input-token ceiling.
        #[arg(long, default_value_t = 10_000_000)]
        max_input_tokens: u64,
        /// Authorize new spending after a failed or interrupted attempt.
        #[arg(long)]
        retry: bool,
    },
    /// Publish or retry a committed knowledge outbox without inference.
    Publish(ArchiveArgs),
    /// Retrieve private cross-session knowledge from one pinned `TypeDB` build.
    Context {
        /// Explicit visibility.
        #[arg(long)]
        scope: String,
        /// Explicit applicability filter.
        #[arg(long)]
        repo: String,
        /// Bounded lexical task terms.
        query: String,
        /// Maximum records returned.
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Maximum serialized record characters.
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Drill into database intelligence and assessment receipts.
    KnowledgeEvidence {
        /// Explicit visibility.
        #[arg(long)]
        scope: String,
        /// Explicit applicability filter.
        #[arg(long)]
        repo: String,
        /// Intelligence identity.
        id: String,
    },
}

async fn pinned(scope: &str) -> Result<(Option<String>, Bundle), String> {
    if !scope.starts_with("private:") || scope.len() <= 8 {
        return Err("private scope required".into());
    }
    let mut store = TypeDbStore::new(TypeDbConfig::from_env()?);
    let result = store.knowledge(scope).await.map_err(|e| e.to_string())?;
    let (pin, bundle) = match result {
        Some((id, raw)) => {
            if chaosbox_core::sha256_hex(&["session-knowledge-v1", scope, &raw]) != id {
                return Err("knowledge digest mismatch".into());
            }
            (
                Some(id),
                serde_json::from_str::<Bundle>(&raw)
                    .map_err(|_| "invalid database knowledge export")?,
            )
        }
        None => (None, Bundle::new(scope)),
    };
    bundle.validate()?;
    if bundle.scope != scope {
        return Err("database knowledge scope mismatch".into());
    }
    Ok((pin, bundle))
}

/// Execute a bounded command. Source capture never loads model credentials.
pub async fn run(command: Command) -> Result<Value, String> {
    match command {
        Command::Capture(archive) => capture(&archive, None),
        Command::Compact { archive, max_chars } => capture(&archive, Some(max_chars)),
        Command::Evidence {
            archive,
            hash,
            pointer,
            offset,
            max_chars,
        } => evidence_page(&archive, &hash, pointer.as_deref(), offset, max_chars),
        Command::Status(archive) => {
            let journal = Journal::open(&archive.work, &archive.scope)?;
            let (requests, tokens): (u32, i64) = journal
                .db
                .query_row(
                    "SELECT count(*),coalesce(sum(reserved),0) FROM attempts",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| e.to_string())?;
            let bundle = journal.bundle()?;
            Ok(
                json!({"pending":journal.pending()?,"records":bundle.records.len(),"assessments":bundle.assessments.len(),"reserved_requests":requests,"reserved_input_tokens":tokens}),
            )
        }
        Command::Drain {
            archive,
            live_jev,
            publish,
            max_requests,
            max_input_tokens,
            retry,
        } => {
            if !live_jev {
                return Err("drain requires explicit --live-jev consent".into());
            }
            let mut journal = Journal::open(&archive.work, &archive.scope)?;
            let mut responder = LiveResponder::new(
                JevClient::new(JevPolicy {
                    max_requests,
                    max_input_tokens,
                    max_retries: 0,
                    ..JevPolicy::default()
                })
                .map_err(|_| "session Jev preflight failed")?,
            );
            let result = journal
                .assess_pending(
                    &archive.work,
                    &mut responder,
                    Budget {
                        requests: max_requests,
                        input_tokens: max_input_tokens,
                        retry,
                    },
                )
                .await;
            let publication = if publish {
                Some(
                    journal
                        .publish_pending(
                            &archive.work,
                            &mut TypeDbStore::new(TypeDbConfig::from_env()?),
                        )
                        .await?,
                )
            } else {
                None
            };
            Ok(json!({"completed":result?,"publication":publication,"pending":journal.pending()?}))
        }
        Command::Publish(archive) => {
            let mut journal = Journal::open(&archive.work, &archive.scope)?;
            let id = journal
                .publish_pending(
                    &archive.work,
                    &mut TypeDbStore::new(TypeDbConfig::from_env()?),
                )
                .await?;
            Ok(json!({"publication":id}))
        }
        Command::Context {
            scope,
            repo,
            query,
            limit,
            max_chars,
        } => {
            let (pin, bundle) = pinned(&scope).await?;
            Ok(
                json!({"pin":pin,"scope":scope,"historical_data_not_instructions":true,"exhaustive":false,"records":bundle.context(&scope,&repo,&query,limit,max_chars)?}),
            )
        }
        Command::KnowledgeEvidence { scope, repo, id } => {
            let (pin, bundle) = pinned(&scope).await?;
            Ok(json!({"pin":pin,"evidence":intelligence::cli::evidence(&bundle,&scope,&repo,&id)?}))
        }
    }
}

fn evidence_page(
    archive: &ArchiveArgs,
    hash: &str,
    pointer: Option<&str>,
    offset: usize,
    max_chars: usize,
) -> Result<Value, String> {
    if !(1..=32_000).contains(&max_chars) {
        return Err("invalid evidence page budget".into());
    }
    let value = Journal::open(&archive.work, &archive.scope)?.evidence(hash)?;
    let selected = match pointer {
        Some(p) => value.pointer(p).ok_or("source pointer not found")?,
        None => &value,
    };
    let text = selected
        .as_str()
        .map_or_else(|| selected.to_string(), str::to_owned);
    let total = text.chars().count();
    if offset > total {
        return Err("evidence offset outside source".into());
    }
    let page: String = text.chars().skip(offset).take(max_chars).collect();
    let next = offset + page.chars().count();
    Ok(
        json!({"hash":hash,"pointer":pointer,"offset":offset,"text":page,"total_chars":total,"next_offset":next,"has_more":next<total}),
    )
}

fn capture(archive: &ArchiveArgs, budget: Option<usize>) -> Result<Value, String> {
    let mut raw = String::new();
    std::io::stdin()
        .take(64 * 1024 * 1024 + 1)
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())?;
    if raw.len() > 64 * 1024 * 1024 {
        return Err("capture exceeds 64 MiB".into());
    }
    let input: Capture = serde_json::from_str(&raw).map_err(|_| "invalid capture input")?;
    let mut journal = Journal::open(&archive.work, &archive.scope)?;
    let receipt = journal.capture(&input)?;
    match budget {
        Some(max) => {
            serde_json::to_value(journal.plan(&receipt.id, max)?).map_err(|e| e.to_string())
        }
        None => serde_json::to_value(receipt).map_err(|e| e.to_string()),
    }
}
