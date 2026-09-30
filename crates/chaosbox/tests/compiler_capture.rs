//! Fresh process capture brackets inputs and never replaces an existing artifact.
use std::{
    fs,
    path::{Path, PathBuf},
};
use chaosbox::compiler::{run, Command};
use protobuf::{Message, MessageField};
use scip::types::{Document, Index, Metadata, Occurrence, PositionEncoding, ToolInfo};

fn capture_command(work: &Path, mode: &str) -> (Command, PathBuf, PathBuf) {
    let root = work.join("source");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a.rs"), "pub fn item() {}\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'fixture'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: url::Url::from_directory_path(&root).unwrap().to_string(),
            tool_info: MessageField::some(ToolInfo {
                name: "fixture-indexer".into(),
                version: "1".into(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        documents: vec![Document {
            relative_path: "a.rs".into(),
            position_encoding: PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into(),
            occurrences: vec![Occurrence {
                symbol: "fixture cargo fixture 0.1.0 item().".into(),
                range: vec![0, 7, 11],
                symbol_roles: 1,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let template = work.join("template.scip");
    fs::write(&template, index.write_to_bytes().unwrap()).unwrap();
    let output = work.join("artifact");
    let script = r"const fs = require('node:fs');
fs.copyFileSync(process.argv[1], process.argv[2]);
const mode = process.argv[3];
if (mode === 'source') fs.appendFileSync('a.rs', '// changed during execution\n');
if (mode === 'config') fs.appendFileSync('Cargo.toml', '\n[features]\nextra = []\n');
if (mode === 'failure') process.exit(2);
if (mode === 'malformed') fs.writeFileSync(process.argv[2], 'invalid protobuf');
";
    (
        Command::Capture {
            path: root.clone(),
            repo: "fixture".into(),
            source_paths: vec![],
            inputs: vec![],
            context: ["toolchain", "configuration", "target", "features"]
                .into_iter()
                .map(|key| (key.into(), "test fixture".into()))
                .collect(),
            unspecified_encoding: None,
            output: output.clone(),
            timeout_seconds: 10,
            command: vec![
                "node".into(),
                "-e".into(),
                script.into(),
                template.to_str().unwrap().into(),
                "{index}".into(),
                mode.into(),
            ],
        },
        root,
        output,
    )
}

#[tokio::test]
async fn capture_inspect_import_and_existing_artifact_guard_use_the_same_receipt() {
    let work = tempfile::tempdir().unwrap();
    let (command, root, output) = capture_command(work.path(), "ok");
    let captured = run(command).await.unwrap();
    assert_eq!(captured["coverage"]["definitions"], 1);
    let inspected = run(Command::Inspect {
        artifact: output.clone(),
        path: root.clone(),
    })
    .await
    .unwrap();
    assert_eq!(inspected, captured["coverage"]);
    let receipt = fs::read(output.join("receipt.json")).unwrap();
    let snapshot = chaosbox_extract::Snapshot::capture("fixture", &root).unwrap();
    let mut extraction = chaosbox_extract::extract_snapshot(&snapshot);
    chaosbox::compiler::attach(&output, &root, &snapshot, &mut extraction).unwrap();
    assert_eq!(extraction.compiler.unwrap().definitions, 1);
    let loaded = chaosbox::compiler::load_receipt(&output).unwrap();
    let attempt = Command::Capture {
        path: root.clone(),
        repo: "fixture".into(),
        source_paths: vec![],
        inputs: vec![],
        context: loaded.settings.declared.into_iter().collect(),
        unspecified_encoding: None,
        output: output.clone(),
        timeout_seconds: 10,
        command: loaded.settings.command,
    };
    assert!(run(attempt).await.is_err());
    assert_eq!(fs::read(output.join("receipt.json")).unwrap(), receipt);
    fs::write(root.join("Cargo.toml"), "changed\n").unwrap();
    assert!(run(Command::Inspect {
        artifact: output,
        path: root
    })
    .await
    .is_err());
}

#[tokio::test]
async fn changed_inputs_failed_producer_and_bad_protobuf_never_create_receipts() {
    for mode in ["source", "config", "failure", "malformed"] {
        let work = tempfile::tempdir().unwrap();
        let (command, _, output) = capture_command(work.path(), mode);
        let error = run(command).await.unwrap_err().to_string();
        if matches!(mode, "source" | "config") {
            assert!(error.contains("inputs changed"), "{error}");
        }
        assert!(!output.exists(), "{mode} created an artifact");
    }
}
