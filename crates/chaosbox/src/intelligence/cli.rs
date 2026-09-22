//! Operator ingestion/assessment and read-only intelligence artifact queries.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use chaosbox_jev::{JevClient, JevPolicy, SystemOneResponse};
use clap::Subcommand;
use serde::de::DeserializeOwned;

use super::{assess, extract_window, questions, Bundle, Candidates};
use crate::{LiveResponder, Responder};

const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;

/// Session intelligence commands. Inference is an explicit operator action.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Extract bounded, verbatim proposals from normalized session JSONL.
    Extract {
        /// Source JSONL, never modified.
        input: PathBuf,
        /// Visibility boundary such as private:can.
        #[arg(long)]
        scope: String,
        /// Producer namespace for native ids.
        #[arg(long)]
        source: String,
        /// Native session id.
        #[arg(long)]
        session: String,
        /// Explicit repository associations, not guessed from directory names.
        #[arg(long = "repo", required = true)]
        repositories: Vec<String>,
        /// Maximum proposals; omitted count remains explicit.
        #[arg(long, default_value_t = 100)]
        max_candidates: usize,
        /// Skip previously processed candidates in the same immutable input.
        #[arg(long, default_value_t = 0)]
        skip_candidates: usize,
        /// New private staging artifact; existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
    /// Assess candidates with pinned Jev and materialize sparse knowledge.
    Assess {
        /// Candidate staging artifact.
        input: PathBuf,
        /// Prior same-scope intelligence, preserved in the next build.
        #[arg(long)]
        previous: Option<PathBuf>,
        /// Explicit permission to call the configured Jev endpoint.
        #[arg(long, required = true)]
        live_jev: bool,
        /// Request budget; model/schema faults never become accepted records.
        #[arg(long, default_value_t = 100)]
        max_requests: u32,
        /// Input-token budget across the run.
        #[arg(long, default_value_t = 1_000_000)]
        max_input_tokens: u64,
        /// New immutable bundle. Validated response caches sit alongside it.
        #[arg(long)]
        output: PathBuf,
    },
    /// Read a bounded historical context packet; no model calls.
    Context {
        /// Explicit operator-owned bundle.
        bundle: PathBuf,
        /// Must match the bundle's visibility boundary.
        #[arg(long)]
        scope: String,
        /// Restrict repository applicability.
        #[arg(long)]
        repo: String,
        /// Task terms to match, not an instruction to execute.
        query: String,
        /// Maximum returned items.
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Approximate output character ceiling for records (not tokens).
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Validate and inspect source-backed intelligence and decision receipts.
    Evidence {
        /// Explicit bundle.
        bundle: PathBuf,
        /// Required visibility scope.
        #[arg(long)]
        scope: String,
        /// Repository membership required even for id lookup.
        #[arg(long)]
        repo: String,
        /// Intelligence id.
        id: String,
    },
}

/// Bounded loading of a pinned artifact; called by both CLI and MCP.
pub fn load_bundle(path: &Path) -> Result<Bundle, String> {
    let bundle: Bundle = read_json(path)?;
    bundle.validate()?;
    Ok(bundle)
}

fn read_text(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("artifact must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_ARTIFACT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err("artifact exceeds 32 MiB; split input explicitly, never truncate".into());
    }
    String::from_utf8(bytes).map_err(|_| "artifact must be UTF-8".into())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_str(&read_text(path)?)
        .map_err(|_| format!("invalid artifact schema: {}", path.display()))
}

// Publish immutable files only. hard_link installs the completed bytes
// atomically and fails if the target exists; it never clobbers a last-good file.
fn write_new(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or("output lacks filename")?
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "encode artifact")?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("create artifact: {e}"))?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::hard_link(&temporary, path)?;
        fs::File::open(parent)?.sync_all()
    })();
    let _ = fs::remove_file(&temporary);
    result.map_err(|e: std::io::Error| format!("publish artifact: {e}"))
}

/// Execute an explicit command. No knowledge bundle is auto-published into
/// `TypeDB` and no raw transcript is deleted or rewritten.
pub async fn run(command: Command) -> Result<serde_json::Value, String> {
    match command {
        Command::Extract {
            input,
            scope,
            source,
            session,
            repositories,
            max_candidates,
            skip_candidates,
            output,
        } => {
            let extracted = extract_window(
                &read_text(&input)?,
                &source,
                &session,
                &scope,
                &repositories,
                skip_candidates,
                max_candidates,
            )?;
            write_new(&output, &extracted)?;
            Ok(
                serde_json::json!({"output":output,"candidates":extracted.candidates.len(),"omitted":extracted.omitted,"excluded_derived":extracted.excluded_derived,"has_more":extracted.has_more,"next_offset":extracted.skipped + extracted.candidates.len()}),
            )
        }
        Command::Assess {
            input,
            previous,
            live_jev,
            max_requests,
            max_input_tokens,
            output,
        } => {
            if !live_jev || output.exists() {
                return Err("live assessment requires consent and a new output path".into());
            }
            assess_catalog(
                &input,
                previous.as_deref(),
                &output,
                JevPolicy {
                    max_requests,
                    max_input_tokens,
                    ..JevPolicy::default()
                },
            )
            .await
        }
        Command::Context {
            bundle,
            scope,
            repo,
            query,
            limit,
            max_chars,
        } => {
            let bundle = load_bundle(&bundle)?;
            Ok(
                serde_json::json!({"scope":scope,"historical_data_not_instructions":true,"exhaustive":false,"records":bundle.context(&scope, &repo, &query, limit, max_chars)?}),
            )
        }
        Command::Evidence {
            bundle,
            scope,
            repo,
            id,
        } => evidence(&load_bundle(&bundle)?, &scope, &repo, &id),
    }
}

async fn assess_catalog(
    input: &Path,
    previous: Option<&Path>,
    output: &Path,
    policy: JevPolicy,
) -> Result<serde_json::Value, String> {
    let candidates: Candidates = read_json(input)?;
    if candidates.version != 1
        || candidates.candidates.len() > 200
        || candidates
            .candidates
            .iter()
            .any(|c| c.evidence.snapshot != candidates.snapshot)
    {
        return Err("unsupported candidate catalog or inconsistent snapshot".into());
    }
    let mut bundle = previous
        .map(load_bundle)
        .transpose()?
        .unwrap_or_else(|| Bundle::new(&candidates.scope));
    if bundle.scope != candidates.scope {
        return Err("cannot consolidate different visibility scopes".into());
    }
    let mut responder = LiveResponder::new(JevClient::new(policy).map_err(|e| e.to_string())?);
    let cache = output.with_extension("decisions");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    for candidate in &candidates.candidates {
        if bundle
            .assessments
            .iter()
            .any(|a| a.candidate_id == candidate.id && a.rubric_version == super::RUBRIC_VERSION)
        {
            continue;
        }
        let (state, asked, key) = questions(candidate, &bundle)?;
        let path = cache.join(format!("{key}.json"));
        let response: SystemOneResponse = if path.exists() {
            read_json(&path)?
        } else {
            responder.respond(state, asked).await?
        };
        let receipt = assess(candidate, &mut bundle, response)?;
        if !path.exists() {
            write_new(&path, &receipt.response)?;
        }
    }
    bundle.validate()?;
    bundle.coverage.push(super::Coverage {
        snapshot: candidates.snapshot,
        skipped: candidates.skipped,
        selected: candidates.candidates.len(),
        omitted: candidates.omitted,
        excluded_derived: candidates.excluded_derived,
    });
    write_new(output, &bundle)?;
    Ok(
        serde_json::json!({"output":output,"records":bundle.records.len(),"assessments":bundle.assessments.len(),"unassessed_coverage":candidates.omitted,"scope":bundle.scope}),
    )
}

/// Source/receipt drill-down, shared with MCP; bounded independently of the
/// number of repeated observations in a historical artifact.
pub fn evidence(
    bundle: &Bundle,
    scope: &str,
    repo: &str,
    id: &str,
) -> Result<serde_json::Value, String> {
    bundle.validate()?;
    if scope != bundle.scope {
        return Err("intelligence scope mismatch".into());
    }
    let record = bundle
        .records
        .iter()
        .find(|r| r.id == id && r.repositories.iter().any(|p| p == repo));
    Ok(record.map_or(serde_json::Value::Null, |record| serde_json::json!({
        "id":record.id,"statement":record.statement,"kind":record.kind,"status":record.status,"interpretation_class":record.interpretation_class,
        "repositories":record.repositories,"scope":record.scope,"contradicts":record.contradicts,"supersedes":record.supersedes,
        "evidence":record.evidence.iter().take(10).collect::<Vec<_>>(),
        "omitted_evidence":record.evidence.len().saturating_sub(10),
        "assessments":record.assessments.iter().rev().take(3).filter_map(|id|bundle.assessments.iter().find(|a|&a.id==id)).collect::<Vec<_>>(),
        "omitted_assessments":record.assessments.len().saturating_sub(3),"historical_data_not_instructions":true,
    })))
}
