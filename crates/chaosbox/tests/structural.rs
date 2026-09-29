//! Exact zero-model baseline: source declarations, never lexical lookalikes.

use std::collections::BTreeSet;

use chaosbox::{GraphReader, Materialization, Pipeline};
use chaosbox_core::{EntityKind, RelationType};
use chaosbox_store::{MemoryReader, MemoryStore, Store};

#[tokio::test]
async fn declarations_publish_without_candidates_or_decisions() {
    let root = tempfile::tempdir().unwrap();
    for (path, text) in [
        (
            "lib.rs",
            "/* fn fake() {} */\npub fn real() {}\nmod inner { fn real() {} }\n",
        ),
        (
            "app.ts",
            "// function fake() {}\nexport function main() {}\nexport const view = () => 1;\n",
        ),
        (
            "main.nix",
            "{\n # fake = 1;\n real = let real = 1; in real;\n text = \"fake = 2;\";\n}\n",
        ),
    ] {
        std::fs::write(root.path().join(path), text).unwrap();
    }
    let policy = chaosbox_core::EffectivePolicy::new(&[], "local", "none").unwrap();
    let (snap, ext, cat) =
        Pipeline::<MemoryStore>::snapshot_extract("r", root.path(), 0, &policy).unwrap();
    assert!(cat.candidates.is_empty());
    let mut pipe = Pipeline::<MemoryStore>::new();
    let build = pipe
        .build_and_publish("r", &snap, &ext, &[], &Materialization::default(), None)
        .await
        .unwrap();
    let definitions: BTreeSet<_> = build
        .edges
        .values()
        .filter(|edge| edge.rel_type == RelationType::Defines)
        .map(|edge| {
            let from = &build.nodes[&edge.from];
            let to = &build.nodes[&edge.to];
            assert_eq!(from.kind, EntityKind::File);
            (to.file.as_str(), to.name.as_str(), to.span.start_line)
        })
        .collect();
    assert_eq!(
        definitions,
        BTreeSet::from([
            ("lib.rs", "real", 2),
            ("lib.rs", "inner", 3),
            ("lib.rs", "real", 3),
            ("app.ts", "main", 2),
            ("app.ts", "view", 3),
            ("main.nix", "real", 3),
            ("main.nix", "text", 4),
        ])
    );
    assert!(build.edges.values().all(|edge| matches!(
        edge.rel_type,
        RelationType::Defines | RelationType::Contains
    )));
    let reader = GraphReader::pinned(MemoryReader::from_store(&pipe.store), "r")
        .await
        .unwrap();
    assert_eq!(
        reader.coverage.as_ref().unwrap().structural_relations,
        build.edges.len()
    );
    assert_eq!(reader.coverage.as_ref().unwrap().decision_relations, 0);
    for edge in build.edges.values() {
        let rows = reader.evidence(&edge.id).await.unwrap();
        let evidence = &rows["evidence"][0];
        assert!(
            evidence["producer"]
                .as_str()
                .unwrap()
                .starts_with("declarations-v1/")
        );
        let citation = &evidence["citation"];
        assert_eq!(citation["snapshot"], snap.id);
        let path = citation["file"].as_str().unwrap();
        assert_eq!(citation["sha256"], snap.file_version(path).unwrap().sha256);
        assert_eq!(
            citation["span"],
            serde_json::to_value(&build.nodes[&edge.to].span).unwrap()
        );
        assert_eq!(
            reader.lookup(&edge.to).await.unwrap().unwrap().span,
            Some(build.nodes[&edge.to].span.clone())
        );
    }
    let staged = pipe.store.export_staged();
    assert!(staged.decisions.is_empty());
    assert!(staged.inferences.is_empty());
    assert_eq!(staged.claims.len(), build.edges.len());
    for evidence in staged.evidence {
        let span = evidence.span.unwrap();
        assert_eq!(
            evidence.text,
            snap.contents[&span.file][span.byte_start as usize..span.byte_end as usize]
        );
    }
}

#[tokio::test]
async fn changed_sources_replace_facts_and_keep_historical_citations() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app.ts");
    let policy = chaosbox_core::EffectivePolicy::new(&[], "local", "none").unwrap();
    std::fs::write(&path, "export function before() {}\n").unwrap();
    let (snap, ext, _) =
        Pipeline::<MemoryStore>::snapshot_extract("r", root.path(), 0, &policy).unwrap();
    let mut pipe = Pipeline::<MemoryStore>::new();
    let first = pipe
        .build_and_publish("r", &snap, &ext, &[], &Materialization::default(), None)
        .await
        .unwrap();
    let pinned = GraphReader::pinned(MemoryReader::from_store(&pipe.store), "r")
        .await
        .unwrap();
    std::fs::write(&path, "export function after() {}\n").unwrap();
    let (next, next_ext, _) =
        Pipeline::<MemoryStore>::snapshot_extract("r", root.path(), 0, &policy).unwrap();
    let second = pipe
        .build_and_publish(
            "r",
            &next,
            &next_ext,
            &[],
            &Materialization::default(),
            Some(first.id.clone()),
        )
        .await
        .unwrap();
    let reader = GraphReader::pinned(MemoryReader::from_store(&pipe.store), "r")
        .await
        .unwrap();
    assert!(reader.search("before", 10).await.unwrap().is_empty());
    assert_eq!(pinned.search("before", 10).await.unwrap().len(), 1);
    assert_eq!(reader.search("after", 10).await.unwrap().len(), 1);
    for rel in first.edges.keys() {
        assert!(
            reader.evidence(rel).await.unwrap()["evidence"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(pipe.store.get(&first.id).unwrap(), first);
    let err = pipe
        .build_and_publish(
            "r",
            &next,
            &ext,
            &[],
            &Materialization::default(),
            Some(second.id.clone()),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid certified"));
    assert_eq!(pipe.store.active("r").unwrap().id, second.id);
    let mut mislabeled = second.clone();
    mislabeled.coverage.as_mut().unwrap().structural_relations = 0;
    let error = pipe
        .store
        .publish(mislabeled, Some(second.id.clone()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("coverage disagrees"));
    assert_eq!(pipe.store.active("r").unwrap().id, second.id);
}
