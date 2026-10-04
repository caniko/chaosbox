//! Enrollment, publication, automatic peer contact, and bounded live reconciliation.
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use clap::Subcommand;
use serde_json::{Value, json};
use chaosbox_core::sha256_hex;
use chaosbox_store::ReplicaStore;
use chaosbox_typedb::store::{TypeDbStore, TypeDbConfig};
use chaosbox_jev::{JevClient, JevPolicy};
use super::{
    settings, identity, Authority, Grant, Identity, Publication, SignedEvent, Payload, transport,
    load_replica, persist_event, publish_current, publication_from_sources, reconcile,
    ReconcileBudget, current::CurrentReader,
};
pub use super::settings::Settings;

/// Operator and read-only peer intelligence commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a device, optionally under an existing user-root public key.
    Init {
        /// Explicit private scope to replicate.
        #[arg(long)]
        scope: String,
        /// Existing user-root key. Omit only when creating the first user/device.
        #[arg(long)]
        user: Option<String>,
    },
    /// Offline enrollment authority signs a new device's public key.
    Authorize {
        /// New device key returned by its init command.
        device: String,
        /// Private output grant file; transfer this public certificate to that device.
        #[arg(long)]
        output: PathBuf,
    },
    /// Install a root-signed grant matching this device and user.
    Enroll {
        /// Certificate produced by authorize.
        grant: PathBuf,
    },
    /// Add an authenticated SSH endpoint, including a LAN/VPN host alias.
    Peer {
        /// SSH host alias or user@host.
        #[arg(long)]
        target: String,
        /// Enrolled device public key to expect at this endpoint.
        #[arg(long)]
        device: String,
        /// SSH port.
        #[arg(long, default_value_t = 22)]
        port: u16,
    },
    /// Configure an existing admitted custody outbox for automatic publication.
    Configure {
        /// Existing private memory archive, watched by serve.
        #[arg(long)]
        archive: Option<PathBuf>,
    },
    /// Verify original normalized sources and publish a bundle for peer replication.
    Publish {
        /// Existing assessed bundle/manifest.
        bundle: PathBuf,
        /// Original source shards needed by retained evidence; never transferred whole.
        #[arg(long, required = true)]
        source_jsonl: Vec<PathBuf>,
    },
    /// Publish the admitted outbox of an existing custody archive.
    PublishMemory {
        /// Existing archive created by memory capture/drain.
        #[arg(long)]
        work: PathBuf,
    },
    /// Bidirectional exchange with every configured reachable peer.
    Once,
    /// Remote stdio exchange endpoint; invoked by SSH with this constant command.
    Exchange,
    /// Automatically publish an archive outbox and contact peers until interrupted.
    Serve {
        /// Peer reconnect/publication polling interval.
        #[arg(long, default_value_t = 30)]
        interval_seconds: u64,
        /// Enable explicitly budgeted Jev semantic reconciliation in the worker.
        #[arg(long)]
        live_jev: bool,
        /// Persistent per-device budget epoch.
        #[arg(long, default_value = "reconcile-v1")]
        budget: String,
        /// Cumulative dispatch ceiling in this budget epoch.
        #[arg(long, default_value_t = 100)]
        max_requests: u32,
        /// Cumulative conservative input-token ceiling in this budget epoch.
        #[arg(long, default_value_t = 1_000_000)]
        max_input_tokens: u64,
    },
    /// Explicit bounded Jev reconciliation; peer transport itself never performs inference.
    Reconcile {
        /// Required permission to dispatch pinned Jev requests.
        #[arg(long, required = true)]
        live_jev: bool,
        /// Persistent per-device budget epoch.
        #[arg(long, default_value = "reconcile-v1")]
        budget: String,
        /// Cumulative dispatch ceiling.
        #[arg(long, default_value_t = 100)]
        max_requests: u32,
        /// Cumulative conservative input-token ceiling.
        #[arg(long, default_value_t = 1_000_000)]
        max_input_tokens: u64,
        /// Jobs processed by this invocation.
        #[arg(long, default_value_t = 20)]
        max_jobs: usize,
        /// Authorize new spending after failed/unknown attempts.
        #[arg(long)]
        retry: bool,
    },
    /// Read local frontier, staged dependencies and semantic backlog without inference.
    Status,
    /// Read the newest local synchronized context snapshot.
    Context {
        /// Repository applicability filter.
        #[arg(long)]
        repo: String,
        /// Task terms.
        query: String,
        /// Maximum returned records.
        #[arg(long, default_value_t = 5)]
        limit: usize,
        /// Maximum record characters.
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Inspect item sources and replicated semantic relationships.
    Evidence {
        /// Explicit repository applicability.
        #[arg(long)]
        repo: String,
        /// Intelligence identity.
        id: String,
    },
}

fn backend() -> Result<TypeDbStore, String> {
    Ok(TypeDbStore::new(TypeDbConfig::from_env()?))
}

/// Create a read-only live consumer using public owner/scope settings only.
pub fn current_reader(directory: &std::path::Path) -> Result<CurrentReader, String> {
    let settings = Settings::load(directory)?;
    Ok(CurrentReader::new(
        Box::new(backend()?),
        &settings.user,
        &settings.scope,
    ))
}

async fn publish(
    store: &mut TypeDbStore,
    settings: &Settings,
    identity: &Identity,
    publication: Publication,
) -> Result<Value, String> {
    if publication.bundle.scope != settings.scope {
        return Err("source outbox visibility differs from enrolled scope".into());
    }
    let mut replica = load_replica(store, &settings.user, &settings.scope).await?;
    let bytes = serde_json::to_string(&publication).map_err(|_| "encode publication")?;
    let already = replica.events.values().any(|e| matches!(&e.payload,Payload::Publication(p) if serde_json::to_string(p).is_ok_and(|old| old == bytes)));
    if !already {
        let event = SignedEvent::publication(identity, replica.heads(), publication)?;
        replica.receive(event.clone())?;
        replica.view()?;
        persist_event(store, &settings.user, &settings.scope, &event).await?;
    }
    let view = publish_current(store, &settings.user, &settings.scope).await?;
    Ok(json!({"snapshot":view.digest,"records":view.bundle.records.len(),"reused":already}))
}

async fn contact(store: &mut TypeDbStore, settings: &Settings, identity: &Identity) -> Value {
    let mut reports = Vec::new();
    for peer in &settings.peers {
        let result = transport::connect(store, identity, &settings.scope, peer).await;
        reports.push(json!({"device":peer.device,"target":peer.target,"synced":result.is_ok(),"error":result.err()}));
    }
    json!({"peers":reports,"globally_current":false})
}

async fn assess(
    store: &mut TypeDbStore,
    settings: &Settings,
    identity: &Identity,
    budget: ReconcileBudget,
) -> Result<Value, String> {
    let mut responder = crate::LiveResponder::new(
        JevClient::new(JevPolicy {
            max_requests: budget.requests,
            max_input_tokens: budget.input_tokens,
            max_retries: 0,
            ..JevPolicy::default()
        })
        .map_err(|e| e.to_string())?,
    );
    let result = reconcile(store, identity, &settings.scope, &mut responder, &budget).await;
    let (requests, input_tokens) = responder.usage();
    eprintln!("usage: requests={requests} input_tokens={input_tokens}");
    result.map(
        |completed| json!({"completed":completed,"requests":requests,"input_tokens":input_tokens}),
    )
}

/// Run one operator command. Read-only commands do not load device or model keys.
pub async fn run(directory: Option<PathBuf>, command: Command) -> Result<Value, String> {
    let directory = directory.map_or_else(settings::default_directory, Ok)?;
    match command {
        Command::Init { scope, user } => init(&directory, scope, user),
        command => {
            let mut settings = Settings::load(&directory)?;
            match command {
                Command::Authorize { device, output } => {
                    let authority = Authority::from_pkcs8(settings::read_bytes(
                        &directory.join("authority.pk8"),
                        4096,
                        true,
                    )?)?;
                    let grant = authority.authorize(&device, vec![settings.scope])?;
                    if grant.user != settings.user {
                        return Err("enrollment authority does not match configured user".into());
                    }
                    settings::write_json(&output, &grant, false)?;
                    Ok(json!({"grant":output,"device":device}))
                }
                Command::Enroll { grant } => {
                    let grant: Grant = settings::read_json(&grant)?;
                    grant.validate(&settings.user, &settings.scope)?;
                    Identity::from_pkcs8(
                        &settings::read_bytes(&directory.join("device.pk8"), 4096, true)?,
                        grant.clone(),
                        &settings.scope,
                    )?;
                    settings::write_json(&directory.join("grant.json"), &grant, true)?;
                    Ok(json!({"enrolled":true,"device":grant.device}))
                }
                Command::Peer {
                    target,
                    device,
                    port,
                } => {
                    let peer = transport::Peer {
                        target,
                        port,
                        device,
                    };
                    peer.validate()?;
                    settings.peers.retain(|p| p.device != peer.device);
                    settings.peers.push(peer);
                    if settings.peers.len() > 256 {
                        return Err("peer capacity exceeded".into());
                    }
                    settings::write_json(&directory.join("config.json"), &settings, true)?;
                    Ok(json!({"peers":settings.peers.len()}))
                }
                Command::Configure { archive } => {
                    if let Some(path) = archive {
                        if !path.join("memory.sqlite").is_file() {
                            return Err("configure an existing custody archive".into());
                        }
                        settings.archive = Some(path);
                    }
                    settings::write_json(&directory.join("config.json"), &settings, true)?;
                    Ok(json!({"archive":settings.archive}))
                }
                Command::Context {
                    repo,
                    query,
                    limit,
                    max_chars,
                } => {
                    current_reader(&directory)?
                        .query(
                            "intelligence_context",
                            &json!({"repo":repo,"query":query,"limit":limit,"max_chars":max_chars}),
                        )
                        .await
                }
                Command::Evidence { repo, id } => {
                    current_reader(&directory)?
                        .query("intelligence_evidence", &json!({"repo":repo,"id":id}))
                        .await
                }
                Command::Status => status(&settings).await,
                command => run_signed(&directory, &settings, command).await,
            }
        }
    }
}

fn init(directory: &std::path::Path, scope: String, user: Option<String>) -> Result<Value, String> {
    if !scope.starts_with("private:") || scope.len() <= 8 || scope.len() > 256 {
        return Err("private scope required (maximum 256 bytes)".into());
    }
    settings::private_directory(directory)?;
    if directory.join("config.json").exists() || directory.join("device.pk8").exists() {
        return Err("sync identity already exists".into());
    }
    let private = identity::generate_key()?;
    let device = identity::public_key(&private)?;
    let (user, grant) = if let Some(user) = user {
        if !super::is_digest(&user) {
            return Err("invalid user-root public key".into());
        }
        (user, None)
    } else {
        let authority = Authority::from_pkcs8(identity::generate_key()?)?;
        let grant = authority.authorize(&device, vec![scope.clone()])?;
        settings::write_bytes(
            &directory.join("authority.pk8"),
            authority.private_bytes(),
            false,
        )?;
        (grant.user.clone(), Some(grant))
    };
    settings::write_bytes(&directory.join("device.pk8"), &private, false)?;
    let configured = Settings {
        version: 1,
        user: user.clone(),
        scope,
        peers: vec![],
        archive: None,
    };
    settings::write_json(&directory.join("config.json"), &configured, false)?;
    if let Some(grant) = &grant {
        settings::write_json(&directory.join("grant.json"), grant, false)?;
    }
    Ok(json!({"user":user,"device":device,"enrolled":grant.is_some(),"directory":directory}))
}

async fn run_signed(
    directory: &std::path::Path,
    settings: &Settings,
    command: Command,
) -> Result<Value, String> {
    let identity = settings.identity(directory)?;
    let _lease = if matches!(
        command,
        Command::Serve { .. }
            | Command::Reconcile { .. }
            | Command::Publish { .. }
            | Command::PublishMemory { .. }
    ) {
        Some(settings::worker_lease(directory)?)
    } else {
        None
    };
    let mut store = backend()?;
    match command {
        Command::Publish {
            bundle,
            source_jsonl,
        } => {
            let publication = source_publication(&bundle, source_jsonl)?;
            publish(&mut store, settings, &identity, publication).await
        }
        Command::PublishMemory { work } => {
            let publication = crate::compaction::Journal::open(&work, &settings.scope)?
                .replication_publication()?;
            publish(&mut store, settings, &identity, publication).await
        }
        Command::Once => Ok(contact(&mut store, settings, &identity).await),
        Command::Exchange => {
            transport::serve(
                &mut store,
                &identity,
                &settings.scope,
                &mut tokio::io::stdin(),
                &mut tokio::io::stdout(),
            )
            .await?;
            Ok(Value::Null)
        }
        Command::Reconcile {
            live_jev,
            budget,
            max_requests,
            max_input_tokens,
            max_jobs,
            retry,
        } => {
            if !live_jev {
                return Err("reconciliation requires --live-jev".into());
            }
            assess(
                &mut store,
                settings,
                &identity,
                ReconcileBudget {
                    epoch: budget,
                    requests: max_requests,
                    input_tokens: max_input_tokens,
                    max_jobs,
                    retry,
                },
            )
            .await
        }
        Command::Serve {
            interval_seconds,
            live_jev,
            budget,
            max_requests,
            max_input_tokens,
        } => {
            serve(
                &mut store,
                settings,
                &identity,
                interval_seconds,
                live_jev.then_some(ReconcileBudget {
                    epoch: budget,
                    requests: max_requests,
                    input_tokens: max_input_tokens,
                    max_jobs: 20,
                    retry: false,
                }),
            )
            .await
        }
        _ => Err("invalid sync command dispatch".into()),
    }
}

fn source_publication(
    bundle: &std::path::Path,
    paths: Vec<PathBuf>,
) -> Result<Publication, String> {
    let mut sources = BTreeMap::new();
    let mut total = 0;
    for path in paths {
        let text = String::from_utf8(settings::read_bytes(&path, 32 * 1024 * 1024, false)?)
            .map_err(|_| "source must be UTF-8")?;
        total += text.len();
        if total > super::MAX_REPLICA_BYTES {
            return Err("source shard aggregate exceeds bounded import capacity".into());
        }
        sources.insert(sha256_hex(&[&text]), text);
    }
    publication_from_sources(crate::intelligence::cli::load_bundle(bundle)?, |hash| {
        sources
            .get(hash)
            .cloned()
            .ok_or_else(|| "missing original source shard".into())
    })
}

async fn status(settings: &Settings) -> Result<Value, String> {
    let mut store = backend()?;
    let replica = load_replica(&mut store, &settings.user, &settings.scope).await?;
    let published = store
        .replica_current(&settings.user, &settings.scope)
        .await
        .map_err(|e| e.to_string())?;
    let missing: std::collections::BTreeSet<_> = replica
        .events
        .values()
        .flat_map(|e| &e.parents)
        .filter(|id| !replica.events.contains_key(*id))
        .collect();
    let view = replica.view();
    let jobs = view
        .as_ref()
        .ok()
        .map(|v| super::reconcile_jobs(&replica, v, 200))
        .transpose()?
        .unwrap_or_default();
    Ok(
        json!({"user":settings.user,"scope":settings.scope,"events":replica.events.len(),"heads":replica.heads(),
        "missing_dependencies":missing,"pending_reconciliation_at_least":jobs.len(),"snapshot":published.as_ref().map(|(id,_)| id),
        "materializable_snapshot":view.as_ref().ok().map(|v| &v.digest),
        "publication_error":view.err(),"globally_current":false}),
    )
}

async fn cycle(
    store: &mut TypeDbStore,
    settings: &Settings,
    identity: &Identity,
    budget: Option<&ReconcileBudget>,
) {
    if let Some(work) = &settings.archive {
        let result = async {
            let publication = crate::compaction::Journal::open(work, &settings.scope)?
                .replication_publication()?;
            publish(store, settings, identity, publication).await
        }
        .await;
        eprintln!(
            "{}",
            json!({"operation":"archive-publication","result":result.as_ref().ok(),"error":result.err()})
        );
    }
    eprintln!("{}", contact(store, settings, identity).await);
    if let Some(budget) = budget {
        let result = assess(store, settings, identity, budget.clone()).await;
        eprintln!(
            "{}",
            json!({"operation":"reconcile","result":result.as_ref().ok(),"error":result.err()})
        );
    }
}

async fn serve(
    store: &mut TypeDbStore,
    settings: &Settings,
    identity: &Identity,
    interval: u64,
    budget: Option<ReconcileBudget>,
) -> Result<Value, String> {
    if !(1..=3600).contains(&interval) {
        return Err("worker interval 1..3600 seconds".into());
    }
    loop {
        tokio::select! { _ = tokio::signal::ctrl_c() => break, () = cycle(store,settings,identity,budget.as_ref()) => {} }
        tokio::select! { _ = tokio::signal::ctrl_c() => break, () = tokio::time::sleep(Duration::from_secs(interval)) => {} }
    }
    Ok(json!({"stopped":true}))
}
