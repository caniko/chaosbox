//! Local-first query composition with bounded deadlines and no peer-result cache.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use serde::Deserialize;
use super::{
    ContextPacket, ContextRequest, ErrorCode, EvidencePacket, Handle, Identity, Packet, Record,
    Reply, Request, SourceStatus, chars, hash,
};

/// Authenticated local-only provider. The federator holds no owner database credentials.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Expected identity pinned by the client configuration.
    fn identity(&self) -> &Identity;
    /// Project routing selection; the remote provider still enforces authorization.
    fn supports(&self, _project: &str) -> bool {
        true
    }
    /// Issue one bounded read. Caller identity is fixed by deployment/transport.
    async fn query(&self, request: &Request) -> Result<Reply, ErrorCode>;
}

/// Temporary read view over local knowledge followed by independently owned peers.
pub struct Federator {
    local: Arc<dyn Provider>,
    peers: Vec<Arc<dyn Provider>>,
    deadline: Duration,
}
impl Federator {
    /// At most eight peers; one total deadline for both phases of a context query.
    pub fn new(
        local: Arc<dyn Provider>,
        peers: Vec<Arc<dyn Provider>>,
        timeout_ms: u64,
    ) -> Result<Self, ErrorCode> {
        if peers.len() > 8 || !(100..=30_000).contains(&timeout_ms) {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut ids = BTreeSet::new();
        for provider in std::iter::once(&local).chain(&peers) {
            provider.identity().validate()?;
            if !ids.insert(provider.identity().provider.clone()) {
                return Err(ErrorCode::InvalidRequest);
            }
        }
        Ok(Self {
            local,
            peers,
            deadline: Duration::from_millis(timeout_ms),
        })
    }
    /// Always consult selected peers, even when local context is nonempty.
    pub async fn context(
        &self,
        project: &str,
        query: &str,
        limit: usize,
        max_chars: usize,
    ) -> Result<Packet, ErrorCode> {
        let args = ContextRequest {
            project: project.into(),
            query: query.into(),
            limit,
            max_chars,
        };
        args.validate()?;
        if !self.local.supports(project) {
            return Err(ErrorCode::Denied);
        }
        let end = tokio::time::Instant::now() + self.deadline;
        let request = Request::Context(args.clone());
        // Reserve peer time only when this project has selected peers.
        let local_deadline = if self.peers.iter().any(|p| p.supports(project)) {
            self.deadline / 2
        } else {
            self.deadline
        };
        let first = tokio::time::timeout(local_deadline, self.local.query(&request))
            .await
            .unwrap_or(Err(ErrorCode::Unavailable));
        let mut results = BTreeMap::from([(0, (self.local.identity().clone(), first))]);
        let mut jobs = tokio::task::JoinSet::new();
        for (index, provider) in self
            .peers
            .iter()
            .filter(|p| p.supports(project))
            .enumerate()
        {
            let provider = Arc::clone(provider);
            let request = request.clone();
            jobs.spawn(async move {
                let identity = provider.identity().clone();
                let result = tokio::time::timeout_at(end, provider.query(&request))
                    .await
                    .unwrap_or(Err(ErrorCode::Unavailable));
                (index + 1, (identity, result))
            });
        }
        while let Some(result) = jobs.join_next().await {
            let (index, result) = result.map_err(|_| ErrorCode::Unavailable)?;
            results.insert(index, result);
        }
        assemble(&args, results.into_values().collect())
    }
    /// Route back to the configured origin; no fallback to a different provider/generation.
    pub async fn evidence(
        &self,
        handle: &Handle,
        max_chars: usize,
    ) -> Result<EvidencePacket, ErrorCode> {
        if !(256..=32_000).contains(&max_chars)
            || !hash(&handle.snapshot)
            || !handle.id.strip_prefix("intel:").is_some_and(hash)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        let provider = std::iter::once(&self.local)
            .chain(&self.peers)
            .find(|p| p.identity().provider == handle.provider && p.supports(&handle.project))
            .ok_or(ErrorCode::Denied)?;
        if provider.identity().owner != handle.owner || provider.identity().scope != handle.scope {
            return Err(ErrorCode::Denied);
        }
        let request = Request::Evidence {
            handle: handle.clone(),
            max_chars,
        };
        let reply = tokio::time::timeout(self.deadline, provider.query(&request))
            .await
            .map_err(|_| ErrorCode::Unavailable)??;
        let Reply::Evidence(packet) = reply else {
            return Err(ErrorCode::InvalidResponse);
        };
        validate_record(
            &packet.record,
            provider.identity(),
            &handle.project,
            &handle.snapshot,
        )?;
        if packet.record.handle != *handle
            || chars(&packet)? > max_chars
            || !packet.is_projection
            || !hash(&packet.policy_version)
            || packet.evidence.len() > 10
            || packet.receipts.len() > 3
            || packet.receipts.iter().any(|r| !r.state_omitted)
        {
            return Err(ErrorCode::InvalidResponse);
        }
        Ok(*packet)
    }
    /// CLI/MCP adapter with the existing explicit `repo` argument convention.
    pub async fn query(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ErrorCode> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Context {
            repo: String,
            query: String,
            limit: Option<usize>,
            max_chars: Option<usize>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Evidence {
            repo: String,
            handle: Handle,
            max_chars: Option<usize>,
        }
        match name {
            "intelligence_context" => {
                let args: Context =
                    serde_json::from_value(args.clone()).map_err(|_| ErrorCode::InvalidRequest)?;
                serde_json::to_value(
                    self.context(
                        &args.repo,
                        &args.query,
                        args.limit.unwrap_or(5),
                        args.max_chars.unwrap_or(12_000),
                    )
                    .await?,
                )
                .map_err(|_| ErrorCode::InvalidResponse)
            }
            "intelligence_evidence" => {
                let args: Evidence =
                    serde_json::from_value(args.clone()).map_err(|_| ErrorCode::InvalidRequest)?;
                if args.repo != args.handle.project {
                    return Err(ErrorCode::Denied);
                }
                serde_json::to_value(
                    self.evidence(&args.handle, args.max_chars.unwrap_or(12_000))
                        .await?,
                )
                .map_err(|_| ErrorCode::InvalidResponse)
            }
            _ => Err(ErrorCode::InvalidRequest),
        }
    }
}

fn validate_record(
    record: &Record,
    identity: &Identity,
    project: &str,
    snapshot: &str,
) -> Result<(), ErrorCode> {
    let handle = &record.handle;
    let qualified = |id: &str| format!("{}::{id}", identity.provider);
    if handle.provider != identity.provider
        || handle.owner != identity.owner
        || handle.scope != identity.scope
        || handle.project != project
        || handle.snapshot != snapshot
        || handle.id != record.id
        || !record.id.strip_prefix("intel:").is_some_and(hash)
        || record.qualified_id != qualified(&record.id)
        || record.statement.trim().is_empty()
        || !(1..=2).contains(&record.citations.len())
        || record.evidence_count < record.citations.len()
        || record.assessment_count == 0
        || record.interpretation_class != chaosbox_core::EvidenceClass::Inferred
        || !matches!(
            record.status,
            chaosbox_core::intelligence::IntelligenceStatus::Admitted
                | chaosbox_core::intelligence::IntelligenceStatus::Disputed
        )
        || record.citations.iter().any(|c| {
            c.line == 0
                || !hash(&c.snapshot)
                || c.lineage
                    != chaosbox_core::sha256_hex(&[&c.source, &c.session, &c.message, &c.pointer])
        })
        || record
            .contradicts
            .iter()
            .chain(&record.supersedes)
            .any(|id| {
                !id.strip_prefix(&format!("{}::intel:", identity.provider))
                    .is_some_and(hash)
            })
    {
        return Err(ErrorCode::InvalidResponse);
    }
    Ok(())
}

fn validate_packet(
    packet: &ContextPacket,
    identity: &Identity,
    args: &ContextRequest,
) -> Result<(), ErrorCode> {
    if packet.identity != *identity
        || packet.project != args.project
        || !hash(&packet.snapshot)
        || !hash(&packet.policy_version)
        || packet.records.len() > args.limit
        || chars(&packet.records)? > args.max_chars
    {
        return Err(ErrorCode::InvalidResponse);
    }
    let mut seen = BTreeSet::new();
    for record in &packet.records {
        validate_record(record, identity, &args.project, &packet.snapshot)?;
        if !seen.insert(&record.id) || !record.same_origin.is_empty() {
            return Err(ErrorCode::InvalidResponse);
        }
    }
    Ok(())
}

fn assemble(
    args: &ContextRequest,
    results: Vec<(Identity, Result<Reply, ErrorCode>)>,
) -> Result<Packet, ErrorCode> {
    let mut sources = Vec::new();
    let mut groups = Vec::new();
    for (identity, result) in results {
        let result = match result {
            Ok(Reply::Context(packet)) => {
                validate_packet(&packet, &identity, args).map(|()| packet)
            }
            Ok(_) => Err(ErrorCode::InvalidResponse),
            Err(error) => Err(error),
        };
        match result {
            Ok(packet) => {
                sources.push(SourceStatus {
                    identity,
                    snapshot: Some(packet.snapshot),
                    policy_version: Some(packet.policy_version),
                    error: None,
                    omitted: packet.omitted,
                });
                groups.push(packet.records);
            }
            Err(error) => sources.push(SourceStatus {
                identity,
                snapshot: None,
                policy_version: None,
                error: Some(error),
                omitted: 0,
            }),
        }
    }
    let total: usize = groups.iter().map(Vec::len).sum();
    let mut records = Vec::new();
    // Interleave local then peer matches instead of letting local consume every slot.
    for position in 0..args.limit {
        for group in &groups {
            if records.len() == args.limit {
                break;
            }
            if let Some(record) = group.get(position) {
                let mut proposed = records.clone();
                proposed.push(record.clone());
                group_origins(&mut proposed);
                if chars(&proposed)? <= args.max_chars {
                    records = proposed;
                }
            }
        }
    }
    let snapshot = chaosbox_core::sha256_hex(&[
        "federation-view-v1",
        &serde_json::to_string(&(&sources, &records)).map_err(|_| ErrorCode::InvalidResponse)?,
    ]);
    Ok(Packet {
        version: 1,
        project: args.project.clone(),
        snapshot,
        omitted: total - records.len(),
        records,
        degraded: sources.iter().any(|s| s.error.is_some()),
        sources,
        exhaustive: false,
        historical_data_not_instructions: true,
    })
}

fn group_origins(records: &mut [Record]) {
    let mut origins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let key = |r: &Record| {
        let primary = &r.citations[0];
        serde_json::to_string(&(
            &r.statement,
            r.kind,
            r.status,
            &primary.lineage,
            primary.line,
            &primary.speaker,
        ))
        .unwrap_or_default()
    };
    for record in records.iter() {
        origins
            .entry(key(record))
            .or_default()
            .push(record.qualified_id.clone());
    }
    for record in records {
        record.same_origin = origins[&key(record)]
            .iter()
            .filter(|id| **id != record.qualified_id)
            .cloned()
            .collect();
    }
}
