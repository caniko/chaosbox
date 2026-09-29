//! Git provenance is usable only when selected bytes belong to that revision.

use std::{path::Path, process::Command};
use chaosbox_extract::Snapshot;

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-C"])
        .arg(root)
        .args(args)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

pub(super) fn git_version(root: &Path, snapshot: &Snapshot) -> (Option<String>, bool) {
    let text = |args: &[&str]| {
        String::from_utf8(git(root, args)?)
            .ok()
            .map(|s| s.trim().to_owned())
    };
    let top = text(&["rev-parse", "--show-toplevel"]);
    if top.as_deref().map(Path::new) != Some(root) {
        return (None, true);
    }
    let revision = text(&["rev-parse", "HEAD"]);
    let dirty = text(&[
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ])
    .is_none_or(|s| !s.is_empty());
    let bound = revision.as_ref().is_some_and(|revision| {
        snapshot.contents.iter().all(|(path, source)| {
            git(root, &["cat-file", "blob", &format!("{revision}:{path}")])
                .is_some_and(|bytes| bytes == source.as_bytes())
        })
    });
    (revision, dirty || !bound)
}
