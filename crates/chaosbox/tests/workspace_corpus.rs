//! S05a inputs and independent oracle integrity; automatic resolution is qualified later.

use std::{collections::BTreeMap, fmt::Write, fs, path::PathBuf};

use chaosbox_core::{coverage::SyntaxStatus, EntityKind};
use chaosbox_extract::{extract_snapshot, Snapshot};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn root() -> PathBuf {
    std::env::var_os("CHAOSBOX_TEST_CORPUS").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/workspace-corpus/v1"),
        PathBuf::from,
    )
}

fn load(name: &str) -> Value {
    serde_json::from_slice(&fs::read(root().join(name)).unwrap()).unwrap()
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn frozen_sources_and_evidence_match_v1() {
    for (file, expected) in [
        (
            "corpus.json",
            "4320e432f94efb783a977e19c100e7e6a5d0bcd0b7462598fa6535d06663fbfc",
        ),
        (
            "expected.json",
            "e95b1f1e4bf66d43fe8809917cae8c04b4f0123ec25053827e91f59829551d11",
        ),
    ] {
        assert_eq!(
            digest(&fs::read(root().join(file)).unwrap()),
            expected,
            "frozen {file}"
        );
    }
    let corpus = load("corpus.json");
    let oracle = load("expected.json");
    let members = corpus["members"].as_object().unwrap();
    assert_eq!(members.len(), 4);
    let mut files = 0;
    let mut bytes = 0;
    for (name, member) in members {
        assert_eq!(member["root"], name.as_str());
        let inventory = member["files"].as_object().unwrap();
        assert_eq!(
            inventory.keys().map(String::as_str).collect::<Vec<_>>(),
            ["Cargo.toml", "src/lib.rs"]
        );
        let mut manifest = String::new();
        for (path, expected) in inventory {
            let source = fs::read(root().join(name).join(path)).unwrap();
            let hash = digest(&source);
            assert_eq!(json!(source.len()), expected["bytes"], "{name}/{path}");
            assert_eq!(hash, expected["sha256"], "{name}/{path}");
            writeln!(manifest, "{hash}  {path}").unwrap();
            files += 1;
            bytes += source.len();
        }
        assert_eq!(
            digest(manifest.as_bytes()),
            member["content_manifest_sha256"]
        );
        let package = fs::read_to_string(root().join(name).join("Cargo.toml")).unwrap();
        assert!(package.starts_with(&format!(
            "[package]\nname = {:?}\nversion = {:?}\n",
            member["project"].as_str().unwrap(),
            member["version"].as_str().unwrap()
        )));
    }
    assert_eq!((files, bytes), (8, 1050));
    assert!(!root().join("missing-api").exists());
    let evidence = oracle["evidence"].as_object().unwrap();
    assert_eq!(evidence.len(), 11);
    for (id, citation) in evidence {
        let member = citation["member"].as_str().unwrap();
        let file = citation["file"].as_str().unwrap();
        assert!(members[member]["files"].get(file).is_some());
        let text = fs::read_to_string(root().join(member).join(file)).unwrap();
        let span = &citation["span"];
        let start = usize::try_from(span["byte_start"].as_u64().unwrap()).unwrap();
        let end = usize::try_from(span["byte_end"].as_u64().unwrap()).unwrap();
        assert_eq!(text.get(start..end), citation["quote"].as_str(), "{id}");
        for (offset, line, column) in [
            (start, "start_line", "start_col"),
            (end, "end_line", "end_col"),
        ] {
            let prefix = &text[..offset];
            assert_eq!(
                json!(prefix.bytes().filter(|byte| *byte == b'\n').count() + 1),
                span[line],
                "{id}"
            );
            assert_eq!(
                json!(prefix.rsplit('\n').next().unwrap().chars().count() + 1),
                span[column],
                "{id}"
            );
        }
    }
    eprintln!(
        "{}",
        json!({"case":"s05a-frozen-corpus", "members":4, "files":files, "source_bytes":bytes, "source_citations":evidence.len()})
    );
}

#[test]
fn independent_oracle_binds_exact_versions_and_visible_evidence() {
    let corpus = load("corpus.json");
    let oracle = load("expected.json");
    let statuses = BTreeMap::from([
        ("impact-v1", "resolved"),
        ("resolve-v2", "resolved"),
        ("ambiguous-context", "ambiguous"),
        ("unavailable-v3", "unresolved"),
        ("wrong-provider-version", "version_mismatch"),
        ("scope-blocked", "scope_blocked"),
        ("maintainer-private", "resolved"),
    ]);
    let tasks = corpus["tasks"].as_array().unwrap();
    let results = oracle["results"].as_object().unwrap();
    assert_eq!(tasks.len(), statuses.len());
    assert_eq!(results.len(), statuses.len());
    for task in tasks {
        let id = task["id"].as_str().unwrap();
        let result = &results[id];
        assert_eq!(result["status"], statuses[id]);
        let citations = result["evidence"].as_array().unwrap();
        assert!(!citations.is_empty());
        for citation in citations {
            let source = &oracle["evidence"][citation.as_str().unwrap()];
            let member = &corpus["members"][source["member"].as_str().unwrap()];
            assert!(
                member["scopes"]
                    .as_array()
                    .unwrap()
                    .contains(&task["scope"]),
                "{id}: {citation}"
            );
            if let Some(forbidden) = result["forbidden_evidence"].as_array() {
                assert!(!forbidden.contains(citation));
            }
            if let Some(forbidden) = result["forbidden_text"].as_array() {
                for text in forbidden {
                    assert!(!source["quote"]
                        .as_str()
                        .unwrap()
                        .contains(text.as_str().unwrap()));
                }
            }
        }
        for binding in result["bindings"].as_array().unwrap() {
            for key in ["consumer", "provider", "dependency"] {
                assert!(citations.contains(&binding[key]), "{id}: uncited binding");
            }
            let source = &oracle["evidence"][binding["consumer"].as_str().unwrap()];
            let provider = &oracle["evidence"][binding["provider"].as_str().unwrap()];
            let dependency = &oracle["evidence"][binding["dependency"].as_str().unwrap()];
            assert_eq!(source["member"], dependency["member"]);
            assert!(result["providers"]
                .as_array()
                .unwrap()
                .contains(&provider["member"]));
            let member = &corpus["members"][provider["member"].as_str().unwrap()];
            for declaration in [
                format!("package = {:?}", member["project"].as_str().unwrap()),
                format!("path = \"../{}\"", member["root"].as_str().unwrap()),
                format!("version = \"={}\"", member["version"].as_str().unwrap()),
            ] {
                assert!(
                    dependency["quote"].as_str().unwrap().contains(&declaration),
                    "{id}: {declaration}"
                );
            }
        }
        if result["status"] != "resolved" {
            assert_eq!(result["providers"], json!([]));
            assert_eq!(result["bindings"], json!([]));
        }
    }
    assert_eq!(
        results["ambiguous-context"]["candidates"],
        json!(["api-v1", "api-v2"])
    );
    assert_eq!(results["unavailable-v3"]["requested_version"], "3.0.0");
    assert_ne!(
        corpus["members"]["api-v1"]["version"],
        corpus["members"]["api-v2"]["version"]
    );
    assert_eq!(
        corpus["members"]["private-api"]["scopes"],
        json!(["private:maintainer"])
    );
    eprintln!(
        "{}",
        json!({"case":"s05a-independent-oracle", "tasks":tasks.len(), "statuses":statuses})
    );
}

#[test]
fn native_source_ingestion_keeps_all_member_versions_distinct() {
    let corpus = load("corpus.json");
    let mut snapshots = BTreeMap::new();
    let mut definitions = BTreeMap::new();
    for (name, member) in corpus["members"].as_object().unwrap() {
        let snapshot =
            Snapshot::capture(member["project"].as_str().unwrap(), &root().join(name)).unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].path, "src/lib.rs");
        let extraction = extract_snapshot(&snapshot);
        assert_eq!(extraction.coverage.len(), 1);
        assert_eq!(extraction.coverage[0].status, SyntaxStatus::Parsed);
        let declarations = extraction
            .entities
            .iter()
            .filter(|entity| entity.kind == EntityKind::Definition)
            .collect::<Vec<_>>();
        let mut names = declarations
            .iter()
            .map(|entity| entity.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(
            names,
            if name == "app" {
                vec!["current", "legacy", "restricted", "unavailable"]
            } else {
                vec!["context"]
            }
        );
        for fact in &extraction.facts {
            let source = &snapshot.contents[&fact.span.file];
            assert_eq!(
                &source[fact.span.byte_start as usize..fact.span.byte_end as usize],
                fact.text
            );
        }
        definitions.insert(
            name.clone(),
            declarations
                .iter()
                .map(|entity| entity.id.clone())
                .collect::<Vec<_>>(),
        );
        snapshots.insert(name.clone(), snapshot.id);
    }
    assert_ne!(snapshots["api-v1"], snapshots["api-v2"]);
    assert_ne!(definitions["api-v1"], definitions["api-v2"]);
    eprintln!(
        "{}",
        json!({"case":"s05a-native-ingestion", "snapshots":snapshots, "definitions":definitions, "parsed_rust_files":4})
    );
}
