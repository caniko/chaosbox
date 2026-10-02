//! Bounded one-request SSH transport. Recipient identity is fixed in the forced
//! server command, not claimed by the request. Fleet configuration owns routing.
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use super::{Backend, ErrorCode, Federator, Identity, Policy, Provider, Reader, Reply, Request};

const CONFIG_BYTES: u64 = 1024 * 1024;
const REQUEST_BYTES: u64 = 8192;
const RESPONSE_BYTES: u64 = 256 * 1024;

/// Owner-local endpoint configuration; credentials remain in its environment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Pinned identity and current directional project grants.
    pub policy: Policy,
    /// Owner-local storage adapter.
    pub backend: Backend,
}

fn load<T: DeserializeOwned>(path: &Path) -> Result<T, ErrorCode> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| ErrorCode::Unavailable)?
        .take(CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Unavailable)?;
    if bytes.len() as u64 > CONFIG_BYTES {
        return Err(ErrorCode::InvalidRequest);
    }
    serde_json::from_slice(&bytes).map_err(|_| ErrorCode::InvalidRequest)
}

/// File-backed provider reloading grants and backend settings on each request.
pub struct FileProvider {
    path: PathBuf,
    recipient: String,
    identity: Identity,
    projects: BTreeSet<String>,
}
impl FileProvider {
    /// The recipient must come from a trusted forced command or local owner.
    pub fn new(path: &Path, recipient: Option<&str>) -> Result<Self, ErrorCode> {
        let config: ProviderConfig = load(path)?;
        config.policy.validate()?;
        let recipient = recipient
            .unwrap_or(&config.policy.identity.owner)
            .to_owned();
        if recipient.is_empty() || recipient.len() > 128 {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(Self {
            path: path.to_owned(),
            recipient,
            identity: config.policy.identity,
            projects: config.policy.projects.into_keys().collect(),
        })
    }
}
#[async_trait::async_trait]
impl Provider for FileProvider {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn supports(&self, project: &str) -> bool {
        self.projects.contains(project)
    }
    async fn query(&self, request: &Request) -> Result<Reply, ErrorCode> {
        let path = self.path.clone();
        let config: ProviderConfig = tokio::task::spawn_blocking(move || load(&path))
            .await
            .map_err(|_| ErrorCode::Unavailable)??;
        if config.policy.identity != self.identity {
            return Err(ErrorCode::InvalidResponse);
        }
        Reader::new(config.policy, config.backend)?
            .query(&self.recipient, request)
            .await
    }
}

/// One explicitly selected peer. Routing uses an existing verified SSH alias.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    /// Expected provider/owner/scope; response identity must match exactly.
    pub identity: Identity,
    /// SSH destination (for example `dejana@latlas`) resolved by fleet config.
    pub destination: String,
    /// Explicit SSH port from fleet topology.
    pub port: u16,
    /// Projects for which this peer is always consulted.
    pub projects: Vec<String>,
    /// Optional isolated fleet-managed SSH configuration. Omitting it preserves
    /// existing operator-managed alias routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_config: Option<PathBuf>,
}
impl Peer {
    fn validate(&self) -> Result<(), ErrorCode> {
        self.identity.validate()?;
        if self.port == 0
            || self
                .ssh_config
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || self.destination.is_empty()
            || self.destination.len() > 256
            || self.destination.starts_with('-')
            || !self
                .destination
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
            || self.projects.is_empty()
            || self.projects.len() > 1024
            || self
                .projects
                .iter()
                .any(|p| p.trim().is_empty() || p.len() > 256)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl Provider for Peer {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn supports(&self, project: &str) -> bool {
        self.projects.iter().any(|p| p == project)
    }
    async fn query(&self, request: &Request) -> Result<Reply, ErrorCode> {
        self.validate()?;
        let frame = serde_json::to_vec(&WireRequest {
            version: 1,
            request: request.clone(),
        })
        .map_err(|_| ErrorCode::InvalidRequest)?;
        if frame.len() as u64 >= REQUEST_BYTES {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut command = Command::new("ssh");
        if let Some(path) = &self.ssh_config {
            command.arg("-F").arg(path);
        }
        let mut child = command
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ConnectTimeout=5",
                "-p",
            ])
            .arg(self.port.to_string())
            .arg("--")
            .arg(&self.destination)
            .arg("chaosbox federation serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| ErrorCode::Unavailable)?;
        let mut stdin = child.stdin.take().ok_or(ErrorCode::Unavailable)?;
        stdin
            .write_all(&frame)
            .await
            .map_err(|_| ErrorCode::Unavailable)?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| ErrorCode::Unavailable)?;
        stdin.shutdown().await.map_err(|_| ErrorCode::Unavailable)?;
        drop(stdin);
        let stdout = child.stdout.take().ok_or(ErrorCode::Unavailable)?;
        let frame = read_frame(stdout, RESPONSE_BYTES).await;
        if !child
            .wait()
            .await
            .map_err(|_| ErrorCode::Unavailable)?
            .success()
        {
            return Err(ErrorCode::Unavailable);
        }
        let frame = frame.map_err(|_| ErrorCode::InvalidResponse)?;
        let response: WireResponse =
            serde_json::from_slice(&frame).map_err(|_| ErrorCode::InvalidResponse)?;
        if response.version != 1 {
            return Err(ErrorCode::InvalidResponse);
        }
        match response.result {
            WireResult::Ok { reply } => Ok(reply),
            WireResult::Error { code } => Err(code),
        }
    }
}

/// Consumer configuration. Paths and peer routes are operator-controlled.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    /// Contract version, currently 1.
    pub version: u32,
    /// Owner-local provider configuration, independent of peer-sync enrollment.
    pub local: PathBuf,
    /// Selected peers; sharing is enforced again at each peer.
    #[serde(default)]
    pub peers: Vec<Peer>,
    /// Total context/evidence deadline, including local reading and SSH.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}
fn default_timeout() -> u64 {
    10_000
}
impl ClientConfig {
    /// Load an operator-owned JSON client configuration and validate peer routes.
    pub fn load(path: &Path) -> Result<Self, ErrorCode> {
        let config: Self = load(path)?;
        if config.version != 1 || !config.local.is_absolute() {
            return Err(ErrorCode::InvalidRequest);
        }
        for peer in &config.peers {
            peer.validate()?;
        }
        Ok(config)
    }
    /// Create a read-only federator without loading any peer database credentials.
    pub fn reader(&self) -> Result<Federator, ErrorCode> {
        if self.version != 1 {
            return Err(ErrorCode::InvalidRequest);
        }
        let local = Arc::new(FileProvider::new(&self.local, None)?) as Arc<dyn Provider>;
        let peers = self
            .peers
            .iter()
            .cloned()
            .map(|p| Arc::new(p) as Arc<dyn Provider>)
            .collect();
        Federator::new(local, peers, self.timeout_ms)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    version: u32,
    request: Request,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireResponse {
    version: u32,
    result: WireResult,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum WireResult {
    Ok { reply: Reply },
    Error { code: ErrorCode },
}

async fn read_frame(reader: impl AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>, ErrorCode> {
    let mut frame = Vec::new();
    BufReader::new(reader.take(limit + 1))
        .read_until(b'\n', &mut frame)
        .await
        .map_err(|_| ErrorCode::Unavailable)?;
    if frame.len() as u64 > limit || frame.last() != Some(&b'\n') {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(frame)
}

/// Serve exactly one request as a fixed recipient; no federation/recursion here.
pub async fn serve(path: &Path, recipient: &str) -> Result<(), ErrorCode> {
    if std::env::var("SSH_ORIGINAL_COMMAND").is_ok_and(|c| c != "chaosbox federation serve") {
        return Err(ErrorCode::Denied);
    }
    let provider = FileProvider::new(path, Some(recipient))?;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let frame = read_frame(tokio::io::stdin(), REQUEST_BYTES).await?;
        let request: WireRequest =
            serde_json::from_slice(&frame).map_err(|_| ErrorCode::InvalidRequest)?;
        if request.version != 1 {
            return Err(ErrorCode::InvalidRequest);
        }
        provider.query(&request.request).await
    })
    .await
    .unwrap_or(Err(ErrorCode::Unavailable));
    let result = match result {
        Ok(reply) => WireResult::Ok { reply },
        Err(code) => WireResult::Error { code },
    };
    let mut frame = serde_json::to_vec(&WireResponse { version: 1, result })
        .map_err(|_| ErrorCode::InvalidResponse)?;
    if frame.len() as u64 >= RESPONSE_BYTES {
        return Err(ErrorCode::InvalidResponse);
    }
    frame.push(b'\n');
    tokio::io::stdout()
        .write_all(&frame)
        .await
        .map_err(|_| ErrorCode::Unavailable)
}
