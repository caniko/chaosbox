//! Local-only authorized projection. Never forwards queries or alters source bundles.
use std::collections::BTreeSet;
use chaosbox_core::intelligence::Intelligence;
use crate::intelligence::{Bundle, RUBRIC_VERSION, words};
use super::{
    Citation, ContextPacket, ContextRequest, ErrorCode, EvidencePacket, Handle, KnowledgeSource,
    Policy, ReceiptProjection, Record, Reply, Request, Snapshot, chars,
};

/// Authorized reader over a single owner's knowledge source.
pub struct Reader<S> {
    /// Current policy; file-backed endpoints reload it for each request.
    pub policy: Policy,
    source: S,
}
impl<S: KnowledgeSource> Reader<S> {
    /// Construct a provider without any inference or writer capability.
    pub fn new(policy: Policy, source: S) -> Result<Self, ErrorCode> {
        policy.validate()?;
        Ok(Self { policy, source })
    }
    /// Recipient comes from authenticated transport, never the wire request.
    pub async fn query(&self, recipient: &str, request: &Request) -> Result<Reply, ErrorCode> {
        self.policy.validate()?;
        match request {
            Request::Context(args) => {
                args.validate()?;
                self.policy.repo(recipient, &args.project)?;
                let snapshot = self.source.current(&self.policy.identity.scope).await?;
                self.check(&snapshot)?;
                self.context(recipient, args, &snapshot).map(Reply::Context)
            }
            Request::Evidence { handle, max_chars } => {
                if !(256..=32_000).contains(max_chars)
                    || !super::hash(&handle.snapshot)
                    || !handle.id.strip_prefix("intel:").is_some_and(super::hash)
                {
                    return Err(ErrorCode::InvalidRequest);
                }
                let identity = &self.policy.identity;
                if handle.provider != identity.provider
                    || handle.owner != identity.owner
                    || handle.scope != identity.scope
                {
                    return Err(ErrorCode::Denied);
                }
                self.policy.repo(recipient, &handle.project)?;
                let current = self.source.current(&identity.scope).await?;
                self.check(&current)?;
                // An old handle cannot resurrect deleted, withheld or unselected knowledge.
                if !current.bundle.records.iter().any(|r| {
                    r.id == handle.id && self.policy.eligible(recipient, &handle.project, r)
                }) {
                    return Err(ErrorCode::Denied);
                }
                let historical = if current.id == handle.snapshot {
                    current.clone()
                } else {
                    self.source
                        .snapshot(&identity.scope, &handle.snapshot)
                        .await?
                        .ok_or(ErrorCode::SnapshotUnavailable)?
                };
                self.check(&historical)?;
                if historical.id != handle.snapshot {
                    return Err(ErrorCode::InvalidResponse);
                }
                self.evidence(recipient, handle, *max_chars, &historical, &current)
                    .map(|packet| Reply::Evidence(Box::new(packet)))
            }
        }
    }
    fn check(&self, snapshot: &Snapshot) -> Result<(), ErrorCode> {
        snapshot
            .bundle
            .validate()
            .map_err(|_| ErrorCode::InvalidResponse)?;
        if snapshot.bundle.scope != self.policy.identity.scope || !super::hash(&snapshot.id) {
            return Err(ErrorCode::InvalidResponse);
        }
        Ok(())
    }
    fn context(
        &self,
        recipient: &str,
        args: &ContextRequest,
        snapshot: &Snapshot,
    ) -> Result<ContextPacket, ErrorCode> {
        let allowed = self.allowed(recipient, &args.project, &snapshot.bundle);
        let tokens = words(&args.query);
        let mut ranked: Vec<_> = snapshot
            .bundle
            .records
            .iter()
            .filter(|r| allowed.contains(&r.id))
            .filter_map(|r| {
                let score = words(&r.statement).intersection(&tokens).count();
                (score > 0).then_some((score, r))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        let total = ranked.len();
        let mut records = Vec::new();
        for (_, record) in ranked {
            if records.len() == args.limit {
                break;
            }
            records.push(self.project(record, &args.project, snapshot, &allowed));
            if chars(&records)? > args.max_chars {
                records.pop();
            }
        }
        Ok(ContextPacket {
            identity: self.policy.identity.clone(),
            project: args.project.clone(),
            snapshot: snapshot.id.clone(),
            policy_version: self.policy.digest()?,
            omitted: total - records.len(),
            records,
        })
    }
    fn allowed(&self, recipient: &str, project: &str, bundle: &Bundle) -> BTreeSet<String> {
        bundle
            .records
            .iter()
            .filter(|r| self.policy.eligible(recipient, project, r))
            .map(|r| r.id.clone())
            .collect()
    }
    fn project(
        &self,
        record: &Intelligence,
        project: &str,
        snapshot: &Snapshot,
        allowed: &BTreeSet<String>,
    ) -> Record {
        let identity = &self.policy.identity;
        let qualified = |id: &str| format!("{}::{id}", identity.provider);
        let contradicts: Vec<_> = record
            .contradicts
            .iter()
            .filter(|id| allowed.contains(*id))
            .map(|id| qualified(id))
            .collect();
        let supersedes = record
            .supersedes
            .as_ref()
            .filter(|id| allowed.contains(*id))
            .map(|id| qualified(id));
        let omitted_relationships = record.contradicts.len() - contradicts.len()
            + usize::from(record.supersedes.is_some() && supersedes.is_none());
        let mut citations = vec![Citation::from(&record.evidence[0])];
        if let Some(evidence) = record.evidence.last().filter(|_| record.evidence.len() > 1) {
            citations.push(Citation::from(evidence));
        }
        let latest = record
            .assessments
            .last()
            .and_then(|id| snapshot.bundle.assessments.iter().find(|a| &a.id == id));
        Record {
            id: record.id.clone(),
            qualified_id: qualified(&record.id),
            handle: Handle {
                provider: identity.provider.clone(),
                owner: identity.owner.clone(),
                scope: identity.scope.clone(),
                project: project.into(),
                snapshot: snapshot.id.clone(),
                id: record.id.clone(),
            },
            statement: record.statement.clone(),
            kind: record.kind,
            status: record.status,
            interpretation_class: record.interpretation_class,
            citations,
            evidence_count: record.evidence.len(),
            assessment_count: record.assessments.len(),
            needs_revalidation: latest.is_none_or(|a| a.rubric_version != RUBRIC_VERSION),
            contradicts,
            supersedes,
            omitted_relationships,
            same_origin: vec![],
        }
    }
    fn evidence(
        &self,
        recipient: &str,
        handle: &Handle,
        max_chars: usize,
        snapshot: &Snapshot,
        current: &Snapshot,
    ) -> Result<EvidencePacket, ErrorCode> {
        let allowed_current = self.allowed(recipient, &handle.project, &current.bundle);
        let allowed: BTreeSet<_> = self
            .allowed(recipient, &handle.project, &snapshot.bundle)
            .intersection(&allowed_current)
            .cloned()
            .collect();
        let record = snapshot
            .bundle
            .records
            .iter()
            .find(|r| r.id == handle.id && allowed.contains(&r.id))
            .ok_or(ErrorCode::Denied)?;
        let mut packet = EvidencePacket {
            record: self.project(record, &handle.project, snapshot, &allowed),
            policy_version: self.policy.digest()?,
            evidence: vec![],
            receipts: vec![],
            omitted_evidence: record.evidence.len(),
            omitted_receipts: record.assessments.len(),
            is_projection: true,
        };
        if chars(&packet)? > max_chars {
            return Err(ErrorCode::InvalidRequest);
        }
        for evidence in record.evidence.iter().take(10) {
            packet.evidence.push(evidence.clone());
            packet.omitted_evidence -= 1;
            if chars(&packet)? > max_chars {
                packet.evidence.pop();
                packet.omitted_evidence += 1;
            }
        }
        for id in record.assessments.iter().rev().take(3) {
            let receipt = snapshot
                .bundle
                .assessments
                .iter()
                .find(|a| &a.id == id)
                .ok_or(ErrorCode::InvalidResponse)?;
            packet.receipts.push(ReceiptProjection {
                id: id.clone(),
                rubric_version: receipt.rubric_version.clone(),
                model_requested: receipt.model_requested.clone(),
                state_omitted: true,
            });
            packet.omitted_receipts -= 1;
            if chars(&packet)? > max_chars {
                packet.receipts.pop();
                packet.omitted_receipts += 1;
            }
        }
        Ok(packet)
    }
}
