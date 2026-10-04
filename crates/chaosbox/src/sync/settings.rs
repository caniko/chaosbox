//! Operator-pinned peer settings and private enrollment files.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use super::{Identity, Grant, transport::Peer};

/// Per-device settings. Peers never supply paths or connection credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Settings contract version.
    pub version: u32,
    /// Stable user-root public key.
    pub user: String,
    /// Exact replicated visibility scope.
    pub scope: String,
    /// Explicit SSH endpoints, including VPN aliases.
    pub peers: Vec<Peer>,
    /// Optional existing private custody archive whose admitted outbox is watched.
    pub archive: Option<PathBuf>,
}

/// Default state root; remote SSH exchange uses the same environment/config convention.
pub fn default_directory() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CHAOSBOX_SYNC_DIR") {
        return Ok(path.into());
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .ok_or("HOME/XDG_STATE_HOME unset")?;
    Ok(base.join("chaosbox/sync"))
}

pub(crate) fn private_directory(path: &Path) -> Result<(), String> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path).map_err(|e| e.to_string())?;
    }
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() {
        return Err("sync root must be a nonsymlink directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err("sync root must be private (0700)".into());
        }
    }
    Ok(())
}

pub(crate) fn read_bytes(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("sync input must be a nonsymlink regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, MetadataExt};
        if private && (meta.permissions().mode() & 0o077 != 0 || meta.nlink() != 1) {
            return Err("sync credential must be private and singly linked".into());
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("sync input exceeds bounded capacity".into());
    }
    Ok(bytes)
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&read_bytes(path, 64 * 1024, false)?)
        .map_err(|_| "invalid sync settings/enrollment".into())
}

pub(crate) fn write_bytes(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let parent = path.parent().ok_or("sync file lacks parent")?;
    private_directory(parent)?;
    if path.exists() {
        if !replace {
            return Err("sync identity/output already exists".into());
        }
        read_bytes(path, 64 * 1024, true)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    if replace {
        temporary.persist(path).map_err(|e| e.to_string())?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|e| e.to_string())?;
    }
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize, replace: bool) -> Result<(), String> {
    write_bytes(
        path,
        &serde_json::to_vec_pretty(value).map_err(|_| "encode sync settings")?,
        replace,
    )
}

/// Process-held local publisher/dispatch lease. A crash releases the OS lock.
pub(crate) fn worker_lease(directory: &Path) -> Result<fs::File, String> {
    private_directory(directory)?;
    let path = directory.join("worker.lock");
    if path.exists() {
        read_bytes(&path, 1024, true)?;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    fs2::FileExt::try_lock_exclusive(&file)
        .map_err(|_| "another local sync publisher/reconciler holds the device lease")?;
    Ok(file)
}

impl Settings {
    /// Load only public settings; read-only consumers never open the device signing key.
    pub fn load(directory: &Path) -> Result<Self, String> {
        let value: Self = read_json(&directory.join("config.json"))?;
        if value.version != 1
            || !super::is_digest(&value.user)
            || !value.scope.starts_with("private:")
            || value.scope.len() <= 8
            || value.scope.len() > 256
            || value.peers.len() > 256
        {
            return Err("invalid sync user/scope/settings version".into());
        }
        for peer in &value.peers {
            peer.validate()?;
        }
        Ok(value)
    }
    /// Load private signing material only for an explicit publisher or sync worker.
    pub fn identity(&self, directory: &Path) -> Result<Identity, String> {
        let grant: Grant = read_json(&directory.join("grant.json"))?;
        grant.validate(&self.user, &self.scope)?;
        Identity::from_pkcs8(
            &read_bytes(&directory.join("device.pk8"), 4096, true)?,
            grant,
            &self.scope,
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn device_lease_excludes_a_second_worker_and_releases_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sync");
        let first = super::worker_lease(&directory).unwrap();
        assert!(super::worker_lease(&directory).is_err());
        drop(first);
        assert!(super::worker_lease(&directory).is_ok());
    }
}
