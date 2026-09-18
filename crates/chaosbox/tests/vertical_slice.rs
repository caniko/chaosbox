//! Vertical slice: snapshot fixture -> extract -> candidates -> fixture Jev
//! decisions -> persist -> publish -> query -> change/delete source ->
//! incremental rerun -> replacement build. Mirrors the acceptance order.

use std::{collections::BTreeMap, path::PathBuf};

use chaosbox::{FixtureResponder, Materialization, Pipeline, export_json, search};
use chaosbox_core::diff_builds;
use chaosbox_extract::Snapshot;
use chaosbox_gel::Store as _;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-repo")
}

#[tokio::test]
async fn vertical_slice_publish_query_incremental() {
    let root = fixture_root();
    assert!(root.exists(), "fixture repo missing at {root:?}");

    // 1-2. snapshot + deterministic extraction/candidates
    let (snap, ext, cands) = Pipeline::snapshot_extract("demo", &root, 200).unwrap();
    assert!(ext.entities.len() > 10, "expected entities, got {}", ext.entities.len());
    assert!(!cands.is_empty(), "expected candidates");

    // 3. real Rust Jev adapter path against a local protocol fixture
    // (typed request/response validation, no creds, no network).
    // One threshold source for decisions and publication (see Materialization).
    let mat = Materialization::default();
    let entities: BTreeMap<_, _> = ext.entities.iter().map(|e| (e.id.clone(), e.clone())).collect();
    let mut responder = FixtureResponder::new(true);
    let decided =
        Pipeline::decide(&cands, &entities, &mut responder, chaosbox_jev::JEV_MODEL_PINNED, &mat)
            .await
            .unwrap();
    assert_eq!(decided.len(), cands.len());
    for (_, d, _) in &decided {
        assert_eq!(d.model_requested, chaosbox_jev::JEV_MODEL_PINNED);
        assert_eq!(d.model_returned, chaosbox_jev::JEV_MODEL_PINNED);
    }

    // 4-5. persist decisions/evidence + publish validated build
    let mut pipe = Pipeline::new();
    let build = pipe.build_and_publish("demo", &snap, &ext, &decided, &mat, None).unwrap();
    assert!(!build.nodes.is_empty());
    assert!(!build.edges.is_empty(), "fixture decisions should materialize edges");

    // 6. query through shared implementation (CLI/MCP use the same fns)
    let hits = search(&build, "hello", 10);
    assert!(!hits.is_empty(), "search should find hello");
    let v = export_json(&build);
    assert!(v.get("nodes").is_some() && v.get("links").is_some());

    // 7. change/delete a source, rerun incrementally, verify replacement
    let tmp = tempfile::tempdir().unwrap();
    let tmp_root = tmp.path();
    copy_dir(&root, tmp_root);
    std::fs::remove_file(tmp_root.join("greeter.py")).unwrap();
    std::fs::write(tmp_root.join("notes.txt"), "changed notes about hello\n").unwrap();
    let (snap2, ext2, cands2) = Pipeline::snapshot_extract("demo", tmp_root, 200).unwrap();
    assert_ne!(snap.id, snap2.id, "changed sources => new snapshot");
    let entities2: BTreeMap<_, _> = ext2.entities.iter().map(|e| (e.id.clone(), e.clone())).collect();
    let mut responder2 = FixtureResponder::new(true);
    let decided2 =
        Pipeline::decide(&cands2, &entities2, &mut responder2, chaosbox_jev::JEV_MODEL_PINNED, &mat)
            .await
            .unwrap();
    let build2 = pipe
        .build_and_publish("demo", &snap2, &ext2, &decided2, &mat, Some(build.id.clone()))
        .unwrap();
    assert_ne!(build.id, build2.id);
    assert_eq!(pipe.store.active("demo").unwrap().id, build2.id, "last good replaced atomically");
    let diff = diff_builds(&build, &build2);
    assert!(!diff.added_nodes.is_empty() || !diff.removed_nodes.is_empty(), "incremental diff must show change");
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
    for entry in walkdir_files(src) {
        let rel = entry.strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).unwrap();
        } else {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p).unwrap();
            }
            std::fs::copy(&entry, &target).unwrap();
        }
    }
}

fn walkdir_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    let _ = Snapshot::capture; // keep import if optimizers complain
    out
}
