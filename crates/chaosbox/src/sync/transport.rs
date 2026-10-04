//! Bounded signed anti-entropy over an encrypted SSH stdio connection.
//! The remote command is constant; peer addresses never become shell fragments.
use std::{collections::BTreeSet, process::Stdio, time::Duration};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::Command,
};
use chaosbox_store::ReplicaStore;
use super::{
    identity, Grant, Identity, SignedEvent, Replica, MAX_EVENTS, MAX_EVENT_BYTES, load_replica,
    persist_event, publish_current,
};

const MAX_FRAME: usize = 12 * 1024 * 1024;
const PAGE_BYTES: usize = 6 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(60);

/// An operator-configured SSH target and enrolled device identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    /// SSH host alias or user@host; never a command or argument list.
    pub target: String,
    /// Standard SSH port, or an explicit fleet port.
    pub port: u16,
    /// Expected device public key; avoids connecting to a different enrolled host.
    pub device: String,
}

impl Peer {
    /// Reject options and shell syntax before constructing a process.
    pub fn validate(&self) -> Result<(), String> {
        if self.target.is_empty()
            || self.target.len() > 255
            || self.target.starts_with('-')
            || !self
                .target
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-@:[]".contains(&b))
            || self.port == 0
            || !super::is_digest(&self.device)
        {
            return Err("invalid SSH peer endpoint/device".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    grant: Grant,
    scope: String,
    nonce: String,
    body: Value,
    signature: String,
}

impl Envelope {
    fn bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&(
            self.version,
            &self.grant,
            &self.scope,
            &self.nonce,
            &self.body,
        ))
        .map_err(|_| "encode peer message".into())
    }
    fn new(identity: &Identity, scope: &str, nonce: &str, body: Value) -> Result<Self, String> {
        let mut e = Self {
            version: 1,
            grant: identity.grant.clone(),
            scope: scope.into(),
            nonce: nonce.into(),
            body,
            signature: String::new(),
        };
        e.signature = identity.sign("chaosbox-wire-v1\0", &e.bytes()?);
        Ok(e)
    }
    fn validate(
        &self,
        user: &str,
        scope: &str,
        nonce: Option<&str>,
        device: Option<&str>,
    ) -> Result<(), String> {
        self.grant.validate(user, scope)?;
        if self.version != 1
            || self.scope != scope
            || !super::is_digest(&self.nonce)
            || nonce.is_some_and(|n| n != self.nonce)
            || device.is_some_and(|d| d != self.grant.device)
        {
            return Err("peer session version, challenge or identity mismatch".into());
        }
        identity::verify(
            &self.grant.device,
            "chaosbox-wire-v1\0",
            &self.bytes()?,
            &self.signature,
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    inventory: Vec<String>,
    events: Vec<SignedEvent>,
    done: bool,
}

impl Page {
    fn validate(&self) -> Result<(), String> {
        if self.inventory.len() > MAX_EVENTS
            || self.inventory.windows(2).any(|w| w[0] >= w[1])
            || self.inventory.iter().any(|id| !super::is_digest(id))
            || self.events.len() > 200
        {
            return Err("unbounded or unordered peer inventory".into());
        }
        Ok(())
    }
}

async fn read(reader: &mut (impl AsyncRead + Unpin)) -> Result<Envelope, String> {
    tokio::time::timeout(DEADLINE, async {
        let size = reader.read_u32().await.map_err(|e| e.to_string())? as usize;
        if size == 0 || size > MAX_FRAME {
            return Err("peer frame exceeds bounded capacity".into());
        }
        let mut bytes = vec![0u8; size];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|_| "invalid peer envelope".into())
    })
    .await
    .map_err(|_| "peer read deadline exceeded")?
}

async fn write(writer: &mut (impl AsyncWrite + Unpin), envelope: &Envelope) -> Result<(), String> {
    let bytes = serde_json::to_vec(envelope).map_err(|_| "encode peer envelope")?;
    if bytes.len() > MAX_FRAME {
        return Err("peer output exceeds frame capacity".into());
    }
    tokio::time::timeout(DEADLINE, async {
        writer
            .write_u32(u32::try_from(bytes.len()).map_err(|_| "peer frame overflow")?)
            .await
            .map_err(|e| e.to_string())?;
        writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
        writer.flush().await.map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "peer write deadline exceeded")?
}

fn page(replica: &Replica, known: &BTreeSet<String>, done: bool) -> Result<Page, String> {
    let mut events = Vec::new();
    let mut bytes = 0;
    for e in replica.events.values().filter(|e| !known.contains(&e.id)) {
        let size = serde_json::to_vec(e).map_err(|_| "encode event")?.len();
        if size > MAX_EVENT_BYTES {
            return Err("oversize event in replica".into());
        }
        if bytes + size > PAGE_BYTES || events.len() == 200 {
            break;
        }
        bytes += size;
        events.push(e.clone());
    }
    Ok(Page {
        inventory: replica.events.keys().cloned().collect(),
        done: done && events.is_empty(),
        events,
    })
}

async fn accept(
    store: &mut impl ReplicaStore,
    replica: &mut Replica,
    page: &Page,
) -> Result<(), String> {
    page.validate()?;
    // Validate the whole page and capacity before the first durable insert.
    let mut next = replica.clone();
    for event in &page.events {
        next.receive(event.clone())?;
    }
    let missing = next
        .events
        .values()
        .flat_map(|e| &e.parents)
        .any(|id| !next.events.contains_key(id));
    if !missing {
        next.view()?;
    }
    for event in &page.events {
        persist_event(store, &replica.user, &replica.scope, event).await?;
    }
    *replica = next;
    // Staged dependency gaps are normal; operational errors still propagate.
    if replica.view().is_ok() {
        publish_current(store, &replica.user, &replica.scope).await?;
    }
    Ok(())
}

/// Serve one authenticated stdio session. Called remotely by the fixed SSH command.
pub async fn serve(
    store: &mut impl ReplicaStore,
    identity: &Identity,
    scope: &str,
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<(), String> {
    let hello = read(reader).await?;
    hello.validate(&identity.user(), scope, None, None)?;
    if hello.body != json!({"hello":true}) {
        return Err("invalid peer handshake".into());
    }
    let nonce = identity::nonce()?;
    write(
        writer,
        &Envelope::new(identity, scope, &nonce, json!({"echo":hello.nonce}))?,
    )
    .await?;
    for _ in 0..MAX_EVENTS + 2 {
        let request = read(reader).await?;
        request.validate(
            &identity.user(),
            scope,
            Some(&nonce),
            Some(&hello.grant.device),
        )?;
        let request: Page =
            serde_json::from_value(request.body).map_err(|_| "invalid peer page")?;
        let mut replica = load_replica(store, &identity.user(), scope).await?;
        accept(store, &mut replica, &request).await?;
        let known = request.inventory.iter().cloned().collect();
        let reply = page(&replica, &known, request.done)?;
        let done = reply.done;
        write(
            writer,
            &Envelope::new(
                identity,
                scope,
                &hello.nonce,
                serde_json::to_value(reply).map_err(|_| "encode page")?,
            )?,
        )
        .await?;
        if done {
            return Ok(());
        }
    }
    Err("peer exchange exceeded bounded round count".into())
}

/// Bidirectional anti-entropy over an already encrypted stream; useful for protocol tests.
pub async fn exchange(
    store: &mut impl ReplicaStore,
    identity: &Identity,
    scope: &str,
    expected_device: &str,
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<(), String> {
    let nonce = identity::nonce()?;
    write(
        writer,
        &Envelope::new(identity, scope, &nonce, json!({"hello":true}))?,
    )
    .await?;
    let hello = read(reader).await?;
    hello.validate(&identity.user(), scope, None, Some(expected_device))?;
    if hello.body != json!({"echo":nonce}) {
        return Err("peer did not answer the current challenge".into());
    }
    let mut known = BTreeSet::new();
    for _ in 0..MAX_EVENTS + 2 {
        let mut replica = load_replica(store, &identity.user(), scope).await?;
        let outgoing = page(&replica, &known, true)?;
        write(
            writer,
            &Envelope::new(
                identity,
                scope,
                &hello.nonce,
                serde_json::to_value(outgoing).map_err(|_| "encode page")?,
            )?,
        )
        .await?;
        let response = read(reader).await?;
        response.validate(&identity.user(), scope, Some(&nonce), Some(expected_device))?;
        let response: Page =
            serde_json::from_value(response.body).map_err(|_| "invalid peer page")?;
        accept(store, &mut replica, &response).await?;
        known = response.inventory.iter().cloned().collect();
        if response.done {
            return Ok(());
        }
    }
    Err("peer exchange exceeded bounded round count".into())
}

/// Connect without passwords, forwarding, or unverified SSH host keys.
pub async fn connect(
    store: &mut impl ReplicaStore,
    identity: &Identity,
    scope: &str,
    peer: &Peer,
) -> Result<(), String> {
    peer.validate()?;
    let mut child = Command::new("ssh")
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
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=10",
            "-o",
            "ServerAliveCountMax=3",
            "-p",
        ])
        .arg(peer.port.to_string())
        .arg("--")
        .arg(&peer.target)
        .arg("chaosbox sync exchange")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut reader = child.stdout.take().ok_or("missing SSH stdout")?;
    let mut writer = child.stdin.take().ok_or("missing SSH stdin")?;
    let result = exchange(
        store,
        identity,
        scope,
        &peer.device,
        &mut reader,
        &mut writer,
    )
    .await;
    drop(writer);
    if result.is_err() {
        let _ = child.kill().await;
        return result;
    }
    let status = tokio::time::timeout(DEADLINE, child.wait())
        .await
        .map_err(|_| "SSH completion deadline exceeded")?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("SSH peer exchange failed".into());
    }
    Ok(())
}
