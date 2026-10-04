//! Directional project grants. Applicability strings alone are not authorization.
use std::collections::{BTreeMap, BTreeSet};
use chaosbox_core::intelligence::{Intelligence, IntelligenceStatus};
use serde::{Deserialize, Serialize};
use super::ErrorCode;

/// Explicit provider identity, pinned by deployment rather than client input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Stable provider id; separate devices may serve the same owner.
    pub provider: String,
    /// Owner identity authenticated through the provider transport.
    pub owner: String,
    /// Owner-local visibility boundary, never merged or renamed.
    pub scope: String,
}
impl Identity {
    pub(crate) fn validate(&self) -> Result<(), ErrorCode> {
        if !label(&self.provider)
            || !label(&self.owner)
            || !self.scope.starts_with("private:")
            || self.scope.len() <= 8
            || self.scope.len() > 256
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}
fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// A shared identity maps to one provider-local repository association.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Existing intelligence repository string, explicitly mapped by the owner.
    pub repo: String,
}

/// Selected knowledge is the default; all-project sharing is explicit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharingMode {
    /// Only exact record ids chosen by the owner.
    #[default]
    Selected,
    /// All currently admitted/disputed intelligence applicable to this project.
    AllAdmitted,
}

/// One owner-to-recipient project grant; no transitive or wildcard permissions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    /// Authenticated recipient fixed by the server invocation.
    pub recipient: String,
    /// Exact configured project identity.
    pub project: String,
    /// Operator-managed positive permission revision.
    pub revision: u64,
    /// Sharing boundary; omitted means selected.
    #[serde(default)]
    pub mode: SharingMode,
    /// Original record ids selected for this recipient/project.
    #[serde(default)]
    pub records: Vec<String>,
}

/// Reloaded on each provider request, including historical evidence lookup.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Policy contract version, currently 1.
    pub version: u32,
    /// Operator-pinned identity.
    pub identity: Identity,
    /// Explicit project mappings; local paths/names are not guessed.
    pub projects: BTreeMap<String, Project>,
    /// Directional sharing grants. Empty grants disclose nothing to peers.
    #[serde(default)]
    pub grants: Vec<Grant>,
}

impl Policy {
    /// Reject ambiguous grants and malformed identities before reading knowledge.
    pub fn validate(&self) -> Result<(), ErrorCode> {
        self.identity.validate()?;
        if self.version != 1
            || self.projects.len() > 1024
            || self.grants.len() > 4096
            || self.projects.iter().any(|(key, project)| {
                key.trim().is_empty()
                    || key.len() > 256
                    || project.repo.trim().is_empty()
                    || project.repo.len() > 256
            })
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut seen = BTreeSet::new();
        for grant in &self.grants {
            if !label(&grant.recipient)
                || grant.revision == 0
                || !self.projects.contains_key(&grant.project)
                || !seen.insert((&grant.recipient, &grant.project))
                || grant.records.len() > 100_000
                || grant
                    .records
                    .iter()
                    .any(|id| !id.strip_prefix("intel:").is_some_and(super::hash))
                || (grant.mode == SharingMode::AllAdmitted && !grant.records.is_empty())
            {
                return Err(ErrorCode::InvalidRequest);
            }
        }
        Ok(())
    }
    pub(crate) fn repo(&self, recipient: &str, project: &str) -> Result<&str, ErrorCode> {
        let mapping = self.projects.get(project).ok_or(ErrorCode::Denied)?;
        if recipient != self.identity.owner && self.grant(recipient, project).is_none() {
            return Err(ErrorCode::Denied);
        }
        Ok(&mapping.repo)
    }
    fn grant(&self, recipient: &str, project: &str) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| g.recipient == recipient && g.project == project)
    }
    pub(crate) fn eligible(&self, recipient: &str, project: &str, record: &Intelligence) -> bool {
        record.scope == self.identity.scope
            && self
                .repo(recipient, project)
                .is_ok_and(|repo| record.repositories.iter().any(|r| r == repo))
            && matches!(
                record.status,
                IntelligenceStatus::Admitted | IntelligenceStatus::Disputed
            )
            && (recipient == self.identity.owner
                || self.grant(recipient, project).is_some_and(|g| {
                    g.mode == SharingMode::AllAdmitted || g.records.contains(&record.id)
                }))
    }
    pub(crate) fn digest(&self) -> Result<String, ErrorCode> {
        let json = serde_json::to_string(self).map_err(|_| ErrorCode::InvalidRequest)?;
        Ok(chaosbox_core::sha256_hex(&["federation-policy-v1", &json]))
    }
}
