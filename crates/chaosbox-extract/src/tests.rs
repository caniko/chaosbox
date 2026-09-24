//! Unit tests for extraction and candidate construction.

use super::*;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;

use chaosbox_core::{Entity, EntityKind};

fn tmp_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        let p = dir.path().join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }
    let root = dir.path().to_path_buf();
    (dir, root)
}

#[test]
fn snapshot_is_deterministic() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn foo() {}\n"), ("b.py", "def bar(): pass\n")]);
    let s1 = Snapshot::capture("r", &root).unwrap();
    let s2 = Snapshot::capture("r", &root).unwrap();
    assert_eq!(s1.id, s2.id);
    assert_eq!(s1.files.len(), 2);
}

#[test]
fn rust_python_js_md_txt_covered() {
    let (_t, root) = tmp_repo(&[
        ("a.rs", "use foo::bar;\nfn hello() {}\nhello();\n"),
        ("b.py", "import os\ndef greet(): pass\n"),
        ("c.ts", "import { x } from './y';\nfunction f() {}\n"),
        ("d.md", "# Title\n[link](https://example.com) `code`\n"),
        ("e.txt", "plain words\n"),
    ]);
    let snap = Snapshot::capture("r", &root).unwrap();
    assert_eq!(snap.files.len(), 5);
    let ext = extract_snapshot(&snap);
    let kinds: BTreeSet<String> = ext
        .entities
        .iter()
        .map(|e| format!("{:?}", e.kind))
        .collect();
    for k in [
        "File",
        "Module",
        "Definition",
        "Import",
        "Heading",
        "Link",
        "CodeMention",
    ] {
        assert!(kinds.contains(k), "missing {k} in {kinds:?}");
    }
    let cat = build_candidates(&ext, 100);
    assert!(!cat.candidates.is_empty());
    assert!(cat.candidates.len() <= 100, "candidates bounded");
    assert_eq!(cat.cap, 100);
    assert_eq!(
        cat.selected.values().sum::<u64>(),
        u64::try_from(cat.candidates.len()).expect("candidate count fits in u64"),
    );
    assert!(cat.omitted.is_empty(), "under the cap nothing omitted");
}

#[test]
fn no_cartesian_product() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")]);
    let snap = Snapshot::capture("r", &root).unwrap();
    let ext = extract_snapshot(&snap);
    let cat = build_candidates(&ext, 10);
    // 3 defs would be 9 pairs cartesian; bounded co-occurrence gives <= 2 + structural
    assert!(cat.candidates.len() <= 10);
}

#[test]
fn candidate_cap_reports_omissions() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn one() {}\nfn two() {}\nfn three() {}\n")]);
    let snap = Snapshot::capture("r", &root).unwrap();
    let ext = extract_snapshot(&snap);
    // Well under the cap: everything selected, nothing omitted.
    let full = build_candidates(&ext, 100);
    assert!(full.omitted.is_empty());
    // Under the cap: truncation is observable, not silent.
    let cat = build_candidates(&ext, 2);
    assert_eq!(cat.candidates.len(), 2, "cap enforced");
    assert_eq!(cat.cap, 2);
    let selected = cat.selected.values().sum::<u64>();
    let omitted = cat.omitted.values().sum::<u64>();
    assert_eq!(selected, 2, "selected matches the cap");
    assert!(
        omitted >= 1,
        "omitted candidates must be accounted: {cat:?}"
    );
    assert!(selected + omitted < 100, "accounting stays local");
    // Deterministic: same input, same catalog.
    let again = build_candidates(&ext, 2);
    assert_eq!(cat.candidates, again.candidates);
    assert_eq!(cat.omitted, again.omitted);
}

#[test]
fn nix_bindings_imports_and_references_extracted() {
    let (_t, root) = tmp_repo(&[
        (
            "main.nix",
            "{ ... }:\n\
                 let\n\
                 \x20 name = \"world\";\n\
                 in {\n\
                 \x20 imports = [ ./parts/extra.nix ];\n\
                 \x20 greeting = \"hello ${name}\";\n\
                 \x20 inherit (builtins) toString;\n\
                 }\n",
        ),
        (
            "parts/extra.nix",
            "{ config, ... }:\n{\n  enable = true;\n}\n",
        ),
    ]);
    let snap = Snapshot::capture("r", &root).unwrap();
    assert_eq!(snap.files.len(), 2, "both .nix files snapshotted");
    let ext = extract_snapshot(&snap);

    let file_id = |path: &str| {
        ext.entities
            .iter()
            .find(|e| e.kind == EntityKind::File && e.file == path)
            .map_or_else(|| panic!("file entity for {path}"), |e| e.id.clone())
    };
    let main_id = file_id("main.nix");
    let parts_id = file_id("parts/extra.nix");

    // Bindings become definitions (keyword-free, first site wins).
    let defs: BTreeSet<&str> = ext
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Definition && e.file == "main.nix")
        .map(|e| e.name.as_str())
        .collect();
    for name in ["name", "imports", "greeting"] {
        assert!(defs.contains(name), "missing binding {name} in {defs:?}");
    }

    // Resolved in-repo import: the stub is gone, the edge lands on
    // the real target file node.
    assert_eq!(
        ext.entities
            .iter()
            .filter(|e| e.kind == EntityKind::Import)
            .count(),
        0,
        "resolved import stub must be replaced by the file edge"
    );
    assert!(
        ext.explicit_refs
            .contains(&(main_id.clone(), parts_id, "imports".to_owned())),
        "file -> file import edge required"
    );

    // `${name}` interpolation references the in-file binding.
    let name_def = ext
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Definition && e.name == "name")
        .map(|e| e.id.clone())
        .unwrap();
    assert!(
        ext.explicit_refs
            .contains(&(main_id, name_def, "references".to_owned())),
        "interpolation reference required"
    );
}

#[test]
fn nix_unresolved_import_keeps_stub() {
    let (_t, root) = tmp_repo(&[(
        "main.nix",
        "{ ... }:\n{\n  imports = [ ./missing.nix ];\n}\n",
    )]);
    let snap = Snapshot::capture("r", &root).unwrap();
    let ext = extract_snapshot(&snap);
    let stubs: Vec<&Entity> = ext
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Import)
        .collect();
    assert_eq!(stubs.len(), 1, "unresolved import stays visible");
    assert_eq!(stubs[0].name, "./missing.nix");
    let main_id = ext
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::File && e.file == "main.nix")
        .map_or_else(|| panic!("file entity for main.nix"), |e| e.id.clone());
    assert!(
        ext.explicit_refs
            .contains(&(main_id, stubs[0].id.clone(), "imports".to_owned())),
        "stub keeps its import edge"
    );
}

#[cfg(unix)]
#[test]
fn snapshot_skips_symlink_escape() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.rs"), "fn secret() {}\n").unwrap();
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
    std::os::unix::fs::symlink(outside.path().join("secret.rs"), root.join("evil.rs")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
    let snap = Snapshot::capture("r", &root).unwrap();
    let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["a.rs"],
        "outside-root links must not leak in: {paths:?}"
    );
}

#[cfg(unix)]
#[test]
fn snapshot_skips_interior_links_and_cycles() {
    let (_t, root) = tmp_repo(&[("sub/inner.rs", "fn inner() {}\n")]);
    std::os::unix::fs::symlink(root.join("sub"), root.join("linked")).unwrap();
    // Cycle: sub/loop -> root. Capture must terminate.
    std::os::unix::fs::symlink(root.clone(), root.join("sub").join("loop")).unwrap();
    let snap = Snapshot::capture("r", &root).unwrap();
    let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    // Symlinked content is out of scope: only the real tree is indexed.
    assert_eq!(paths, ["sub/inner.rs"]);
}

#[cfg(unix)]
#[test]
fn snapshot_skips_special_files() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
    // Unix socket at a supported extension: not a regular file, must
    // never reach a blocking read.
    std::os::unix::net::UnixListener::bind(root.join("events.txt")).unwrap();
    let snap = Snapshot::capture("r", &root).unwrap();
    let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["a.rs"]);
}

#[test]
fn snapshot_rejects_broken_ignore_rules() {
    let (_t, root) = tmp_repo(&[(".gitignore", "{unclosed\n"), ("a.rs", "fn a() {}\n")]);
    let err = Snapshot::capture("r", &root).expect_err("broken ignore rules must fail loudly");
    assert!(
        !err.to_string().is_empty(),
        "an explicit exclusion-policy error is required"
    );
}

#[test]
fn snapshot_respects_gitignore() {
    let (_t, root) = tmp_repo(&[
        (".gitignore", "private.txt\ngen/\n"),
        ("a.rs", "fn a() {}\n"),
        ("private.txt", "excluded words\n"),
        ("gen/out.rs", "fn out() {}\n"),
    ]);
    let snap = Snapshot::capture("r", &root).unwrap();
    let mut paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        ["a.rs"],
        "ignored files must never reach inference: {paths:?}"
    );
}

#[test]
fn snapshot_skips_nested_git_repos() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
    std::fs::create_dir_all(root.join("vendor/dep/.git")).unwrap();
    std::fs::write(root.join("vendor/dep/b.rs"), "fn b() {}\n").unwrap();
    let snap = Snapshot::capture("r", &root).unwrap();
    let paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["a.rs"], "nested checkouts stay out: {paths:?}");
}

#[test]
fn unsupported_reported() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
    std::fs::write(root.join("img.png"), [0u8, 1, 2]).unwrap();
    let bad = report_unsupported(&root);
    assert_eq!(bad.len(), 1);
}

#[test]
fn spans_are_sane() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn foo() {}\n")]);
    let snap = Snapshot::capture("r", &root).unwrap();
    let ext = extract_snapshot(&snap);
    let def = ext
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Definition)
        .unwrap();
    assert_eq!(def.span.start_line, 1);
    assert_eq!(def.span.file, "a.rs");
}

#[cfg(unix)]
#[test]
fn symlinks_never_followed() {
    use std::os::unix::fs::symlink;
    let (_t, root) = tmp_repo(&[("a.rs", "fn foo() {}\n"), ("sub/b.rs", "fn bar() {}\n")]);
    std::fs::create_dir_all(root.join("outside")).unwrap();
    std::fs::write(root.join("outside/secret.rs"), "fn secret() {}\n").unwrap();
    symlink("a.rs", root.join("link-file.rs")).unwrap();
    symlink("sub", root.join("link-dir")).unwrap();
    symlink("..", root.join("sub/loop")).unwrap();
    symlink("outside/secret.rs", root.join("escape.rs")).unwrap();
    let snap = Snapshot::capture("r", &root).unwrap();
    let mut files: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    files.sort_unstable();
    assert_eq!(files, ["a.rs", "outside/secret.rs", "sub/b.rs"]);
}

#[test]
fn scoped_capture_restricts_to_subtree() {
    let (_t, root) = tmp_repo(&[("cli/a.rs", "fn a() {}\n"), ("lib/b.rs", "fn b() {}\n")]);
    let whole = Snapshot::capture("r", &root).unwrap();
    assert!(whole.scope.is_empty());
    assert_eq!(whole.files.len(), 2);
    let scoped = Snapshot::capture_scoped("r", &root, &["cli".to_owned()]).unwrap();
    assert_eq!(scoped.scope, vec!["cli".to_owned()]);
    let paths: Vec<_> = scoped.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["cli/a.rs"],
        "scope must not pull siblings: {paths:?}"
    );
    assert_ne!(
        scoped.id, whole.id,
        "scope is part of the snapshot identity"
    );
}

#[test]
fn scope_is_part_of_identity_even_for_empty_dirs() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let whole = Snapshot::capture("r", &root).unwrap();
    let scoped = Snapshot::capture_scoped("r", &root, &["empty".to_owned()]).unwrap();
    assert!(scoped.files.is_empty());
    assert_ne!(
        scoped.id, whole.id,
        "same file set under a different scope must still differ"
    );
}

#[test]
fn invalid_scope_rejected() {
    let (_t, root) = tmp_repo(&[("a.rs", "fn a() {}\n")]);
    for bad in [
        "/abs".to_owned(),
        "../escape".to_owned(),
        "a/../b".to_owned(),
        String::new(),
    ] {
        assert!(
            Snapshot::capture_scoped("r", &root, std::slice::from_ref(&bad)).is_err(),
            "scope {bad:?} must fail loudly"
        );
    }
    assert!(
        Snapshot::capture_scoped("r", &root, &["missing".to_owned()]).is_err(),
        "missing scope dir must fail, never publish an empty graph"
    );
    assert!(
        Snapshot::capture_scoped("r", &root, &["a.rs".to_owned()]).is_err(),
        "scope must be a directory, not a file"
    );
}

#[test]
fn overlapping_scopes_dedup() {
    let (_t, root) = tmp_repo(&[("a/x.rs", "fn x() {}\n"), ("a/b/y.rs", "fn y() {}\n")]);
    let snap = Snapshot::capture_scoped(
        "r",
        &root,
        &["a".to_owned(), "a/b".to_owned(), "a".to_owned()],
    )
    .unwrap();
    assert_eq!(snap.scope, vec!["a".to_owned(), "a/b".to_owned()]);
    let mut paths: Vec<_> = snap.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["a/b/y.rs", "a/x.rs"]);
}
