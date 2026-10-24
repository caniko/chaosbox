//! Scoped, version-bound workspace impact is reproducible and fails stale.

use std::{collections::BTreeMap, fs, path::Path, process::Command};
use chaosbox::workspace::{capture, Spec};

fn fixture() -> (tempfile::TempDir, Spec) {
    let dir = tempfile::tempdir().unwrap();
    let provider = dir.path().join("provider");
    let consumer = dir.path().join("consumer");
    fs::create_dir(&provider).unwrap();
    fs::create_dir(&consumer).unwrap();
    fs::write(
        provider.join("api.rs"),
        "pub fn context() {}\n// historical evidence only\n",
    )
    .unwrap();
    fs::write(
        consumer.join("plugin.ts"),
        "const result = run('context');\n",
    )
    .unwrap();
    fs::write(
        consumer.join("flake.lock"),
        "{\"nodes\":{\"api\":{\"locked\":{\"rev\":\"0000000000000000000000000000000000000001\"}}}}\n",
    )
    .unwrap();
    let spec = serde_json::from_value(serde_json::json!({
        "scope":"private:test",
        "members":{
            "provider":{"root":provider,"files":["api.rs"]},
            "consumer":{"root":consumer,"files":["plugin.ts","flake.lock"]}
        },
        "endpoints":{
            "response":{"member":"provider","file":"api.rs","quote":"pub fn context() {}"},
            "consumer":{"member":"consumer","file":"plugin.ts","quote":"run('context')"},
            "constraint-source":{"member":"provider","file":"api.rs","quote":"historical evidence only"}
        },
        "bridges":[{"from":"response","to":"consumer","reason":"consumer invokes the context command", "evidence":["response","consumer"], "pin":{"member":"consumer","file":"flake.lock","pointer":"/nodes/api/locked/rev"}}],
        "constraints":[{"id":"historical","statement":"Treat the response as historical evidence.","applies_to":["response"],"evidence":["constraint-source"]}]
    })).unwrap();
    (dir, spec)
}

#[tokio::test]
async fn version_mismatch_is_reported_with_exact_member_builds_and_scoped_constraints() {
    let (_dir, spec) = fixture();
    let artifact = capture(spec).await.unwrap();
    let answer = artifact.impact("private:test", "response", 4, 20).unwrap();
    assert_eq!(answer["status"], "partial");
    assert_eq!(answer["blocked"][0]["status"], "version_mismatch");
    assert_eq!(answer["constraints"][0]["id"], "historical");
    assert_eq!(answer["impacted"].as_array().unwrap().len(), 1);
    assert_eq!(answer["exhaustive"], false);
    assert_eq!(answer["evidence"]["consumer"]["quote"], "run('context')");
    assert_eq!(answer["blocked"][0]["evidence"][1], "consumer");
    assert!(artifact.impact("private:other", "response", 4, 20).is_err());
    let endpoint = &artifact.endpoints["response"];
    assert_eq!(endpoint.build, artifact.members["provider"].build.id);
    assert!(artifact.members["provider"]
        .build
        .nodes
        .contains_key(&endpoint.entity));
}

fn git(root: &Path, args: &[&str]) -> String {
    // Identity/configuration applies only to these disposable test repositories.
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Chaosbox Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[tokio::test]
async fn exact_git_pin_traverses_but_ignored_or_changed_provider_content_does_not() {
    let (_dir, spec) = fixture();
    let root = &spec.members["provider"].root;
    git(root, &["init", "--quiet"]);
    git(root, &["add", "api.rs"]);
    git(root, &["commit", "--quiet", "-m", "fixture"]);
    let revision = git(root, &["rev-parse", "HEAD"]);
    fs::write(
        spec.members["consumer"].root.join("flake.lock"),
        serde_json::json!({"nodes":{"api":{"locked":{"rev":revision}}}}).to_string(),
    )
    .unwrap();
    let artifact = capture(spec.clone()).await.unwrap();
    let answer = artifact.impact("private:test", "response", 4, 20).unwrap();
    assert_eq!(answer["status"], "ready");
    assert_eq!(answer["impacted"].as_array().unwrap().len(), 2);
    assert_eq!(answer["blocked"], serde_json::json!([]));

    // User display preferences must not hide an unclean checkout from the pin check.
    git(root, &["config", "status.showUntrackedFiles", "no"]);
    fs::write(root.join("untracked.rs"), "pub fn extra() {}\n").unwrap();
    assert!(git(root, &["status", "--porcelain"]).is_empty());
    let hidden = capture(spec.clone()).await.unwrap();
    assert_eq!(
        hidden.impact("private:test", "response", 4, 20).unwrap()["blocked"][0]["status"],
        "version_mismatch"
    );
    fs::remove_file(root.join("untracked.rs")).unwrap();
    git(root, &["config", "--unset", "status.showUntrackedFiles"]);

    // A clean Git status cannot authenticate ignored inputs against a commit.
    fs::write(root.join(".git/info/exclude"), "ignored.rs\n").unwrap();
    fs::write(root.join("ignored.rs"), "pub fn ignored() {}\n").unwrap();
    assert!(git(root, &["status", "--porcelain"]).is_empty());
    let mut ignored = spec.clone();
    ignored
        .members
        .get_mut("provider")
        .unwrap()
        .files
        .push("ignored.rs".into());
    let unbound = capture(ignored).await.unwrap();
    assert_eq!(
        unbound.impact("private:test", "response", 4, 20).unwrap()["blocked"][0]["status"],
        "version_mismatch"
    );

    // Git's assume-unchanged flag must not certify bytes absent from the pin.
    git(root, &["update-index", "--assume-unchanged", "api.rs"]);
    fs::write(
        root.join("api.rs"),
        "pub fn context() {}\n// historical evidence only\n// changed\n",
    )
    .unwrap();
    assert!(git(root, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        artifact.impact("private:test", "response", 4, 20).unwrap()["status"],
        "stale"
    );
    let changed = capture(spec).await.unwrap();
    assert_eq!(
        changed.impact("private:test", "response", 4, 20).unwrap()["blocked"][0]["status"],
        "version_mismatch"
    );
}

#[tokio::test]
async fn impact_retains_diamond_bridges_and_respects_hop_boundaries() {
    let (_dir, mut spec) = fixture();
    let local = spec.endpoints.get_mut("consumer").unwrap();
    local.member = "provider".into();
    local.file = "api.rs".into();
    local.quote = "context()".into();
    spec.members.remove("consumer");
    let mut second = spec.bridges[0].clone();
    spec.bridges[0].pin = None;
    second.to = "constraint-source".into();
    second.pin = None;
    spec.bridges.push(second.clone());
    second.from = "consumer".into();
    spec.bridges.push(second);
    let artifact = capture(spec).await.unwrap();
    let answer = artifact.impact("private:test", "response", 2, 20).unwrap();
    assert_eq!(answer["bridges"].as_array().unwrap().len(), 3);
    assert_eq!(answer["impacted"].as_array().unwrap().len(), 3);
    let one_hop = artifact.impact("private:test", "response", 1, 20).unwrap();
    assert_eq!(one_hop["bridges"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn artifact_roundtrip_is_private_create_only_and_scope_checked() {
    use chaosbox::workspace::cli::{run, Command};
    let (dir, spec) = fixture();
    let recipe = dir.path().join("spec.json");
    let output = dir.path().join("artifact.json");
    fs::write(&recipe, serde_json::to_vec(&spec).unwrap()).unwrap();
    run(Command::Capture {
        spec: recipe.clone(),
        output: output.clone(),
    })
    .await
    .unwrap();
    let before = fs::read(&output).unwrap();
    assert!(run(Command::Capture {
        spec: recipe,
        output: output.clone()
    })
    .await
    .is_err());
    assert_eq!(fs::read(&output).unwrap(), before);
    let result = run(Command::Impact {
        artifact: output.clone(),
        changed: "response".into(),
        scope: "private:test".into(),
        max_hops: 4,
        max_nodes: 20,
    })
    .await
    .unwrap();
    assert_eq!(result["status"], "partial");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn changed_deleted_sources_and_tampered_artifacts_cannot_return_current_impact() {
    let (_dir, spec) = fixture();
    let artifact = capture(spec).await.unwrap();
    fs::write(
        artifact.members["provider"].root.join("api.rs"),
        "pub fn changed() {}\n",
    )
    .unwrap();
    let stale = artifact.impact("private:test", "response", 4, 20).unwrap();
    assert_eq!(stale["status"], "stale");
    assert!(stale.get("constraints").is_none());
    fs::remove_file(artifact.members["consumer"].root.join("plugin.ts")).unwrap();
    assert_eq!(
        artifact.impact("private:test", "response", 4, 20).unwrap()["status"],
        "stale"
    );
    let mut tampered = artifact;
    tampered.endpoints.get_mut("response").unwrap().build = "build:foreign".into();
    assert!(tampered.impact("private:test", "response", 4, 20).is_err());
}

#[tokio::test]
async fn local_paths_are_directed_bounded_and_reproducible() {
    let (_dir, mut spec) = fixture();
    spec.members.remove("consumer");
    spec.endpoints.remove("consumer");
    spec.bridges[0].to = "constraint-source".into();
    spec.bridges[0].pin = None;
    spec.bridges[0].evidence = vec!["response".into(), "constraint-source".into()];
    let first = capture(spec.clone()).await.unwrap();
    let clean = capture(spec).await.unwrap();
    assert_eq!(first.id, clean.id);
    let bounded = first.impact("private:test", "response", 4, 1).unwrap();
    assert_eq!(bounded["truncated"], true);
    let complete = first.impact("private:test", "response", 4, 20).unwrap();
    assert_eq!(complete["impacted"].as_array().unwrap().len(), 2);
    let reverse = first
        .impact("private:test", "constraint-source", 4, 20)
        .unwrap();
    assert_eq!(reverse["impacted"].as_array().unwrap().len(), 1);
    assert!(first.impact("private:test", "absent", 4, 20).is_err());
    assert!(first.impact("private:test", "response", 99, 20).is_err());
}

#[tokio::test]
async fn missing_members_unpinned_cross_repo_bridges_and_unmatched_quotes_fail_capture() {
    let (_dir, spec) = fixture();
    let mut missing = spec.clone();
    missing.members = BTreeMap::new();
    assert!(capture(missing).await.is_err());
    let mut unpinned = spec.clone();
    unpinned.bridges[0].pin = None;
    assert!(capture(unpinned).await.is_err());
    let mut wrong_quote = spec;
    wrong_quote.endpoints.get_mut("response").unwrap().quote = "invented()".into();
    assert!(capture(wrong_quote).await.is_err());
}

#[tokio::test]
async fn overlapping_source_quotes_are_ambiguous() {
    let (_dir, mut spec) = fixture();
    fs::write(
        spec.members["provider"].root.join("api.rs"),
        "pub fn aaaa() {}\n// historical evidence only\n",
    )
    .unwrap();
    spec.endpoints.get_mut("response").unwrap().quote = "aaa".into();
    assert!(capture(spec)
        .await
        .unwrap_err()
        .to_string()
        .contains("not unique"));
}
