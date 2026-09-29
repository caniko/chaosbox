//! Operator-only compiler capture/import. Read-only graph queries never execute
//! tools. A capture brackets a fresh indexer invocation with input checks.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
};
use chaosbox_extract::{
    compiler::{AnalysisInputs, AnalysisSettings, Encoding, Receipt},
    Extraction, Snapshot,
};
use clap::Subcommand;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Optional compiler evidence commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute an explicit indexer and retain a source/configuration-bound receipt.
    Capture {
        /// Project root; must match SCIP `metadata.project_root`.
        path: PathBuf,
        /// Repository namespace.
        #[arg(long)]
        repo: String,
        /// Restrict source capture to these subtrees (repeatable).
        #[arg(long = "source-paths")]
        source_paths: Vec<String>,
        /// Additional in-tree configuration/generated input files (repeatable).
        #[arg(long = "input")]
        inputs: Vec<String>,
        /// Effective inputs as key=value. Required: toolchain, configuration,
        /// target, features. Include external/environmental inputs explicitly.
        #[arg(long = "context", value_parser = key_value)]
        context: Vec<(String, String)>,
        /// Fallback only for unspecified encoding (TS 0.4.0 requires utf16).
        #[arg(long, value_parser = ["utf8", "utf16", "utf32"])]
        unspecified_encoding: Option<String>,
        /// New artifact directory, outside the indexed project.
        #[arg(long)]
        output: PathBuf,
        /// Maximum indexer runtime in seconds.
        #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout_seconds: u64,
        /// Executable and arguments after `--`; use one `{index}` argument for
        /// the output file. No shell interpretation or implicit indexer selection.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Check current inputs and report normalized compiler coverage.
    Inspect {
        /// Artifact directory produced by capture.
        artifact: PathBuf,
        /// Current source root.
        #[arg(long)]
        path: PathBuf,
    },
}

fn key_value(value: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = value.split_once('=').ok_or("context must be key=value")?;
    if key.is_empty() || value.trim().is_empty() {
        return Err("context key/value must be nonempty".into());
    }
    Ok((key.into(), value.into()))
}

/// Run an explicit compiler operation and return its JSON result.
pub async fn run(command: Command) -> Result<serde_json::Value> {
    match command {
        Command::Inspect { artifact, path } => {
            let receipt = load_receipt(&artifact)?;
            let snapshot =
                Snapshot::capture_scoped(&receipt.inputs.repo, &path, &receipt.inputs.scope)?;
            let mut extraction = chaosbox_extract::extract_snapshot(&snapshot);
            attach(&artifact, &path, &snapshot, &mut extraction)?;
            Ok(serde_json::to_value(
                extraction.compiler.as_ref().map(|c| c.report()),
            )?)
        }
        Command::Capture {
            path,
            repo,
            source_paths,
            inputs,
            context,
            unspecified_encoding,
            output,
            timeout_seconds,
            command,
        } => {
            let path = path.canonicalize()?;
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .canonicalize()?;
            if parent.starts_with(&path) || output.exists() {
                return Err("artifact output must be new and outside the project".into());
            }
            let mut declared = BTreeMap::new();
            for (key, value) in context {
                if declared.insert(key, value).is_some() {
                    return Err("duplicate context key".into());
                }
            }
            let unspecified_encoding = match unspecified_encoding.as_deref() {
                Some("utf8") => Some(Encoding::Utf8),
                Some("utf16") => Some(Encoding::Utf16),
                Some("utf32") => Some(Encoding::Utf32),
                None => None,
                Some(_) => return Err("unsupported encoding".into()),
            };
            let settings = AnalysisSettings {
                command,
                declared,
                unspecified_encoding,
            };
            settings.validate()?;
            let before = AnalysisInputs::capture(&repo, &path, &source_paths, &inputs)?;
            let scratch = tempfile::tempdir_in(&parent)?;
            let index = scratch.path().join("index.scip");
            let status = tokio::time::timeout(
                std::time::Duration::from_secs(timeout_seconds),
                tokio::process::Command::new(&settings.command[0])
                    .args(settings.command[1..].iter().map(|arg| {
                        if arg == "{index}" {
                            index.as_os_str()
                        } else {
                            std::ffi::OsStr::new(arg)
                        }
                    }))
                    .current_dir(&path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(std::io::stderr()))
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true)
                    .status(),
            )
            .await??;
            if !status.success() {
                return Err(format!("indexer exited {status}; no receipt created").into());
            }
            let after = AnalysisInputs::capture(&repo, &path, &source_paths, &inputs)?;
            if before != after {
                return Err("inputs changed while indexing; no receipt created".into());
            }
            let bytes = read_bounded(&index, 128 * 1024 * 1024)?;
            let receipt = Receipt::seal(before, settings, &bytes)?;
            let snapshot = Snapshot::capture_scoped(&repo, &path, &source_paths)?;
            let extraction = chaosbox_extract::compiler::normalize(&snapshot, &receipt, &bytes)?;
            receipt.verify(&path)?;
            // Reserve the new directory atomically. Partial writes fail closed
            // on load; an existing operator artifact is never overwritten.
            let mut directory = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                directory.mode(0o700);
            }
            directory.create(&output)?;
            fs::write(output.join("index.scip"), bytes)?;
            fs::write(
                output.join("receipt.json"),
                serde_json::to_vec_pretty(&receipt)?,
            )?;
            Ok(serde_json::json!({"artifact": output, "coverage": extraction.coverage.report()}))
        }
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err("artifact is not a regular file".into());
    }
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("artifact exceeds size limit".into());
    }
    Ok(bytes)
}

/// Read one bounded receipt; source verification is separate and mandatory.
pub fn load_receipt(artifact: &Path) -> Result<Receipt> {
    Ok(serde_json::from_slice(&read_bounded(
        &artifact.join("receipt.json"),
        32 * 1024 * 1024,
    )?)?)
}

/// Verify and add one compiler context to an existing syntax extraction.
pub fn attach(
    artifact: &Path,
    root: &Path,
    snapshot: &Snapshot,
    extraction: &mut Extraction,
) -> Result<()> {
    let receipt = load_receipt(artifact)?;
    receipt.verify(root)?;
    let bytes = read_bounded(&artifact.join("index.scip"), 128 * 1024 * 1024)?;
    chaosbox_extract::compiler::normalize(snapshot, &receipt, &bytes)?.attach(extraction)?;
    Ok(())
}
