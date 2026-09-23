//! Operator ingestion/assessment and read-only intelligence artifact queries.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use chaosbox_jev::{JevClient, JevPolicy, SystemOneResponse};
use clap::Subcommand;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use super::{assess, extract_window, questions, Bundle, Candidates};
use crate::{LiveResponder, Responder};

const MAX_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;

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
        /// Original normalized transcript; all anchors are revalidated before inference.
        #[arg(long)]
        source_jsonl: PathBuf,
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
    load_bundle_mode(path, false)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleFile {
    version: u32,
    scope: String,
    records: Vec<chaosbox_core::intelligence::Intelligence>,
    coverage: Vec<super::Coverage>,
    receipts: Vec<String>,
}

fn receipt_path(bundle: &Path, id: &str) -> Result<PathBuf, String> {
    let hash = id
        .strip_prefix("intel-assessment:")
        .ok_or("invalid receipt id")?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid receipt hash".into());
    }
    Ok(bundle
        .parent()
        .unwrap_or(Path::new("."))
        .join("intelligence-receipts")
        .join(format!("{hash}.json")))
}

fn load_bundle_mode(path: &Path, all_receipts: bool) -> Result<Bundle, String> {
    load_bundle_with_budget(path, all_receipts, MAX_BUNDLE_BYTES)
}

fn load_bundle_with_budget(path: &Path, all_receipts: bool, budget: u64) -> Result<Bundle, String> {
    let text = read_text_limited(path, budget.min(MAX_ARTIFACT_BYTES))?;
    let mut remaining = budget - text.len() as u64;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "invalid intelligence manifest")?;
    let bundle: Bundle = if value.get("version").and_then(serde_json::Value::as_u64) == Some(2) {
        let stored: BundleFile =
            serde_json::from_value(value).map_err(|_| "invalid intelligence manifest")?;
        if stored.receipts.len() > 100_000 {
            return Err("receipt index exceeds bounded artifact capacity".into());
        }
        let needed: std::collections::BTreeSet<_> =
            stored.records.iter().flat_map(|r| &r.assessments).collect();
        let mut assessments = Vec::new();
        for id in &stored.receipts {
            if !all_receipts && !needed.contains(id) {
                continue;
            }
            let text =
                read_text_limited(&receipt_path(path, id)?, remaining.min(MAX_ARTIFACT_BYTES))?;
            remaining -= text.len() as u64;
            let assessment: super::Assessment =
                serde_json::from_str(&text).map_err(|_| "invalid assessment receipt")?;
            if &assessment.id != id {
                return Err("receipt reference mismatch".into());
            }
            assessments.push(assessment);
        }
        Bundle {
            version: 1,
            scope: stored.scope,
            records: stored.records,
            coverage: stored.coverage,
            assessments,
        }
    } else {
        serde_json::from_value(value).map_err(|_| "invalid legacy intelligence bundle")?
    };
    bundle.validate()?;
    Ok(bundle)
}

fn publish_bundle(path: &Path, bundle: &Bundle) -> Result<(), String> {
    bundle.validate()?;
    let directory = path
        .parent()
        .unwrap_or(Path::new("."))
        .join("intelligence-receipts");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    for assessment in &bundle.assessments {
        let target = receipt_path(path, &assessment.id)?;
        if target.exists() {
            let existing: super::Assessment = read_json(&target)?;
            if existing.identity()? != assessment.id {
                return Err("existing receipt content mismatch".into());
            }
        } else {
            write_new(&target, assessment)?;
        }
    }
    write_new(
        path,
        &BundleFile {
            version: 2,
            scope: bundle.scope.clone(),
            records: bundle.records.clone(),
            coverage: bundle.coverage.clone(),
            receipts: bundle.assessments.iter().map(|a| a.id.clone()).collect(),
        },
    )
}

fn read_text(path: &Path) -> Result<String, String> {
    read_text_limited(path, MAX_ARTIFACT_BYTES)
}

fn read_text_limited(path: &Path, limit: u64) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("artifact must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(
            "artifact exceeds file or aggregate read budget; partition explicitly, never truncate"
                .into(),
        );
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
    if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err("artifact exceeds reader capacity; partition explicitly".into());
    }
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
            source_jsonl,
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
                &source_jsonl,
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
    source_jsonl: &Path,
    previous: Option<&Path>,
    output: &Path,
    policy: JevPolicy,
) -> Result<serde_json::Value, String> {
    let candidates: Candidates = read_json(input)?;
    if candidates.version != 2
        || candidates.candidates.len() > 200
        || candidates
            .candidates
            .iter()
            .any(|c| c.evidence.snapshot != candidates.snapshot)
    {
        return Err("unsupported candidate catalog or inconsistent snapshot".into());
    }
    let verified = extract_window(
        &read_text(source_jsonl)?,
        &candidates.source,
        &candidates.session,
        &candidates.scope,
        &candidates.repositories,
        candidates.skipped,
        candidates.candidates.len().max(1),
    )?;
    if verified.snapshot != candidates.snapshot
        || verified.candidates != candidates.candidates
        || verified.omitted != candidates.omitted
        || verified.excluded_derived != candidates.excluded_derived
    {
        return Err(
            "candidate anchors do not match the original transcript; re-extract before assessment"
                .into(),
        );
    }
    let mut bundle = previous
        .map(|path| load_bundle_mode(path, true))
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
        let (state, asked, key) = questions(candidate, &bundle)?;
        if bundle.assessments.iter().any(|a| {
            a.candidate_id == candidate.id
                && a.rubric_version == super::RUBRIC_VERSION
                && a.cache_key == key
        }) {
            continue;
        }
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
    publish_bundle(output, &bundle)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use chaosbox_jev::{Answer, ChoiceAnswer, NoulAnswer, Question, Usage, JEV_MODEL_PINNED};

    #[test]
    fn receipt_storage_is_shared_private_immutable_and_lazy_for_consumers() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.json");
        let second = directory.path().join("second.json");
        let source = serde_json::json!({"id":"u1","type":"user","text":"We must preserve native process permissions."}).to_string();
        let catalog = extract_window(
            &source,
            "opencode",
            "s",
            "private:can",
            &["canix".into()],
            0,
            10,
        )
        .unwrap();
        let candidate = &catalog.candidates[0];
        let mut bundle = Bundle::new("private:can");
        let (_, asked, _) = questions(candidate, &bundle).unwrap();
        let answers = asked
            .iter()
            .map(|(name, question)| {
                let answer = match question {
                    Question::Noul { .. } => Answer::Noul(NoulAnswer { noul: 0.0 }),
                    Question::Choice { criteria, .. } => {
                        let choice = criteria.keys().next().unwrap().clone();
                        Answer::Choice(ChoiceAnswer {
                            choice: choice.clone(),
                            confidence: 1.0,
                            probabilities: criteria
                                .keys()
                                .map(|key| (key.clone(), f64::from(key == &choice)))
                                .collect(),
                        })
                    }
                    Question::Score { .. } => panic!("unexpected score"),
                };
                (name.clone(), answer)
            })
            .collect();
        let response = SystemOneResponse {
            model: JEV_MODEL_PINNED.into(),
            answers,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
            },
        };
        assess(candidate, &mut bundle, response).unwrap();
        publish_bundle(&first, &bundle).unwrap();
        publish_bundle(&second, &bundle).unwrap();
        assert_eq!(
            fs::read_dir(directory.path().join("intelligence-receipts"))
                .unwrap()
                .count(),
            1
        );
        let manifest: serde_json::Value = read_json(&first).unwrap();
        assert!(manifest.get("assessments").is_none());
        assert_eq!(manifest["receipts"].as_array().unwrap().len(), 1);
        assert_eq!(load_bundle_mode(&first, true).unwrap().assessments.len(), 1);
        let manifest_size = fs::metadata(&first).unwrap().len();
        assert!(load_bundle_with_budget(&first, true, manifest_size + 16).is_err());
        assert!(load_bundle_with_budget(&first, false, manifest_size + 16).is_ok());
        assert!(load_bundle(&first).unwrap().assessments.is_empty());
        assert!(publish_bundle(&first, &bundle).is_err());
        let receipt = receipt_path(&first, &bundle.assessments[0].id).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::write(&receipt, b"{}").unwrap();
        assert!(load_bundle_mode(&first, true).is_err());
    }
}
