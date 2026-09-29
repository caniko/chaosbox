use std::{collections::BTreeMap, path::Path};

use chaosbox_core::{compiler::AnalysisContext, sha256_hex};
use protobuf::Message;
use scip::types::Index;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{invalid, CompilerError};
use crate::paths::checked_path;
use crate::{FileVersion, Snapshot};

const CONTRACT: &str = "scip-receipt-v1/scip-0.10.0";

/// Explicit encoding override for producers which leave encoding unspecified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    /// Byte offsets from line start.
    Utf8,
    /// UTF-16 code units from line start.
    Utf16,
    /// Unicode scalar offsets from line start.
    Utf32,
}

impl Encoding {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "utf8",
            Self::Utf16 => "utf16",
            Self::Utf32 => "utf32",
        }
    }
}

/// Inputs captured before and after an explicit compiler/indexer invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisInputs {
    /// Repository namespace.
    pub repo: String,
    /// Canonical file URL, checked against SCIP metadata.
    pub root: String,
    /// Syntax snapshot id.
    pub snapshot: String,
    /// Explicit source scope.
    pub scope: Vec<String>,
    /// Source inventory, including files omitted by the producer.
    pub sources: Vec<FileVersion>,
    /// In-tree manifests, locks, configuration and declared extra inputs.
    pub configuration: Vec<FileVersion>,
    /// Explicit additional in-tree inputs, checked for symlinks and boundaries.
    pub additional: Vec<String>,
}

fn configuration_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "Cargo.lock"
            | "flake.lock"
            | "yarn.lock"
            | "bun.lock"
            | "rust-toolchain"
            | ".gitignore"
            | ".npmrc"
            | "config"
    ) || matches!(
        name.rsplit('.').next(),
        Some("toml" | "json" | "jsonc" | "yaml" | "yml" | "nix")
    )
}

impl AnalysisInputs {
    /// Capture scoped source hashes plus in-tree configuration hashes. External
    /// dependency trees, environment and generated inputs need declarations.
    pub fn capture(
        repo: &str,
        root: &Path,
        scope: &[String],
        additional: &[String],
    ) -> Result<Self, CompilerError> {
        let root = root.canonicalize()?;
        let scope = crate::validate_scope(scope)?;
        // Check all ancestors of explicit scopes as well as the final directory.
        for path in &scope {
            checked_path(&root, path, false)?;
        }
        let snapshot = Snapshot::capture_scoped(repo, &root, &scope)?;
        let config = Snapshot::capture_matching(repo, &root, &[], configuration_file)?;
        let mut configuration: BTreeMap<_, _> = config
            .files
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();
        let mut additional = additional.to_vec();
        additional.sort();
        additional.dedup();
        for path in &additional {
            let file = checked_path(&root, path, true)?;
            let text = std::fs::read_to_string(file)?;
            configuration.insert(
                path.clone(),
                FileVersion {
                    path: path.clone(),
                    sha256: sha256_hex(&[&text]),
                    bytes: text.len() as u64,
                },
            );
        }
        Ok(Self {
            repo: repo.into(),
            root: url::Url::from_directory_path(&root)
                .map_err(|()| invalid("invalid root URL"))?
                .to_string(),
            snapshot: snapshot.id,
            scope: snapshot.scope,
            sources: snapshot.files,
            configuration: configuration.into_values().collect(),
            additional,
        })
    }
}

/// Explicit compiler execution settings, retained rather than guessed from SCIP.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisSettings {
    /// Executable and arguments, with one `{index}` output placeholder.
    pub command: Vec<String>,
    /// Effective external/environmental inputs. Required keys: toolchain,
    /// configuration, target, features. Values are operator declarations.
    pub declared: BTreeMap<String, String>,
    /// Explicit fallback only for encoding value zero; unknown values fail.
    pub unspecified_encoding: Option<Encoding>,
}

impl AnalysisSettings {
    /// Validate declarations before launching any external process.
    pub fn validate(&self) -> Result<(), CompilerError> {
        if self.command.is_empty()
            || self.command[0].is_empty()
            || self.command.iter().filter(|a| *a == "{index}").count() != 1
            || self.command[0] == "{index}"
        {
            return Err(invalid("command requires exactly one {index} argument"));
        }
        for key in ["toolchain", "configuration", "target", "features"] {
            if self.declared.get(key).is_none_or(|v| v.trim().is_empty()) {
                return Err(invalid(format!("missing declared {key}")));
            }
        }
        Ok(())
    }
}

/// Content-bound receipt. This is provenance, not a cryptographic attestation
/// that an indexer is honest or that its compiler succeeded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Adapter and schema binding contract.
    pub contract: String,
    /// Fingerprint of every field except this one.
    pub context: String,
    /// Source and configuration inventory.
    pub inputs: AnalysisInputs,
    /// Explicit invocation and configuration declarations.
    pub settings: AnalysisSettings,
    /// Raw protobuf content hash.
    pub index_sha256: String,
    /// Producer name from metadata.
    pub tool: String,
    /// Producer version from metadata.
    pub version: String,
}

/// SHA-256 of raw binary bytes, without text-hash separators.
#[must_use]
pub fn binary_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Receipt {
    /// Bind an index to a captured input inventory. The operator capture command
    /// additionally verifies unchanged inputs around actual indexer execution.
    pub fn seal(
        inputs: AnalysisInputs,
        settings: AnalysisSettings,
        bytes: &[u8],
    ) -> Result<Self, CompilerError> {
        settings.validate()?;
        let index = decode(bytes)?;
        let metadata = index
            .metadata
            .as_ref()
            .ok_or_else(|| invalid("missing index metadata"))?;
        let tool = metadata
            .tool_info
            .as_ref()
            .ok_or_else(|| invalid("missing producer metadata"))?;
        if tool.name.is_empty() || tool.version.is_empty() {
            return Err(invalid("missing producer name/version"));
        }
        let root = url::Url::parse(&metadata.project_root).map_err(|e| invalid(e.to_string()))?;
        let expected = url::Url::parse(&inputs.root).map_err(|e| invalid(e.to_string()))?;
        if root.to_file_path().ok() != expected.to_file_path().ok() || root.scheme() != "file" {
            return Err(invalid("index project root differs from captured inputs"));
        }
        if metadata.version.value() != 0 {
            return Err(invalid("unsupported SCIP protocol version"));
        }
        let mut receipt = Self {
            contract: CONTRACT.into(),
            context: String::new(),
            inputs,
            settings,
            index_sha256: binary_hash(bytes),
            tool: tool.name.clone(),
            version: tool.version.clone(),
        };
        let encoded = serde_json::to_string(&receipt).map_err(|e| invalid(e.to_string()))?;
        receipt.context = format!("analysis:{}", sha256_hex(&[&encoded]));
        Ok(receipt)
    }

    /// Fail if current source, configuration, scope or declared input inventory
    /// differs. Additions/deletions are changes, not just modified old files.
    pub fn verify(&self, root: &Path) -> Result<(), CompilerError> {
        let current = AnalysisInputs::capture(
            &self.inputs.repo,
            root,
            &self.inputs.scope,
            &self.inputs.additional,
        )?;
        if current != self.inputs {
            return Err(invalid(
                "stale source/configuration receipt; recapture the index",
            ));
        }
        Ok(())
    }

    pub(super) fn provenance(&self) -> AnalysisContext {
        AnalysisContext {
            id: self.context.clone(),
            producer: format!("{CONTRACT}/{}@{}/{}", self.tool, self.version, self.context),
            index_sha256: self.index_sha256.clone(),
            inputs: self
                .inputs
                .sources
                .iter()
                .chain(&self.inputs.configuration)
                .map(|f| (f.path.clone(), f.sha256.clone()))
                .collect(),
            command: self.settings.command.clone(),
            declared: self.settings.declared.clone(),
            encoding_override: self.settings.unspecified_encoding.map(|e| e.label().into()),
        }
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Index, CompilerError> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err(invalid("index exceeds 128 MiB"));
    }
    Index::parse_from_bytes(bytes).map_err(|e| invalid(format!("invalid SCIP protobuf: {e}")))
}
