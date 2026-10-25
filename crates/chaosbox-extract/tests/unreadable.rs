//! A non-UTF-8 file under a source extension is skipped and reported, not a
//! capture failure (one such file used to fail whole repository snapshots).
use std::fs;

use chaosbox_extract::Snapshot;

#[test]
fn non_utf8_source_is_skipped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("good.rs"), "pub fn f() {}\n").unwrap();
    fs::write(dir.path().join("binary.rs"), [0xff, 0xfe, b'f', b'n', 0x80]).unwrap();
    let snap = Snapshot::capture("repo", dir.path()).unwrap();
    assert_eq!(snap.unreadable, vec!["binary.rs".to_owned()]);
    assert!(snap.file_version("good.rs").is_some());
    assert!(snap.file_version("binary.rs").is_none());
    // The skipped file never enters the identity: removing it changes nothing.
    fs::remove_file(dir.path().join("binary.rs")).unwrap();
    assert_eq!(Snapshot::capture("repo", dir.path()).unwrap().id, snap.id);
    // Explicit selections still refuse it instead of silently narrowing.
    fs::write(dir.path().join("binary.rs"), [0xff, 0xfe]).unwrap();
    assert!(Snapshot::capture_files("repo", dir.path(), &["binary.rs".into()]).is_err());
}
