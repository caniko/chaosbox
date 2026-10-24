//! Real-journal scratch provenance and cleanup intelligence invariants.
use chaosbox::scratch::{Ledger, QueryPath, Request};
use serde_json::{json, Value};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn request(root: &std::path::Path, events: Value) -> Request {
    let mut value = json!({"version":1,"scope":"private:can","host":"test-host","root":root});
    value["events"] = events;
    serde_json::from_value(value).unwrap()
}

fn identity(path: &std::path::Path) -> Value {
    let m = std::fs::symlink_metadata(path).unwrap();
    json!({"dev":m.dev().to_string(),"ino":m.ino().to_string(),
        "birth_ns":m.created().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos().to_string()})
}

#[test]
fn provenance_survives_restart_and_command_success_does_not_release_work() {
    let work = tempfile::tempdir().unwrap();
    std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("experiment");
    std::fs::create_dir(&path).unwrap();
    let events = json!([
        {"id":"begin","kind":"begin","invocation":"call-1","session":"ses_a","message":"msg_a",
         "tool":"shell","repo":"chaosbox","command":"python experiment.py","cwd":root.path(),
         "sources":[{"record":{"id":"msg_user","type":"user","text":"Reproduce the failure; preserve the resulting patch."},"pointer":"/text"}]},
        {"id":"observe","kind":"observe","path":path,"identity":identity(&path),"present":true,"activity":true,"owners":["call-1"]},
        {"id":"end","kind":"end","invocation":"call-1","outcome":"exited","exit":0}
    ]);
    let input = request(root.path(), events);
    let mut ledger = Ledger::open(work.path(), "private:can").unwrap();
    ledger.record(&input).unwrap();
    ledger.record(&input).unwrap(); // retries do not duplicate provenance
    drop(ledger);
    let ledger = Ledger::read(work.path(), "private:can").unwrap();
    let view = ledger
        .query("test-host", root.path(), &[path], 100)
        .unwrap();
    assert_eq!(view["items"][0]["entries"][0]["disposition"], "open");
    assert_eq!(view["items"][0]["entries"][0]["invocations"][0]["exit"], 0);
    assert!(view["items"][0]["blocked"].as_bool().unwrap());
    assert!(view.to_string().contains("preserve the resulting patch"));
    let hash = view["items"][0]["entries"][0]["invocations"][0]["sources"][0]["hash"]
        .as_str()
        .unwrap();
    assert_eq!(
        ledger.evidence(hash).unwrap()["text"],
        "Reproduce the failure; preserve the resulting patch."
    );
    assert_eq!(view["generation"], 3);
}

#[test]
fn quarantine_keeps_identity_and_replacement_does_not_inherit_release() {
    let work = tempfile::tempdir().unwrap();
    std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cache");
    let quarantine = root.path().join(".doty-quarantine");
    std::fs::create_dir(&path).unwrap();
    std::fs::create_dir(&quarantine).unwrap();
    let mut ledger = Ledger::open(work.path(), "private:can").unwrap();
    let original = identity(&path);
    ledger.record(&request(root.path(), json!([
        {"id":"observe","kind":"observe","path":path,"identity":original,"present":true,"activity":true},
        {"id":"live","kind":"coverage","observer":"observer-a","healthy":true,"detail":"watching"}
    ]))).unwrap();
    ledger
        .annotate(
            "test-host",
            root.path(),
            &path,
            "released",
            "Generated cache",
            None,
        )
        .unwrap();
    let at = quarantine.join("cache");
    std::fs::rename(&path, &at).unwrap();
    ledger.record(&request(root.path(), json!([
        {"id":"missing","kind":"observe","path":path,"identity":original,"present":false,"activity":false}
    ]))).unwrap();
    let located = vec![QueryPath {
        path: path.clone(),
        at: Some(at),
    }];
    let quarantined = ledger
        .query_at("test-host", root.path(), &located, 100)
        .unwrap();
    assert_eq!(quarantined["items"][0]["blocked"], false);
    assert_eq!(
        quarantined["items"][0]["entries"][0]["identity_matches"],
        true
    );
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        ledger
            .query("test-host", root.path(), &[path], 100)
            .unwrap()["items"][0]["blocked"],
        true
    );
}

#[test]
fn actual_node_observer_and_rust_cli_capture_unfinished_work() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let work = fixture.path().join("custody");
    std::fs::create_dir(&root).unwrap();
    let runtime = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/chaosbox-scratch/runtime.mjs");
    let code = r"
        import { pathToFileURL } from 'node:url';
        import { mkdir } from 'node:fs/promises';
        import { join } from 'node:path';
        const { createTracker } = await import(pathToFileURL(process.env.RUNTIME));
        const tracker = await createTracker({ root:process.env.ROOT,work:process.env.WORK,scope:'private:can',host:'test-host',chaosboxBin:process.env.BINARY });
        try {
          const event = {tool:'shell',id:'call',sessionID:'ses_e2e',messageID:'msg_e2e',input:{command:'python reproduction.py'}};
          await tracker.before(event,'chaosbox',process.env.ROOT,[{id:'msg_user',type:'user',text:'Reproduce the bug and keep the patch for integration.'}]);
          const path = join(process.env.ROOT,'indirect-work');
          await mkdir(path);
          await tracker.reconcile();
          await tracker.after({...event,status:'completed',result:{metadata:{exit:0}}});
          await tracker.note(path,'Patch still needs integration');
          const result = await tracker.explain(path);
          if (!result.items[0].blocked || !JSON.stringify(result).includes('keep the patch')) throw new Error('Unfinished work lost');
        } finally { await tracker.close(); }
    ";
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "-e", code])
        .env("RUNTIME", runtime)
        .env("ROOT", &root)
        .env("WORK", &work)
        .env("BINARY", env!("CARGO_BIN_EXE_chaosbox"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ledger = Ledger::read(&work, "private:can").unwrap();
    let view = ledger
        .query("test-host", &root, &[root.join("indirect-work")], 100)
        .unwrap();
    assert_eq!(
        view["items"][0]["entries"][0]["disposition"],
        "needs-finalization"
    );
}

#[test]
fn unfinished_descendants_and_new_activity_override_a_release() {
    let work = tempfile::tempdir().unwrap();
    std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("work");
    let child = parent.join("patch");
    std::fs::create_dir_all(&child).unwrap();
    let mut ledger = Ledger::open(work.path(), "private:can").unwrap();
    ledger.record(&request(root.path(), json!([
        {"id":"parent","kind":"observe","path":parent,"identity":identity(&parent),"present":true,"activity":true},
        {"id":"child","kind":"observe","path":child,"identity":identity(&child),"present":true,"activity":true},
        {"id":"live","kind":"coverage","observer":"observer-a","healthy":true,"detail":"watching"}
    ]))).unwrap();
    ledger
        .annotate(
            "test-host",
            root.path(),
            &parent,
            "released",
            "Disposable cache",
            None,
        )
        .unwrap();
    ledger
        .annotate(
            "test-host",
            root.path(),
            &child,
            "needs-finalization",
            "Integrate patch",
            None,
        )
        .unwrap();
    let view = ledger
        .query("test-host", root.path(), std::slice::from_ref(&parent), 100)
        .unwrap();
    assert_eq!(view["items"][0]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(view["items"][0]["blocked"], true);
    ledger
        .annotate(
            "test-host",
            root.path(),
            &child,
            "released",
            "Patch integrated",
            Some("commit:abc"),
        )
        .unwrap();
    assert_eq!(
        ledger
            .query("test-host", root.path(), std::slice::from_ref(&parent), 100)
            .unwrap()["items"][0]["blocked"],
        false
    );
    ledger.record(&request(root.path(), json!([
        {"id":"resumed","kind":"observe","path":child,"identity":identity(&child),"present":true,"activity":true}
    ]))).unwrap();
    assert_eq!(
        ledger
            .query("test-host", root.path(), &[parent], 100)
            .unwrap()["items"][0]["blocked"],
        true
    );
}

#[test]
fn reads_do_not_initialize_missing_state_and_boundaries_fail_closed() {
    let work = tempfile::tempdir().unwrap();
    std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(Ledger::read(work.path(), "private:can").is_err());
    assert!(!work.path().join("scratch.sqlite").exists());
    let root = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::open(work.path(), "private:can").unwrap();
    assert!(ledger.record(&request(root.path(), json!([
        {"id":"escape","kind":"observe","path":root.path().join("../escape"),"identity":{"dev":"1","ino":"2","birth_ns":"3"},"present":true,"activity":true}
    ]))).is_err());
    assert!(Ledger::read(work.path(), "private:other").is_err());
    let bad = request(
        root.path(),
        json!([
            {"id":"bad-source","kind":"begin","invocation":"call-x","session":"ses_x","message":"msg_x","tool":"shell","repo":"chaosbox","command":"true","cwd":root.path(),
             "sources":[{"record":{"id":"msg_summary","type":"compaction","text":"Already done"},"pointer":"/text"}]}
        ]),
    );
    assert!(ledger.record(&bad).is_err());
}

#[test]
fn orphaned_execution_needs_explicit_resolution_before_release() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("scratch");
    let work = fixture.path().join("private").join("custody");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("unfinished");
    std::fs::create_dir(&path).unwrap();
    let mut ledger = Ledger::open(&work, "private:can").unwrap();
    ledger.record(&request(&root,json!([
        {"id":"begin","kind":"begin","invocation":"orphan","session":"ses_test","message":"msg_test","tool":"shell","repo":"chaosbox","command":"experiment","cwd":root},
        {"id":"observe","kind":"observe","path":path,"identity":identity(&path),"present":true,"activity":true,"owners":["orphan"]},
        {"id":"unknown","kind":"end","invocation":"orphan","outcome":"unknown"},
        {"id":"live","kind":"coverage","observer":"test","healthy":true,"detail":"watching"}
    ]))).unwrap();
    assert!(ledger
        .annotate("test-host", &root, &path, "released", "Disposable", None)
        .is_err());
    ledger
        .resolve(
            "test-host",
            &root,
            "orphan",
            "Operator checked the process has stopped",
        )
        .unwrap();
    let view = ledger
        .query("test-host", &root, std::slice::from_ref(&path), 100)
        .unwrap();
    assert_eq!(view["items"][0]["blocked"], true);
    assert_eq!(
        view["items"][0]["entries"][0]["invocations"][0]["exit"],
        Value::Null
    );
    assert!(view.to_string().contains("Operator checked"));
    ledger
        .annotate(
            "test-host",
            &root,
            &path,
            "released",
            "Outputs reviewed and disposable",
            None,
        )
        .unwrap();
    assert_eq!(
        ledger.query("test-host", &root, &[path], 100).unwrap()["items"][0]["blocked"],
        false
    );
}
