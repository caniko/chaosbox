//! Protobuf, source-context and Unicode boundary regression fixtures.
use std::{collections::BTreeMap, fs};

use chaosbox_core::RelationType;
use chaosbox_extract::{
    compiler::{AnalysisInputs, AnalysisSettings, Receipt, normalize},
    Snapshot,
};
use protobuf::{Message, MessageField};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, ToolInfo};

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.ts"), "const 🦀 = 0; foo(); foo;\n").unwrap();
    fs::write(dir.path().join("b.ts"), "export function foo() {}\n").unwrap();
    fs::write(dir.path().join("tsconfig.json"), "{\"strict\":true}\n").unwrap();
    dir
}

fn occurrence(symbol: &str, range: &[i32], definition: bool) -> Occurrence {
    Occurrence {
        symbol: symbol.into(),
        range: range.into(),
        symbol_roles: i32::from(definition),
        ..Default::default()
    }
}

const FOO: &str = "scip-typescript npm demo 1.0.0 b.ts/foo().";

fn index(root: &std::path::Path) -> Index {
    Index {
        metadata: MessageField::some(Metadata {
            project_root: url::Url::from_directory_path(root).unwrap().to_string(),
            tool_info: MessageField::some(ToolInfo {
                name: "scip-typescript".into(),
                version: "0.4.0".into(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        documents: vec![
            Document {
                relative_path: "a.ts".into(),
                position_encoding: PositionEncoding::UTF16CodeUnitOffsetFromLineStart.into(),
                occurrences: vec![
                    occurrence(FOO, &[0, 14, 17], false),
                    occurrence(FOO, &[0, 21, 24], false),
                ],
                ..Default::default()
            },
            Document {
                relative_path: "b.ts".into(),
                position_encoding: PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into(),
                occurrences: vec![occurrence(FOO, &[0, 16, 19], true)],
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn settings() -> AnalysisSettings {
    AnalysisSettings {
        command: vec!["indexer".into(), "--output".into(), "{index}".into()],
        declared: BTreeMap::from_iter([
            ("toolchain".into(), "fixture".into()),
            ("configuration".into(), "tsconfig.json".into()),
            ("target".into(), "ES2022".into()),
            ("features".into(), "none".into()),
        ]),
        unspecified_encoding: None,
    }
}

#[test]
fn protobuf_references_have_exact_unicode_spans_and_stable_global_anchors() {
    let dir = project();
    let bytes = index(dir.path()).write_to_bytes().unwrap();
    let inputs = AnalysisInputs::capture("demo", dir.path(), &[], &[]).unwrap();
    let receipt = Receipt::seal(inputs, settings(), &bytes).unwrap();
    let snapshot = Snapshot::capture("demo", dir.path()).unwrap();
    let output = normalize(&snapshot, &receipt, &bytes).unwrap();
    let mut refs: Vec<_> = output
        .facts
        .iter()
        .filter(|f| f.rel_type == RelationType::References)
        .collect();
    refs.sort_by_key(|f| f.span.byte_start);
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].text, "foo");
    assert_eq!(refs[0].span.byte_start, 16);
    assert_eq!(refs[0].span.start_col, 14); // Unicode scalars, 1-based
    assert!(!output
        .facts
        .iter()
        .any(|f| f.rel_type == RelationType::Calls));
    let original = &output.entities[0];
    fs::write(dir.path().join("unrelated.ts"), "export const x = 1;\n").unwrap();
    let next = Receipt::seal(
        AnalysisInputs::capture("demo", dir.path(), &[], &[]).unwrap(),
        settings(),
        &bytes,
    )
    .unwrap();
    let revised = normalize(
        &Snapshot::capture("demo", dir.path()).unwrap(),
        &next,
        &bytes,
    )
    .unwrap();
    assert_ne!(original.id, revised.entities[0].id);
    assert_eq!(
        original.compiler.as_ref().unwrap().anchor,
        revised.entities[0].compiler.as_ref().unwrap().anchor
    );
    assert_eq!(output.coverage.references, 2);
    assert_eq!(output.coverage.calls, "unsupported");
}

#[test]
fn receipts_reject_source_config_index_and_scope_drift() {
    let dir = project();
    let bytes = index(dir.path()).write_to_bytes().unwrap();
    let receipt = Receipt::seal(
        AnalysisInputs::capture("demo", dir.path(), &[], &[]).unwrap(),
        settings(),
        &bytes,
    )
    .unwrap();
    receipt.verify(dir.path()).unwrap();
    fs::write(dir.path().join("tsconfig.json"), "{}\n").unwrap();
    assert!(receipt.verify(dir.path()).is_err());
    let snapshot = Snapshot::capture("demo", dir.path()).unwrap();
    assert!(normalize(&snapshot, &receipt, b"bad index").is_err());
    fs::write(dir.path().join("a.ts"), "foo();\n").unwrap();
    assert!(normalize(
        &Snapshot::capture("demo", dir.path()).unwrap(),
        &receipt,
        &bytes
    )
    .is_err());
}

#[test]
fn malformed_ranges_paths_and_unknown_encodings_fail_closed() {
    let dir = project();
    for variant in 0..5 {
        let mut idx = index(dir.path());
        match variant {
            0 => idx.documents[0].relative_path = "../a.ts".into(),
            1 => idx.documents[0].occurrences[0].range = vec![0, 7, 8], // splits surrogate pair
            2 => idx.documents[0].occurrences[0].range = vec![-1, 0, 1],
            3 => idx.documents[0].position_encoding = protobuf::EnumOrUnknown::from_i32(99),
            _ => {
                idx.documents[0].position_encoding =
                    PositionEncoding::UnspecifiedPositionEncoding.into();
            }
        }
        let bytes = idx.write_to_bytes().unwrap();
        let receipt = Receipt::seal(
            AnalysisInputs::capture("demo", dir.path(), &[], &[]).unwrap(),
            settings(),
            &bytes,
        )
        .unwrap();
        assert!(
            normalize(
                &Snapshot::capture("demo", dir.path()).unwrap(),
                &receipt,
                &bytes
            )
            .is_err(),
            "variant {variant}"
        );
    }
}

#[test]
fn locals_are_document_scoped_and_missing_definitions_are_not_invented() {
    let dir = project();
    let mut idx = index(dir.path());
    idx.documents[0].occurrences[0].symbol = "local 0".into();
    idx.documents[1].occurrences[0].symbol = "local 0".into();
    let bytes = idx.write_to_bytes().unwrap();
    let receipt = Receipt::seal(
        AnalysisInputs::capture("demo", dir.path(), &[], &[]).unwrap(),
        settings(),
        &bytes,
    )
    .unwrap();
    let output = normalize(
        &Snapshot::capture("demo", dir.path()).unwrap(),
        &receipt,
        &bytes,
    )
    .unwrap();
    assert_eq!(output.coverage.references, 0);
    assert_eq!(output.coverage.unresolved_references, 2);
    assert_eq!(output.coverage.definitions, 1);
}

#[test]
fn real_compiler_indexes_preserve_aliases_implementations_and_configuration_limits() {
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/indexer-smoke");
    for (artifact, project, definitions, implementations) in [
        ("rust-default.scip", "rust", 18, 0),
        ("rust-extra.scip", "rust", 19, 0),
        ("typescript.scip", "typescript", 18, 2),
    ] {
        let root = fixtures.join(project).canonicalize().unwrap();
        let bytes = fs::read(fixtures.join(artifact)).unwrap();
        let raw = Index::parse_from_bytes(&bytes).unwrap();
        let mut inputs = AnalysisInputs::capture("probe", &root, &[], &[]).unwrap();
        // Archived producer fixture, not a fresh execution receipt: retain its
        // original project URL while reading identical checked-in source bytes.
        inputs
            .root
            .clone_from(&raw.metadata.as_ref().unwrap().project_root);
        let mut settings = settings();
        if project == "typescript" {
            settings.unspecified_encoding = Some(chaosbox_extract::compiler::Encoding::Utf16);
        }
        let receipt = Receipt::seal(inputs, settings, &bytes).unwrap();
        let output = normalize(
            &Snapshot::capture("probe", &root).unwrap(),
            &receipt,
            &bytes,
        )
        .unwrap();
        assert_eq!(output.coverage.definitions, definitions, "{artifact}");
        assert_eq!(
            output.coverage.implementations, implementations,
            "{artifact}"
        );
        assert!(output.coverage.references > 0);
        assert!(output.coverage.unresolved_references > 0);
        assert_eq!(output.coverage.typecheck, "unknown");
        assert_eq!(output.coverage.configuration_membership, "unknown");
        let aliases: Vec<_> = output
            .entities
            .iter()
            .filter(|e| e.name == "renamed")
            .collect();
        assert_eq!(aliases.len(), if project == "rust" { 6 } else { 5 });
        for alias in aliases {
            let shadowed = if project == "rust" {
                [10, 11].contains(&alias.span.start_line)
            } else {
                alias.span.start_line == 6
            };
            let identity = alias.compiler.as_ref().unwrap();
            assert_eq!(identity.local, shadowed);
            assert_eq!(identity.symbol.contains("answer"), !shadowed);
        }
    }
}

#[test]
fn typed_ranges_and_ambiguous_targets_are_handled_without_guessing() {
    let dir = project();
    let mut idx = index(dir.path());
    let reference = &mut idx.documents[0].occurrences[0];
    reference.range.clear();
    reference.set_single_line_range(scip::types::SingleLineRange {
        line: 0,
        start_character: 14,
        end_character: 17,
        ..Default::default()
    });
    idx.documents[0].occurrences[1].symbol_roles = 1;
    let bytes = idx.write_to_bytes().unwrap();
    let receipt = Receipt::seal(
        AnalysisInputs::capture("demo", dir.path(), &[], &[]).unwrap(),
        settings(),
        &bytes,
    )
    .unwrap();
    let output = normalize(
        &Snapshot::capture("demo", dir.path()).unwrap(),
        &receipt,
        &bytes,
    )
    .unwrap();
    assert_eq!(output.coverage.references, 0);
    assert_eq!(output.coverage.ambiguous_references, 1);
    assert_eq!(output.coverage.definitions, 2);
}
