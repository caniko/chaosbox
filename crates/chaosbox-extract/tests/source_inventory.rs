//! Admission metadata must select the same files as native snapshot capture.

use std::{fs, time::Duration};

#[cfg(unix)]
use std::os::unix::fs::symlink;

use chaosbox_extract::{source_inventory, InventoryLimits, Snapshot};

fn limits() -> InventoryLimits {
    InventoryLimits {
        maximum_entries: 100_000,
        timeout: Duration::from_secs(5),
    }
}

#[test]
fn metadata_and_capture_share_ignore_extension_scope_and_repository_boundaries() {
    let root = tempfile::tempdir().unwrap();
    for dir in [
        "src/sub",
        "src/target",
        "src/node_modules",
        "src/nested/.git",
        "outside",
    ] {
        fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    for (path, text) in [
        (".gitignore", "ignored.rs\n"),
        ("src/main.RS", "pub fn main() {}\n"),
        ("src/sub/guide.md", "# Guide\n"),
        ("src/.hidden.ts", "export const hidden = 1;\n"),
        ("src/ignored.rs", "ignored"),
        ("src/unsupported.bin", "unsupported"),
        ("src/target/generated.rs", "generated"),
        ("src/node_modules/dependency.js", "dependency"),
        ("src/nested/foreign.rs", "foreign"),
        ("outside/foreign.rs", "outside"),
    ] {
        fs::write(root.path().join(path), text).unwrap();
    }
    #[cfg(unix)]
    symlink(root.path().join("outside"), root.path().join("src/link")).unwrap();
    let scope = vec!["src".to_owned(), "src/sub".to_owned()];
    let snapshot = Snapshot::capture_scoped("fixture", root.path(), &scope).unwrap();
    let metadata = source_inventory(root.path(), &scope, limits()).unwrap();
    assert!(metadata.complete);
    assert_eq!(metadata.scope, snapshot.scope);
    assert_eq!(metadata.files, 3);
    assert_eq!(metadata.files, snapshot.files.len() as u64);
    assert_eq!(
        metadata.bytes,
        snapshot.files.iter().map(|file| file.bytes).sum::<u64>()
    );
    assert!(metadata.stopped_by.is_none());
}

#[test]
fn metadata_reads_no_source_text_and_notices_new_supported_files() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("invalid.rs"), [0xff, 0xfe]).unwrap();
    let metadata = source_inventory(root.path(), &[], limits()).unwrap();
    assert!(metadata.complete);
    assert_eq!((metadata.files, metadata.bytes), (1, 2));
    assert!(Snapshot::capture("fixture", root.path()).is_err());
    fs::write(root.path().join("added.md"), "new source").unwrap();
    let again = source_inventory(root.path(), &[], limits()).unwrap();
    assert_eq!((again.files, again.bytes), (2, 12));
}

#[test]
fn an_entry_limit_is_partial_admission_evidence() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("one.rs"), "one").unwrap();
    fs::write(root.path().join("two.rs"), "two").unwrap();
    let metadata = source_inventory(
        root.path(),
        &[],
        InventoryLimits {
            maximum_entries: 1,
            ..limits()
        },
    )
    .unwrap();
    assert!(!metadata.complete);
    assert!(metadata.stopped_by.is_some());
    assert_eq!(metadata.entries_visited, 1);
}

#[test]
fn an_elapsed_time_limit_is_partial_admission_evidence() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("one.rs"), "source").unwrap();
    let metadata = source_inventory(
        root.path(),
        &[],
        InventoryLimits {
            timeout: Duration::from_nanos(1),
            ..limits()
        },
    )
    .unwrap();
    assert!(!metadata.complete);
    assert_eq!(metadata.stopped_by, Some("timeout"));
}

#[test]
fn invalid_limits_and_scope_roots_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    assert!(source_inventory(
        root.path(),
        &[],
        InventoryLimits {
            maximum_entries: 0,
            ..limits()
        }
    )
    .is_err());
    assert!(source_inventory(
        root.path(),
        &[],
        InventoryLimits {
            timeout: Duration::ZERO,
            ..limits()
        }
    )
    .is_err());
    for scope in ["missing", "../outside", "/outside"] {
        assert!(source_inventory(root.path(), &[scope.to_owned()], limits()).is_err());
    }
}

#[test]
#[cfg(unix)]
fn explicit_scope_cannot_cross_an_intermediate_symlink_or_checkout() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(outside.path().join("src")).unwrap();
    symlink(outside.path(), root.path().join("link")).unwrap();
    fs::create_dir_all(root.path().join("nested/.git")).unwrap();
    fs::create_dir(root.path().join("nested/src")).unwrap();
    for scope in ["link/src", "nested/src"] {
        let scope = [scope.to_owned()];
        assert!(source_inventory(root.path(), &scope, limits()).is_err());
        assert!(Snapshot::capture_scoped("fixture", root.path(), &scope).is_err());
    }
}

#[test]
fn broken_ignore_rules_do_not_admit_a_partial_policy() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join(".gitignore"), "{unclosed\n").unwrap();
    fs::write(root.path().join("main.rs"), "source").unwrap();
    assert!(source_inventory(root.path(), &[], limits()).is_err());
}
