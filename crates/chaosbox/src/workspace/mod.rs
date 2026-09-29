//! Immutable, explicitly reviewed cross-language/workspace impact pilot.
//! The authoritative single-repository graph and private memory admission remain
//! independent; this artifact connects exact member builds with cited bridges.

mod capture;
pub mod cli;
mod impact;
mod model;
mod version;

pub use capture::capture;
pub use model::{
    Bridge, Constraint, DependencyPin, Endpoint, EndpointSpec, Member, MemberSpec, Spec, Workspace,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const CONTRACT: &str = "workspace-impact-pilot-v1";

fn validate_scope(expected: &str, actual: &str) -> Result<()> {
    if expected != actual
        || !actual.starts_with("private:")
        || actual.len() <= 8
        || actual.len() > 256
    {
        return Err("workspace visibility scope mismatch".into());
    }
    Ok(())
}

fn validate_bounds(
    members: usize,
    endpoints: usize,
    bridges: usize,
    constraints: usize,
) -> Result<()> {
    if members == 0 || members > 16 || endpoints > 128 || bridges > 256 || constraints > 64 {
        return Err("workspace exceeds pilot bounds or has no members".into());
    }
    Ok(())
}

impl Workspace {
    fn fingerprint(&self) -> Result<String> {
        let mut value = serde_json::to_value(self)?;
        value["id"] = serde_json::Value::String(String::new());
        Ok(format!(
            "workspace:{}",
            chaosbox_core::sha256_hex(&[&serde_json::to_string(&value)?])
        ))
    }

    fn validate(&self, scope: &str) -> Result<()> {
        validate_scope(&self.scope, scope)?;
        validate_bounds(
            self.members.len(),
            self.endpoints.len(),
            self.bridges.len(),
            self.constraints.len(),
        )?;
        if self.contract != CONTRACT || self.id != self.fingerprint()? {
            return Err("workspace artifact fingerprint/contract mismatch".into());
        }
        for endpoint in self.endpoints.values() {
            if endpoint.quote.is_empty() || endpoint.quote.len() > 8192 {
                return Err("invalid endpoint quote length".into());
            }
            let member = self
                .members
                .get(&endpoint.member)
                .ok_or("endpoint has no member")?;
            let entity = member
                .build
                .nodes
                .get(&endpoint.entity)
                .ok_or("endpoint has no member entity")?;
            if endpoint.build != member.build.id
                || endpoint.citation.snapshot != member.snapshot.id
                || member.build.repo != endpoint.member
                || member.snapshot.repo != endpoint.member
                || member.build.snapshot_ids != [member.snapshot.id.clone()]
                || entity.repo != endpoint.member
                || entity.snapshot != member.snapshot.id
                || entity.file != endpoint.citation.file
            {
                return Err("endpoint is outside its pinned member build".into());
            }
            let version = member
                .snapshot
                .file_version(&endpoint.citation.file)
                .ok_or("endpoint has no file version")?;
            let text = member
                .snapshot
                .contents
                .get(&version.path)
                .ok_or("endpoint has no source")?;
            let span = endpoint
                .citation
                .span
                .as_ref()
                .ok_or("endpoint has no span")?;
            if version.sha256 != endpoint.citation.sha256
                || span.file != endpoint.citation.file
                || version.sha256 != chaosbox_core::sha256_hex(&[text])
                || text.get(span.byte_start as usize..span.byte_end as usize)
                    != Some(&endpoint.quote)
            {
                return Err("endpoint source citation mismatch".into());
            }
        }
        for bridge in &self.bridges {
            let from = self
                .endpoints
                .get(&bridge.from)
                .ok_or("bridge provider missing")?;
            let to = self
                .endpoints
                .get(&bridge.to)
                .ok_or("bridge consumer missing")?;
            self.validate_evidence(&bridge.evidence)?;
            if bridge.reason.trim().is_empty()
                || bridge.reason.len() > 8192
                || (from.member != to.member && bridge.pin.is_none())
            {
                return Err("bridge needs a reason and cross-repository version pin".into());
            }
            if let Some(pin) = &bridge.pin {
                if pin.member != to.member {
                    return Err("dependency pin must belong to the consumer".into());
                }
                self.pinned_revision(pin)?;
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for constraint in &self.constraints {
            if !ids.insert(&constraint.id)
                || constraint.id.is_empty()
                || constraint.statement.trim().is_empty()
                || constraint.statement.len() > 8192
                || constraint.applies_to.is_empty()
                || constraint.applies_to.len() > 128
            {
                return Err("invalid or duplicate constraint".into());
            }
            self.validate_evidence(&constraint.evidence)?;
            for endpoint in &constraint.applies_to {
                if !self.endpoints.contains_key(endpoint) {
                    return Err("constraint endpoint missing".into());
                }
            }
        }
        Ok(())
    }

    fn validate_evidence(&self, evidence: &[String]) -> Result<()> {
        if evidence.is_empty()
            || evidence.len() > 128
            || evidence.iter().any(|id| !self.endpoints.contains_key(id))
        {
            return Err("reviewed record requires existing source evidence".into());
        }
        Ok(())
    }

    fn pinned_revision(&self, pin: &DependencyPin) -> Result<String> {
        let member = self.members.get(&pin.member).ok_or("pin member missing")?;
        let text = member
            .snapshot
            .contents
            .get(&pin.file)
            .ok_or("pin file outside selected inputs")?;
        let json: serde_json::Value = serde_json::from_str(text)?;
        let revision = json
            .pointer(&pin.pointer)
            .and_then(serde_json::Value::as_str)
            .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or("pin must select a full Git object id")?;
        Ok(revision.into())
    }
}
