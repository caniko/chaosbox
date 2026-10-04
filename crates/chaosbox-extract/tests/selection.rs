//! Explicit workspace inputs have separate identities and strict path boundaries.
use std::fs;
use chaosbox_extract::Snapshot;

#[test]
fn selected_files_are_exact_bounded_and_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source.rs"), "pub fn f() {}\n").unwrap();
    fs::write(dir.path().join("config.json"), "{}\n").unwrap();
    let selected = Snapshot::capture_files(
        "repo",
        dir.path(),
        &["config.json".into(), "source.rs".into()],
    )
    .unwrap();
    let reversed = Snapshot::capture_files(
        "repo",
        dir.path(),
        &["source.rs".into(), "config.json".into(), "source.rs".into()],
    )
    .unwrap();
    assert_eq!(selected.id, reversed.id);
    assert_eq!(selected.contents, reversed.contents);
    assert_ne!(
        selected.id,
        Snapshot::capture("repo", dir.path()).unwrap().id
    );
    assert!(Snapshot::capture_files("repo", dir.path(), &[]).is_err());
    for path in [
        "../source.rs",
        "/source.rs",
        "./source.rs",
        "source.rs/",
        "absent",
    ] {
        assert!(
            Snapshot::capture_files("repo", dir.path(), &[path.into()]).is_err(),
            "{path}"
        );
    }
    fs::create_dir_all(dir.path().join("nested/.git")).unwrap();
    fs::write(dir.path().join("nested/other.rs"), "fn f() {}\n").unwrap();
    assert!(Snapshot::capture_files("repo", dir.path(), &["nested/other.rs".into()]).is_err());
}

#[cfg(unix)]
#[test]
fn explicit_paths_cannot_cross_symlinked_files_or_parent_directories() {
    use std::os::unix::fs::symlink;
    use chaosbox_extract::compiler::AnalysisInputs;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("real/src")).unwrap();
    fs::write(dir.path().join("real/src/a.rs"), "pub fn f() {}\n").unwrap();
    symlink("real", dir.path().join("linked")).unwrap();
    symlink("real/src/a.rs", dir.path().join("alias.rs")).unwrap();
    for path in ["alias.rs", "linked/src/a.rs"] {
        assert!(Snapshot::capture_files("repo", dir.path(), &[path.into()]).is_err());
        assert!(AnalysisInputs::capture("repo", dir.path(), &[], &[path.into()]).is_err());
    }
    assert!(AnalysisInputs::capture("repo", dir.path(), &["linked/src".into()], &[]).is_err());
}
