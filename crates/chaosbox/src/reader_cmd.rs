//! Privileged host-connector launch boundary. The admission file is a control
//! input, not authentication: workers must not be able to launch this process
//! with database credentials or write its admission directory.

use std::{io::Read, path::Path, time::Duration};
use chaosbox::read_view::{ReadError, ReadView, ScopedReader, mcp};
use chaosbox_typedb::reader::TypeDbReader;

const ADMISSION_BYTES: u64 = 128 * 1024;

#[cfg(target_os = "linux")]
mod pipe;

#[derive(PartialEq, Eq)]
struct AdmissionFile {
    bytes: Vec<u8>,
    device: u64,
    inode: u64,
}

/// Load a bounded private regular file owned by the connector process. Open
/// metadata is checked against the original inode to refuse symlink swaps.
#[cfg(target_os = "linux")]
fn admission_file(path: &Path) -> Result<AdmissionFile, ReadError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // Linux UAPI open flags (this entrypoint is Linux-only). No symlink
    // following or blocking FIFO open during a concurrent lease replacement.
    const O_NOFOLLOW: i32 = 0x20000;
    const O_NONBLOCK: i32 = 0x800;
    let before = std::fs::symlink_metadata(path).map_err(|_| ReadError::InvalidView)?;
    if !before.is_file() {
        return Err(ReadError::InvalidView);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .map_err(|_| ReadError::InvalidView)?;
    let opened = file.metadata().map_err(|_| ReadError::InvalidView)?;
    let owner = std::fs::metadata("/proc/self")
        .map_err(|_| ReadError::InvalidView)?
        .uid();
    if !before.is_file()
        || !opened.is_file()
        || opened.uid() != owner
        || opened.mode() & 0o077 != 0
        || before.dev() != opened.dev()
        || before.ino() != opened.ino()
        || opened.len() > ADMISSION_BYTES
    {
        return Err(ReadError::InvalidView);
    }
    let mut bytes = Vec::new();
    file.take(ADMISSION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadError::InvalidView)?;
    if bytes.len() as u64 > ADMISSION_BYTES {
        return Err(ReadError::InvalidView);
    }
    Ok(AdmissionFile {
        bytes,
        device: opened.dev(),
        inode: opened.ino(),
    })
}

#[cfg(not(target_os = "linux"))]
fn admission_file(_path: &Path) -> Result<AdmissionFile, ReadError> {
    Err(ReadError::InvalidView)
}

pub(super) async fn serve(path: &Path) -> Result<(), ReadError> {
    let admission = admission_file(path)?;
    let view: ReadView =
        serde_json::from_slice(&admission.bytes).map_err(|_| ReadError::InvalidView)?;
    let identity = view.identity.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ReadError::Expired)?
        .as_secs();
    view.validate_at(&identity, now)?;
    let timeout = Duration::from_millis(view.budgets.timeout_ms);
    let reader = tokio::time::timeout(timeout, async {
        let config = super::typedb_config_from_env().map_err(|_| ReadError::Backend)?;
        let mut handle = TypeDbReader::new(config);
        handle.connect().await.map_err(|_| ReadError::Backend)?;
        ScopedReader::admit(handle, view, &identity).await
    })
    .await
    .map_err(|_| ReadError::Deadline)??;
    // The file is a one-way lease: disappearance, replacement or content drift
    // revokes rather than altering an already-admitted connection's scope.
    if admission_file(path)? != admission {
        return Err(ReadError::Revoked);
    }
    let revoker = reader.revoker();
    let watch = async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            if !admission_file(path).is_ok_and(|current| current == admission) {
                revoker.revoke();
                return Err(ReadError::Revoked);
            }
        }
    };
    #[cfg(target_os = "linux")]
    let (input, output) = (
        pipe::Pipe::stdio(0).map_err(|_| ReadError::Backend)?,
        pipe::Pipe::stdio(1).map_err(|_| ReadError::Backend)?,
    );
    #[cfg(not(target_os = "linux"))]
    let (input, output) = (tokio::io::stdin(), tokio::io::stdout());
    tokio::select! {
        biased;
        result = watch => result,
        result = mcp::serve(reader, input, output) => result.map_err(|_| ReadError::Backend),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn lease_requires_a_private_owned_regular_bounded_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lease.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(admission_file(&path).unwrap().bytes, b"{}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(admission_file(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(admission_file(&alias).is_err());
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(ADMISSION_BYTES + 1)
            .unwrap();
        assert!(admission_file(&path).is_err());
    }
}
