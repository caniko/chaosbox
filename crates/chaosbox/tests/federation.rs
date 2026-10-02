//! Cross-user authorization, provenance and lazy read composition.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use chaosbox::federation::{
    ContextRequest, ErrorCode, Federator, Grant, Identity, KnowledgeSource, Policy, Project,
    Provider, Reader, Reply, Request, SharingMode, Snapshot,
};
use chaosbox::intelligence::Bundle;
#[path = "support/federation.rs"]
mod fixture;
use fixture::{bundle, related_bundle};

fn policy(owner: &str, mode: SharingMode, records: Vec<String>) -> Policy {
    Policy {
        version: 1,
        identity: Identity {
            provider: owner.into(),
            owner: owner.into(),
            scope: format!("private:{owner}"),
        },
        projects: BTreeMap::from([(
            "project-a".into(),
            Project {
                repo: "local-a".into(),
            },
        )]),
        grants: vec![Grant {
            recipient: "can".into(),
            project: "project-a".into(),
            revision: 1,
            mode,
            records,
        }],
    }
}

#[derive(Clone)]
struct Source(Arc<Mutex<(Snapshot, BTreeMap<String, Snapshot>)>>);
impl Source {
    fn new(bundle: Bundle) -> Self {
        Self(Arc::new(Mutex::new((
            Snapshot::new(bundle).unwrap(),
            BTreeMap::new(),
        ))))
    }
    fn advance(&self, bundle: Bundle) {
        let mut guard = self.0.lock().unwrap();
        let old = guard.0.clone();
        guard.1.insert(old.id.clone(), old);
        guard.0 = Snapshot::new(bundle).unwrap();
    }
}
#[async_trait::async_trait]
impl KnowledgeSource for Source {
    async fn current(&self, _scope: &str) -> Result<Snapshot, ErrorCode> {
        Ok(self.0.lock().unwrap().0.clone())
    }
    async fn snapshot(&self, _scope: &str, id: &str) -> Result<Option<Snapshot>, ErrorCode> {
        let guard = self.0.lock().unwrap();
        Ok(if guard.0.id == id {
            Some(guard.0.clone())
        } else {
            guard.1.get(id).cloned()
        })
    }
}

fn request(limit: usize, max_chars: usize) -> Request {
    Request::Context(ContextRequest {
        project: "project-a".into(),
        query: "bounded context citations".into(),
        limit,
        max_chars,
    })
}

#[tokio::test]
async fn selected_is_default_and_eligibility_precedes_ranking() {
    let bundle = bundle(
        "dejana",
        &[
            (
                "private",
                "We must use bounded context citations for private context.",
                "local-a",
            ),
            ("selected", "We must preserve context citations.", "local-a"),
            (
                "other",
                "We must use bounded context citations for another project.",
                "local-b",
            ),
        ],
    );
    let selected = bundle.records[1].id.clone();
    let mut reader = Reader::new(
        policy("dejana", SharingMode::Selected, vec![selected.clone()]),
        Source::new(bundle),
    )
    .unwrap();
    let Reply::Context(packet) = reader.query("can", &request(1, 12_000)).await.unwrap() else {
        panic!("context required")
    };
    assert_eq!(packet.records.len(), 1);
    assert_eq!(packet.records[0].id, selected);
    assert_eq!(
        reader
            .query("stranger", &request(5, 12_000))
            .await
            .unwrap_err(),
        ErrorCode::Denied
    );
    reader.policy.grants[0].records.clear();
    let Reply::Context(empty) = reader.query("can", &request(5, 12_000)).await.unwrap() else {
        panic!("context required")
    };
    assert!(empty.records.is_empty());
    let json = serde_json::to_value(&reader.policy.grants[0]).unwrap();
    let mut without_mode = json;
    without_mode.as_object_mut().unwrap().remove("mode");
    assert_eq!(
        serde_json::from_value::<Grant>(without_mode).unwrap().mode,
        SharingMode::Selected
    );
}

#[tokio::test]
async fn evidence_is_snapshot_pinned_and_rechecks_current_grants() {
    let old = bundle(
        "dejana",
        &[(
            "shared",
            "We must preserve bounded context citations.",
            "local-a",
        )],
    );
    let source = Source::new(old.clone());
    let mut reader = Reader::new(
        policy("dejana", SharingMode::AllAdmitted, vec![]),
        source.clone(),
    )
    .unwrap();
    let Reply::Context(packet) = reader.query("can", &request(5, 12_000)).await.unwrap() else {
        panic!("context required")
    };
    let handle = packet.records[0].handle.clone();
    let mut next = bundle(
        "dejana",
        &[
            (
                "shared",
                "We must preserve bounded context citations.",
                "local-a",
            ),
            ("new", "We must keep new context bounded.", "local-a"),
        ],
    );
    source.advance(next.clone());
    let evidence_request = Request::Evidence {
        handle: handle.clone(),
        max_chars: 12_000,
    };
    let Reply::Evidence(evidence) = reader.query("can", &evidence_request).await.unwrap() else {
        panic!("evidence required")
    };
    assert_eq!(evidence.record.handle.snapshot, handle.snapshot);
    assert!(!evidence.evidence.is_empty());
    assert!(evidence
        .receipts
        .iter()
        .all(|receipt| receipt.state_omitted));
    reader.policy.grants.clear();
    assert_eq!(
        reader.query("can", &evidence_request).await.unwrap_err(),
        ErrorCode::Denied
    );
    reader.policy.grants.push(Grant {
        recipient: "can".into(),
        project: "project-a".into(),
        revision: 2,
        mode: SharingMode::AllAdmitted,
        records: vec![],
    });
    let mut unavailable = handle;
    unavailable.snapshot = "0".repeat(64);
    assert_eq!(
        reader
            .query(
                "can",
                &Request::Evidence {
                    handle: unavailable,
                    max_chars: 12_000
                }
            )
            .await
            .unwrap_err(),
        ErrorCode::SnapshotUnavailable
    );
    fixture::withhold(
        &mut next,
        "shared",
        "We must preserve bounded context citations.",
        "local-a",
    );
    source.advance(next);
    assert_eq!(
        reader.query("can", &evidence_request).await.unwrap_err(),
        ErrorCode::Denied
    );
}

#[tokio::test]
async fn raw_receipts_and_unshared_relationships_never_cross_the_boundary() {
    let bundle = bundle(
        "dejana",
        &[
            ("other", "We must keep SECRET-PROJECT-B private.", "local-b"),
            (
                "shared",
                "We must preserve bounded context citations.",
                "local-a",
            ),
        ],
    );
    assert!(serde_json::to_string(&bundle.assessments)
        .unwrap()
        .contains("SECRET-PROJECT-B"));
    let reader = Reader::new(
        policy("dejana", SharingMode::AllAdmitted, vec![]),
        Source::new(bundle),
    )
    .unwrap();
    let Reply::Context(packet) = reader.query("can", &request(5, 12_000)).await.unwrap() else {
        panic!("context required")
    };
    let result = reader
        .query(
            "can",
            &Request::Evidence {
                handle: packet.records[0].handle.clone(),
                max_chars: 12_000,
            },
        )
        .await
        .unwrap();
    let json = serde_json::to_string(&result).unwrap();
    assert!(!json.contains("SECRET-PROJECT-B"));
    assert!(!json.contains("local-b"));
    assert!(json.contains("state_omitted"));
}

struct Endpoint {
    reader: Reader<Source>,
    calls: Arc<Mutex<Vec<String>>>,
    unavailable: bool,
}
#[async_trait::async_trait]
impl Provider for Endpoint {
    fn identity(&self) -> &Identity {
        &self.reader.policy.identity
    }
    async fn query(&self, request: &Request) -> Result<Reply, ErrorCode> {
        self.calls
            .lock()
            .unwrap()
            .push(self.identity().provider.clone());
        if self.unavailable {
            return Err(ErrorCode::Unavailable);
        }
        self.reader.query("can", request).await
    }
}

#[tokio::test]
async fn local_matches_do_not_skip_peers_and_outage_preserves_local_results() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let make = |owner: &str, unavailable| {
        Arc::new(Endpoint {
            calls: calls.clone(),
            unavailable,
            reader: Reader::new(
                policy(owner, SharingMode::AllAdmitted, vec![]),
                Source::new(bundle(
                    owner,
                    &[(
                        "shared",
                        "We must preserve bounded context citations.",
                        "local-a",
                    )],
                )),
            )
            .unwrap(),
        }) as Arc<dyn Provider>
    };
    let reader = Federator::new(make("can", false), vec![make("dejana", false)], 1000).unwrap();
    let result = reader
        .context("project-a", "context citations", 5, 12_000)
        .await
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), ["can", "dejana"]);
    assert_eq!(result.records.len(), 2);
    assert_ne!(
        result.records[0].qualified_id,
        result.records[1].qualified_id
    );
    assert_eq!(result.records[0].same_origin.len(), 1);
    assert!(!result.degraded);
    let reader = Federator::new(make("can", false), vec![make("dejana", true)], 1000).unwrap();
    let partial = reader
        .context("project-a", "context citations", 5, 12_000)
        .await
        .unwrap();
    assert_eq!(partial.records.len(), 1);
    assert!(partial.degraded);
    assert_eq!(partial.sources[1].error, Some(ErrorCode::Unavailable));
    assert!(
        serde_json::to_string(&partial.records)
            .unwrap()
            .chars()
            .count()
            <= 12_000
    );
}

fn config_files(
    dir: &std::path::Path,
    owner: &str,
) -> (std::path::PathBuf, std::path::PathBuf, Bundle) {
    let bundle = bundle(
        owner,
        &[(
            "shared",
            "We must preserve bounded context citations.",
            "local-a",
        )],
    );
    let artifact = dir.join("bundle.json");
    std::fs::write(&artifact, serde_json::to_vec(&bundle).unwrap()).unwrap();
    let provider = dir.join("provider.json");
    std::fs::write(
        &provider,
        serde_json::to_vec(&chaosbox::federation::ProviderConfig {
            policy: policy(owner, SharingMode::AllAdmitted, vec![]),
            backend: chaosbox::federation::Backend::Bundle {
                path: artifact,
                history: None,
            },
        })
        .unwrap(),
    )
    .unwrap();
    let client = dir.join("client.json");
    std::fs::write(
        &client,
        serde_json::to_vec(&chaosbox::federation::ClientConfig {
            version: 1,
            local: provider.clone(),
            peers: vec![],
            timeout_ms: 1000,
        })
        .unwrap(),
    )
    .unwrap();
    (provider, client, bundle)
}

fn process(args: &[&str], input: &str) -> serde_json::Value {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(args)
        .env_remove("SSH_ORIGINAL_COMMAND")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_context_and_evidence_share_the_exact_handle_contract() {
    let dir = tempfile::tempdir().unwrap();
    let (_, client, _) = config_files(dir.path(), "can");
    let config = client.to_str().unwrap();
    let context = process(
        &[
            "federation",
            "--config",
            config,
            "context",
            "--repo",
            "project-a",
            "context citations",
        ],
        "",
    );
    assert_eq!(context["records"].as_array().unwrap().len(), 1);
    let handle = context["records"][0]["handle"].to_string();
    let evidence = process(
        &[
            "federation",
            "--config",
            config,
            "evidence",
            "--repo",
            "project-a",
            "--handle",
            &handle,
        ],
        "",
    );
    assert_eq!(
        evidence["record"]["handle"],
        context["records"][0]["handle"]
    );
    assert_eq!(evidence["is_projection"], true);
}

#[test]
fn wire_requests_cannot_claim_a_caller_change_owner_or_select_a_backend() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, _, _) = config_files(dir.path(), "dejana");
    let args = [
        "federation",
        "--config",
        provider.to_str().unwrap(),
        "serve",
        "--caller",
        "can",
    ];
    let valid = serde_json::json!({"version":1,"request":{"operation":"context","project":"project-a","query":"context citations","limit":5,"max_chars":12000}});
    let output = process(&args, &format!("{valid}\n"));
    assert_eq!(output["result"]["status"], "ok");
    for (key, value) in [
        ("caller", "dejana"),
        ("backend", "/private/secret.json"),
        ("scope", "private:can"),
    ] {
        let mut malicious = valid.clone();
        malicious["request"][key] = value.into();
        assert_eq!(
            process(&args, &format!("{malicious}\n"))["result"]["code"],
            "invalid_request"
        );
    }
    let mut unrelated = valid.clone();
    unrelated["request"]["project"] = "project-b".into();
    assert_eq!(
        process(&args, &format!("{unrelated}\n"))["result"]["code"],
        "denied"
    );
    let mut unknown = valid;
    unknown["version"] = 2.into();
    assert_eq!(
        process(&args, &format!("{unknown}\n"))["result"]["code"],
        "invalid_request"
    );
    assert_eq!(
        process(&args, &format!("{}\n", "x".repeat(8193)))["result"]["code"],
        "invalid_request"
    );
}

#[tokio::test]
async fn running_file_provider_observes_revocation_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _, _) = config_files(dir.path(), "dejana");
    let provider = chaosbox::federation::FileProvider::new(&path, Some("can")).unwrap();
    let Reply::Context(packet) = provider.query(&request(5, 12000)).await.unwrap() else {
        panic!("context required")
    };
    let handle = packet.records[0].handle.clone();
    let mut config: chaosbox::federation::ProviderConfig =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config.policy.grants.clear();
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert_eq!(
        provider.query(&request(5, 12000)).await.unwrap_err(),
        ErrorCode::Denied
    );
    assert_eq!(
        provider
            .query(&Request::Evidence {
                handle,
                max_chars: 12000
            })
            .await
            .unwrap_err(),
        ErrorCode::Denied
    );
}

#[test]
fn mcp_exposes_and_executes_snapshot_bound_federation_tools() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let (_, client, _) = config_files(dir.path(), "can");
    let input = [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"cursor":"10"}}),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"intelligence_context","arguments":{"repo":"project-a","query":"context citations"}}}),
    ].into_iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n") + "\n";
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_chaosbox"))
        .args(["mcp", "--intelligence-federation", client.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let replies: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let evidence = tools
        .iter()
        .find(|tool| tool["name"] == "intelligence_evidence")
        .unwrap();
    assert!(evidence["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("handle")));
    let packet: serde_json::Value =
        serde_json::from_str(replies[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(packet["sources"][0]["identity"]["owner"], "can");
    assert_eq!(packet["records"].as_array().unwrap().len(), 1);
}

struct Slow(Identity);
#[async_trait::async_trait]
impl Provider for Slow {
    fn identity(&self) -> &Identity {
        &self.0
    }
    async fn query(&self, _request: &Request) -> Result<Reply, ErrorCode> {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Err(ErrorCode::Unavailable)
    }
}

#[tokio::test]
async fn total_deadline_reserves_time_for_peers_when_local_stalls() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let peer = Endpoint {
        calls: calls.clone(),
        unavailable: false,
        reader: Reader::new(
            policy("dejana", SharingMode::AllAdmitted, vec![]),
            Source::new(bundle(
                "dejana",
                &[(
                    "shared",
                    "We must preserve bounded context citations.",
                    "local-a",
                )],
            )),
        )
        .unwrap(),
    };
    let local = Slow(policy("can", SharingMode::AllAdmitted, vec![]).identity);
    let reader = Federator::new(Arc::new(local), vec![Arc::new(peer)], 100).unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        reader.context("project-a", "context citations", 5, 12000),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.sources[0].error, Some(ErrorCode::Unavailable));
    assert_eq!(result.records[0].handle.owner, "dejana");
    assert_eq!(*calls.lock().unwrap(), ["dejana"]);
}

#[tokio::test]
async fn authorized_disputes_remain_visible_without_leaking_unselected_ids() {
    let bundle = related_bundle(
        "dejana",
        &[
            (
                "first",
                "We must use manual context citations for this project.",
                "local-a",
            ),
            (
                "second",
                "We must use automatic context citations for this project.",
                "local-a",
            ),
        ],
        Some("contradicts"),
    );
    let hidden = bundle.records[0].id.clone();
    let selected = bundle.records[1].id.clone();
    let reader = Reader::new(
        policy("dejana", SharingMode::Selected, vec![selected]),
        Source::new(bundle),
    )
    .unwrap();
    let Reply::Context(packet) = reader.query("can", &request(5, 12000)).await.unwrap() else {
        panic!("context required")
    };
    assert_eq!(
        packet.records[0].status,
        chaosbox_core::intelligence::IntelligenceStatus::Disputed
    );
    assert!(packet.records[0].contradicts.is_empty());
    assert_eq!(packet.records[0].omitted_relationships, 1);
    let evidence = reader
        .query(
            "can",
            &Request::Evidence {
                handle: packet.records[0].handle.clone(),
                max_chars: 12000,
            },
        )
        .await
        .unwrap();
    assert!(!serde_json::to_string(&evidence).unwrap().contains(&hidden));
}

#[tokio::test]
async fn both_sides_of_disagreement_and_different_local_repo_mappings_are_retained() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let local = Endpoint {
        calls: calls.clone(),
        unavailable: false,
        reader: Reader::new(
            policy("can", SharingMode::AllAdmitted, vec![]),
            Source::new(bundle(
                "can",
                &[("first", "We must use manual context citations.", "local-a")],
            )),
        )
        .unwrap(),
    };
    let mut peer_policy = policy("dejana", SharingMode::AllAdmitted, vec![]);
    peer_policy.projects.get_mut("project-a").unwrap().repo = "dejana-repo-name".into();
    let peer = Endpoint {
        calls,
        unavailable: false,
        reader: Reader::new(
            peer_policy,
            Source::new(bundle(
                "dejana",
                &[(
                    "first",
                    "We must use automatic context citations.",
                    "dejana-repo-name",
                )],
            )),
        )
        .unwrap(),
    };
    let reader = Federator::new(Arc::new(local), vec![Arc::new(peer)], 1000).unwrap();
    let result = reader
        .context("project-a", "context citations", 2, 12000)
        .await
        .unwrap();
    assert_eq!(result.records.len(), 2);
    assert!(result.records[0].statement.contains("manual"));
    assert!(result.records[1].statement.contains("automatic"));
    assert!(result.records.iter().all(|r| r.same_origin.is_empty()));
    let bounded = reader
        .context("project-a", "context citations", 2, 1800)
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&bounded.records)
            .unwrap()
            .chars()
            .count()
            <= 1800
    );
    assert!(bounded.omitted > 0);
}

struct Invalid(Identity, Reply);
#[async_trait::async_trait]
impl Provider for Invalid {
    fn identity(&self) -> &Identity {
        &self.0
    }
    async fn query(&self, _request: &Request) -> Result<Reply, ErrorCode> {
        Ok(self.1.clone())
    }
}

#[tokio::test]
async fn malformed_peer_provenance_is_rejected_while_local_results_survive() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let local = Endpoint {
        calls,
        unavailable: false,
        reader: Reader::new(
            policy("can", SharingMode::AllAdmitted, vec![]),
            Source::new(bundle(
                "can",
                &[(
                    "first",
                    "We must preserve bounded context citations.",
                    "local-a",
                )],
            )),
        )
        .unwrap(),
    };
    let peer = Reader::new(
        policy("dejana", SharingMode::AllAdmitted, vec![]),
        Source::new(bundle(
            "dejana",
            &[(
                "first",
                "We must preserve bounded context citations.",
                "local-a",
            )],
        )),
    )
    .unwrap();
    let Reply::Context(mut packet) = peer.query("can", &request(5, 12000)).await.unwrap() else {
        panic!("context required")
    };
    packet.records[0].handle.snapshot = "0".repeat(64);
    let reader = Federator::new(
        Arc::new(local),
        vec![Arc::new(Invalid(
            peer.policy.identity,
            Reply::Context(packet),
        ))],
        1000,
    )
    .unwrap();
    let result = reader
        .context("project-a", "context citations", 5, 12000)
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.sources[1].error, Some(ErrorCode::InvalidResponse));
}

#[tokio::test]
async fn ssh_connection_failure_is_unavailable_rather_than_a_caller_error() {
    use tokio::io::AsyncWriteExt;
    // Disposable listener closes before an SSH handshake; no login or keys are used.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let ssh_config = dir.path().join("ssh-config");
    std::fs::write(
        &ssh_config,
        "Host federation-fixture\n  HostName 127.0.0.1\n  ControlPath none\n",
    )
    .unwrap();
    let peer = chaosbox::federation::Peer {
        identity: policy("dejana", SharingMode::AllAdmitted, vec![]).identity,
        destination: "federation-fixture".into(),
        port,
        projects: vec!["project-a".into()],
        ssh_config: Some(ssh_config),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        peer.query(&request(5, 12000)),
    )
    .await
    .unwrap();
    // Reaching this listener proves the isolated -F config resolved the alias.
    tokio::time::timeout(std::time::Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), ErrorCode::Unavailable);
}
