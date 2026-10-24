//! Disposable-PostgreSQL federation gate; ordinary unit runs do not provision a server.
use std::collections::BTreeMap;
use chaosbox::federation::{
    Backend, ContextRequest, ErrorCode, Grant, Handle, Identity, KnowledgeSource, Policy, Project,
    Reader, Reply, Request, SharingMode,
};
use chaosbox::intelligence::Bundle;
use chaosbox_pg::{reader::PgReader, store::PgStore, PgConfig};
#[path = "support/federation.rs"]
mod fixture;
#[path = "../../chaosbox-pg/tests/support/server.rs"]
mod server;

fn policy() -> Policy {
    Policy {
        version: 1,
        identity: Identity {
            provider: "dejana-atlas".into(),
            owner: "dejana".into(),
            scope: "private:dejana".into(),
        },
        projects: BTreeMap::from([(
            "project-a".into(),
            Project {
                repo: "local-a".into(),
            },
        )]),
        grants: vec![Grant {
            recipient: "can".into(),
            project: "project-a".into(),
            revision: 1,
            mode: SharingMode::AllAdmitted,
            records: vec![],
        }],
    }
}

async fn publish(store: &mut PgStore, bundle: &Bundle, predecessor: Option<&str>) -> String {
    bundle.validate().unwrap();
    store
        .publish_knowledge(
            &bundle.scope,
            &serde_json::to_string(bundle).unwrap(),
            &bundle.records,
            predecessor,
        )
        .await
        .unwrap()
}

async fn missing_database_stays_absent(url: &str) {
    let missing_url = url.replace("dbname=chaosbox", "dbname=chaosbox_missing");
    let missing = PgConfig::new(missing_url.clone()).unwrap();
    std::env::set_var("CHAOSBOX_DATABASE_URL", &missing_url);
    assert!(PgReader::new(missing.clone()).connect().await.is_err());
    assert_eq!(
        Backend::Postgres
            .current("private:dejana")
            .await
            .unwrap_err(),
        ErrorCode::Unavailable
    );
    assert!(
        PgReader::new(missing.clone()).connect().await.is_err(),
        "context read created a database"
    );
    assert_eq!(
        Backend::Postgres
            .snapshot("private:dejana", &"a".repeat(64))
            .await
            .unwrap_err(),
        ErrorCode::Unavailable
    );
    assert!(
        PgReader::new(missing).connect().await.is_err(),
        "historical read created a database"
    );
    std::env::set_var("CHAOSBOX_DATABASE_URL", url);
}

async fn historical_reads_reauthorize(mut store: PgStore) {
    let old = fixture::bundle(
        "dejana",
        &[(
            "shared",
            "We must preserve bounded context citations.",
            "local-a",
        )],
    );
    let original = publish(&mut store, &old, None).await;
    let mut reader = Reader::new(policy(), Backend::Postgres).unwrap();
    let context = Request::Context(ContextRequest {
        project: "project-a".into(),
        query: "context citations".into(),
        limit: 5,
        max_chars: 12_000,
    });
    let Reply::Context(packet) = reader.query("can", &context).await.unwrap() else {
        panic!("context required")
    };
    assert_eq!(packet.snapshot, original);
    let handle = packet.records[0].handle.clone();
    assert_eq!(handle.scope, "private:dejana");
    let next = fixture::bundle(
        "dejana",
        &[
            (
                "shared",
                "We must preserve bounded context citations.",
                "local-a",
            ),
            ("new", "We must keep new context bounded.", "local-a"),
        ],
    );
    let current = publish(&mut store, &next, Some(&original)).await;
    assert_ne!(current, original);
    assert!(Backend::Postgres
        .snapshot("private:can", &original)
        .await
        .unwrap()
        .is_none());
    assert!(Backend::Postgres
        .current("private:can")
        .await
        .unwrap()
        .bundle
        .records
        .is_empty());
    let evidence = Request::Evidence {
        handle: handle.clone(),
        max_chars: 12_000,
    };
    let Reply::Evidence(packet) = reader.query("can", &evidence).await.unwrap() else {
        panic!("evidence required")
    };
    assert_eq!(packet.record.handle, handle);
    assert_eq!(packet.record.statement, old.records[0].statement);
    assert_eq!(packet.evidence, old.records[0].evidence);
    assert!(!packet.evidence.is_empty());
    assert!(packet.receipts.iter().all(|r| r.state_omitted));
    assert_eq!(
        store.knowledge("private:dejana").await.unwrap().unwrap().0,
        current,
        "historical read advanced the current pointer"
    );
    reader.policy.grants.clear();
    assert_eq!(
        reader.query("can", &evidence).await.unwrap_err(),
        ErrorCode::Denied
    );
    reader.policy = policy();
    missing_history_is_unavailable(&reader, handle).await;
    let mut withheld = next;
    fixture::withhold(
        &mut withheld,
        "shared",
        "We must preserve bounded context citations.",
        "local-a",
    );
    publish(&mut store, &withheld, Some(&current)).await;
    assert_eq!(
        reader.query("can", &evidence).await.unwrap_err(),
        ErrorCode::Denied
    );
    let Reply::Context(packet) = reader.query("can", &context).await.unwrap() else {
        panic!("context required")
    };
    assert!(packet
        .records
        .iter()
        .all(|record| record.id != old.records[0].id));
}

async fn missing_history_is_unavailable(reader: &Reader<Backend>, mut handle: Handle) {
    handle.snapshot = "0".repeat(64);
    assert_eq!(
        reader
            .query(
                "can",
                &Request::Evidence {
                    handle,
                    max_chars: 12_000
                }
            )
            .await
            .unwrap_err(),
        ErrorCode::SnapshotUnavailable,
    );
}

#[tokio::test]
#[ignore = "disposable PostgreSQL with pgvector (initdb/pg_ctl/psql)"]
async fn live_pg_federation_is_read_only_scoped_and_snapshot_bound() {
    let server = server::Server::start();
    let url = server.url();
    std::env::set_var("CHAOSBOX_DATABASE_URL", &url);
    let mut store = PgStore::new(PgConfig::new(url.clone()).unwrap());
    // Connect and apply schema before checking the missing database: a
    // broken connection must never masquerade as a successful denial.
    store.migrate().await.unwrap();
    missing_database_stays_absent(&url).await;
    historical_reads_reauthorize(store).await;
}
