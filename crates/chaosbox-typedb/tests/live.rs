//! Live `TypeDB` backend tests: full write/publish/readback cycle, decision
//! supersedure, predecessor guards, and concurrent-publication races against
//! a real server.
//!
//! Server address comes from `CHAOSBOX_TYPEDB_ADDR` (legacy alias
//! `TYPEDB_ADDR`, default `127.0.0.1:1729`); username from
//! `CHAOSBOX_TYPEDB_USER`/`TYPEDB_USERNAME` (default `admin`); password from
//! `CHAOSBOX_TYPEDB_PASSWORD_FILE` or `TYPEDB_PASSWORD` — and never from a
//! default, because the driver reports an absent server and rejected
//! credentials as the same connection error: only the operator knows which
//! was intended, so a usable secret must be configured explicitly.
//!
//! Eligibility is classified before the server is touched, so nothing that
//! fails later can be reclassified as a skip afterwards. Exactly two cases
//! skip, and neither can be mistaken for a pass: nothing is listening at the
//! address, or a server is reachable but no password was ever configured
//! (authentication is the only thing that could have run). Every other
//! outcome — configured credentials the server rejects, a configured secret
//! that cannot be read, a failed migration, a failed conformance check — is
//! a failure, because turning it green would pass off a masked error as
//! evidence. Under `CHAOSBOX_REQUIRE_TYPEDB` (set by
//! `scripts/test-typedb.sh`) nothing skips at all: a missing server, absent
//! credentials, a failed migration or a conformance failure all fail. A skip
//! is NOT conformance evidence (see the execution ledger); the named gate
//! runs these with a server present.

use chaosbox_core::{
    Candidate, Claim, Decision, DecisionOutcome, Entity, EntityKind, Evidence, EvidenceClass,
    GraphBuild, Relation, RelationScope, RelationType, SnapshotFile, SourceSpan,
};
use chaosbox_store::{GraphQueries, Store, check_conformance};
use chaosbox_typedb::reader::TypeDbReader;
use chaosbox_typedb::store::{TypeDbConfig, TypeDbStore};

fn addr() -> String {
    // The CLI contract is authoritative; `TYPEDB_ADDR` stays as the
    // legacy alias so an existing invocation keeps pointing at its server.
    std::env::var("CHAOSBOX_TYPEDB_ADDR")
        .or_else(|_| std::env::var("TYPEDB_ADDR"))
        .unwrap_or_else(|_| "127.0.0.1:1729".into())
}

/// Parse the required-server gate from a value. Split out of the reader so
/// the rule is testable without mutating process environment (which races
/// across a parallel test run).
fn required_from(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let v = v.trim();
        v.eq_ignore_ascii_case("1")
            || v.eq_ignore_ascii_case("true")
            || v.eq_ignore_ascii_case("yes")
    })
}

/// Whether anything is listening at `addr`.
///
/// The driver reports both "nothing is listening" and "the server rejected
/// our credentials" as the same connection error, so the error text cannot
/// distinguish an absent server from a rejected one. Reachability is
/// therefore measured directly: if the address accepts a TCP connection a
/// server is present, so a failed migration there deserves a real answer
/// instead of a skip.
fn server_reachable(addr: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    addr.to_socket_addrs().is_ok_and(|mut addrs| {
        addrs.any(|sa| TcpStream::connect_timeout(&sa, Duration::from_secs(2)).is_ok())
    })
}

/// The operator's password configuration, in three honest states.
///
/// The distinction is the gate: never having been given a credential is an
/// environment gap that may skip (nothing beyond authentication could run),
/// while a credential that was named but yields no usable secret is a broken
/// gate and must fail. Collapsing the latter into "unconfigured" is exactly
/// how a typo'd secret path turns into a passing skip — and how a fallback
/// default password could end up authenticating somewhere it should not.
enum Credentials {
    /// No password source was ever configured.
    Unconfigured,
    /// A usable secret. Never printed, never logged.
    Ready(String),
    /// A password source was configured but produced no usable secret. The
    /// reason names the source and the problem, never the secret itself.
    Broken(String),
}

impl Credentials {
    /// Human-readable state for diagnostics; omits the secret.
    fn describe(&self) -> String {
        match self {
            Credentials::Unconfigured => "no password source configured".into(),
            Credentials::Ready(_) => "credentials configured".into(),
            Credentials::Broken(reason) => reason.clone(),
        }
    }
}

/// Credential resolution, split from the environment so it is testable
/// without mutating process environment (which races across a parallel
/// test run).
fn credentials_from(
    password_file: Option<&std::ffi::OsString>,
    password: Option<&std::ffi::OsString>,
) -> Credentials {
    if let Some(path) = password_file {
        let path = std::path::Path::new(path);
        return match std::fs::read_to_string(path) {
            Ok(raw) if !raw.trim().is_empty() => Credentials::Ready(raw.trim().to_owned()),
            Ok(_) => Credentials::Broken(format!(
                "password file {} is configured but empty",
                path.display()
            )),
            Err(e) => Credentials::Broken(format!(
                "password file {} is configured but unreadable: {e}",
                path.display()
            )),
        };
    }
    match password {
        Some(p) => {
            let trimmed = p.to_string_lossy().trim().to_owned();
            if trimmed.is_empty() {
                Credentials::Broken("TYPEDB_PASSWORD is configured but empty".into())
            } else {
                Credentials::Ready(trimmed)
            }
        }
        None => Credentials::Unconfigured,
    }
}

/// Credential configuration as the tests see it.
///
/// Supplied-but-rejected credentials are a failure: that is a broken gate
/// trying to go green. Never having been given any is an environment gap,
/// and without a session nothing beyond authentication can even run — so it
/// skips and says plainly that live conformance was not proven. The
/// `CHAOSBOX_REQUIRE_TYPEDB` gate admits no skip either way.
fn credentials_from_env() -> Credentials {
    credentials_from(
        std::env::var_os("CHAOSBOX_TYPEDB_PASSWORD_FILE").as_ref(),
        std::env::var_os("TYPEDB_PASSWORD").as_ref(),
    )
}

/// Per-run database name for the publish tests: they exercise the
/// fresh-database predecessor path (no active pointer yet), and a rerun must
/// not inherit the previous run's pointer — the driver has no database
/// delete, so each process gets a fresh database instead (leftovers
/// accumulate; CI runs on an ephemeral server).
fn test_db(base: &str) -> String {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!("{base}_{}_{epoch}", std::process::id())
}

fn config(db: &str) -> TypeDbConfig {
    let username = std::env::var("CHAOSBOX_TYPEDB_USER")
        .or_else(|_| std::env::var("TYPEDB_USERNAME"))
        .unwrap_or_else(|_| "admin".into());
    // No default password: falling back to a well-known secret would mask
    // the difference between "no credentials" and "bad credentials" — the
    // very ambiguity this gate exists to close. `connected_store` classifies
    // every other state before a test can construct a store, so reaching
    // here without a ready credential is a bug in the gate itself.
    let password = match credentials_from_env() {
        Credentials::Ready(password) => password,
        other => panic!(
            "store configured without usable credentials: {}",
            other.describe()
        ),
    };
    TypeDbConfig {
        address: addr(),
        username,
        password,
        database: db.into(),
    }
}

async fn connected_store(db: &str) -> Option<TypeDbStore> {
    let required = required_from(std::env::var("CHAOSBOX_REQUIRE_TYPEDB").as_deref().ok());

    // Classify eligibility BEFORE touching the server. Deciding after a
    // failed migration (as the previous version did) let that failure become
    // a passing skip whenever credentials happened to be unconfigured —
    // including a genuine schema/migration bug on a server whose default
    // credentials worked. Reachability and credential configuration are
    // measured independently here because the driver conflates "no server"
    // with "rejected credentials", so it cannot classify its own errors.
    if !server_reachable(&addr()) {
        assert!(
            !required,
            "CHAOSBOX_REQUIRE_TYPEDB is set: nothing is listening at {}",
            addr()
        );
        println!("SKIP (no server at {})", addr());
        return None;
    }
    match credentials_from_env() {
        Credentials::Ready(_) => {}
        Credentials::Unconfigured => {
            assert!(
                !required,
                "CHAOSBOX_REQUIRE_TYPEDB is set: a server is reachable at {} but no \
                 credentials were configured",
                addr()
            );
            println!(
                "SKIP (server at {} reachable, but no credentials configured): live \
                 conformance NOT proven, this is not a pass",
                addr()
            );
            return None;
        }
        Credentials::Broken(reason) => panic!(
            "credentials were configured for the server at {} but are unusable ({reason}); \
             a broken credential is a failure, never an environment gap",
            addr()
        ),
    }

    let mut s = TypeDbStore::new(config(db));
    s.migrate().await.unwrap_or_else(|e| {
        panic!(
            "live TypeDB migration failed at {} with a reachable server and configured \
             credentials; refusing to report that as a passing skip: {e}",
            addr()
        )
    });
    Some(s)
}

fn span(file: &str) -> SourceSpan {
    SourceSpan::point(file, 1, 1, 0)
}

fn ent(repo: &str, snap: &str, file: &str, name: &str) -> Entity {
    Entity::new(EntityKind::Symbol, repo, snap, file, name, name, span(file))
}

fn decision(candidate_id: &str, question: &str, cache_key: &str) -> Decision {
    Decision {
        id: format!("dec:{candidate_id}:{question}"),
        candidate_id: candidate_id.into(),
        question_id: question.into(),
        outcome: DecisionOutcome::Accepted,
        evidence_class: EvidenceClass::Extracted,
        model_requested: "jev-1.13.0".into(),
        model_returned: "jev-1.13.0".into(),
        confidence: Some(0.9),
        probability: Some(0.8),
        cache_key: cache_key.into(),
    }
}

fn candidate(id: &str, from: &str, to: &str) -> Candidate {
    Candidate {
        id: id.into(),
        rel_type: RelationType::Calls,
        from_entity: from.into(),
        to_entity: to.into(),
        reason: "structural".into(),
        state_excerpt: "fn a() {}".into(),
    }
}

async fn seed_files_run(s: &mut TypeDbStore, repo: &str, snap: &str) -> (String, String, String) {
    let files = vec![SnapshotFile {
        snapshot: snap.into(),
        path: "a.rs".into(),
        sha256: "abc".into(),
        bytes: 10,
    }];
    s.ensure_snapshot_files(snap, repo, &files).await.unwrap();
    let run = format!("run-{snap}");
    let set = format!("set-{snap}");
    s.ensure_run(&run, repo, snap, &set, "digest-1", "rubric-v1")
        .await
        .unwrap();
    (run, set, snap.into())
}

#[test]
fn only_a_missing_server_may_skip() {
    // Self-contained: bind an ephemeral port to observe an address that is
    // definitely listening, then release it to observe one that is not.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let open = listener.local_addr().expect("local addr");
    assert!(
        server_reachable(&open.to_string()),
        "a listening address must be treated as a present server"
    );
    drop(listener);
    assert!(
        !server_reachable(&open.to_string()),
        "a released address must read as no server, which is the only skippable case"
    );
    assert!(
        !server_reachable("definitely-not-a-host.invalid:1729"),
        "an unresolvable address must not read as a present server"
    );
}

#[test]
fn supplied_credentials_are_distinguished_from_no_credentials() {
    let existing_path = std::env::temp_dir().join("chaosbox-live-probe");
    std::fs::write(&existing_path, "s3cret").expect("write probe file");
    let empty_path = std::env::temp_dir().join("chaosbox-live-probe-empty");
    std::fs::write(&empty_path, "").expect("write empty probe file");
    let existing = std::ffi::OsString::from(existing_path.clone());
    let empty = std::ffi::OsString::from(empty_path.clone());
    let missing = std::ffi::OsString::from("/definitely/not/a/chaosbox/password");
    let inline = std::ffi::OsString::from("s3cret");
    let blank = std::ffi::OsString::new();

    assert!(
        matches!(credentials_from(None, None), Credentials::Unconfigured),
        "nothing supplied must read as an environment gap, not a broken credential"
    );
    assert!(
        matches!(
            credentials_from(Some(&existing), None),
            Credentials::Ready(_)
        ),
        "a readable password file means credentials were supplied"
    );
    assert!(
        matches!(credentials_from(None, Some(&inline)), Credentials::Ready(_)),
        "an inline password means credentials were supplied"
    );
    // A configured source that yields no usable secret is supplied-but-
    // broken, not absent: it must fail loudly instead of reading as
    // "never supplied" (which would turn a typo'd secret path into a
    // passing skip) or falling through to a default password.
    assert!(
        matches!(
            credentials_from(Some(&missing), None),
            Credentials::Broken(_)
        ),
        "a password file that does not exist was configured but unusable, not absent"
    );
    assert!(
        matches!(credentials_from(Some(&empty), None), Credentials::Broken(_)),
        "an empty password file is a broken credential, not an absent one"
    );
    assert!(
        matches!(credentials_from(None, Some(&blank)), Credentials::Broken(_)),
        "an empty inline password is broken configuration, not an absent password"
    );
    let _ = std::fs::remove_file(&existing_path);
    let _ = std::fs::remove_file(&empty_path);
}

#[test]
fn required_gate_is_explicit_and_fails_closed_on_empty() {
    assert!(required_from(Some("1")));
    assert!(required_from(Some(" true ")));
    assert!(required_from(Some("YES")));
    assert!(
        !required_from(Some("")),
        "an empty gate value must not enable"
    );
    assert!(!required_from(Some("0")));
    assert!(!required_from(Some("no")));
    assert!(!required_from(None));
}

#[tokio::test]
async fn migrate_is_idempotent() {
    let Some(mut s) = connected_store("t_migrate").await else {
        return;
    };
    s.migrate().await.unwrap();
    s.migrate().await.unwrap();
}

#[tokio::test]
async fn publish_readback_and_predecessor_guards() {
    let db = test_db("t_pub");
    let Some(mut s) = connected_store(&db).await else {
        return;
    };
    let repo = "pubrepo";
    let (_run, set, _snap) = seed_files_run(&mut s, repo, "s1").await;

    let a = ent(repo, "s1", "a.rs", "a");
    let b = ent(repo, "s1", "a.rs", "b");
    s.put_candidate(&set, &candidate("cand:1", &a.id, &b.id))
        .await
        .unwrap();
    s.put_decision(decision("cand:1", "q1", "key-1"))
        .await
        .unwrap();
    s.put_evidence(Evidence {
        id: "ev:1".into(),
        class: EvidenceClass::Extracted,
        supports: true,
        text: "fn a() {}".into(),
        span: Some(span("a.rs")),
        snapshot: "s1".into(),
        source_file_version: "a.rs".into(),
    })
    .await
    .unwrap();
    s.put_claim(Claim {
        id: "claim:1".into(),
        relation_id: "rel:pending".into(),
        supporting: vec!["ev:1".into()],
        contradicting: vec![],
        accepted: true,
    })
    .await
    .unwrap();

    let mut build = GraphBuild::new(repo, vec!["s1".into()], 1);
    build.add_node(a.clone()).unwrap();
    build.add_node(b.clone()).unwrap();
    let bid = build.id.clone();
    build
        .add_edge(Relation::new(
            RelationType::Calls,
            &a.id,
            &b.id,
            RelationScope::CrossFile,
            &bid,
        ))
        .unwrap();
    s.publish(build.clone(), None).await.unwrap();

    // Live readback through a FRESH store (nothing staged): decisions,
    // evidence linkage and the pointer swing really landed.
    let mut fresh = TypeDbStore::new(config(&db));
    fresh.migrate().await.unwrap();
    let found = fresh.find_decision("cand:1", "q1").await.unwrap().unwrap();
    assert_eq!(found.cache_key, "key-1");
    assert_eq!(found.id, "dec:cand:1:q1");

    // Second generation with the predecessor wins.
    let mut build2 = GraphBuild::new(repo, vec!["s1".into()], 2);
    build2.predecessor = Some(bid.clone());
    build2.add_node(a.clone()).unwrap();
    build2.add_node(b.clone()).unwrap();
    s.publish(build2.clone(), Some(bid.clone())).await.unwrap();

    // Stale predecessor and older generation both fail; last good stands.
    let mut stale = GraphBuild::new(repo, vec!["s1".into()], 3);
    stale.add_node(a.clone()).unwrap();
    let err = s
        .publish(stale, Some("build:stale".into()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("predecessor mismatch"), "{err}");
    let mut older = GraphBuild::new(repo, vec!["s1".into()], 1);
    older.predecessor = Some(build2.id.clone());
    older.add_node(a.clone()).unwrap();
    let err = s.publish(older, Some(build2.id.clone())).await.unwrap_err();
    assert!(err.to_string().contains("older worker"), "{err}");
}

#[tokio::test]
async fn concurrent_publishers_from_same_generation_exactly_one_wins() {
    let db = test_db("t_race");
    let Some(mut s1) = connected_store(&db).await else {
        return;
    };
    let repo = "racerepo";
    seed_files_run(&mut s1, repo, "s1").await;
    seed_files_run(&mut s1, repo, "s2").await;
    let a = ent(repo, "s1", "a.rs", "a");
    let mut gen1 = GraphBuild::new(repo, vec!["s1".into()], 1);
    gen1.add_node(a.clone()).unwrap();
    s1.publish(gen1.clone(), None).await.unwrap();

    // Same build published twice is idempotent, not a conflict.
    s1.publish(gen1.clone(), Some(gen1.id.clone()))
        .await
        .unwrap();
    let mut retry = TypeDbStore::new(config(&db));
    retry.migrate().await.unwrap();
    retry.publish(gen1.clone(), None).await.unwrap();

    // Genuine race: two distinct generation-2 builds from the same
    // predecessor, published concurrently. Exactly one wins; the loser
    // reports a conflict and the pointer holds the winner.
    let mut b1 = GraphBuild::new(repo, vec!["s1".into()], 2);
    b1.predecessor = Some(gen1.id.clone());
    b1.add_node(a.clone()).unwrap();
    let mut b2 = GraphBuild::new(repo, vec!["s2".into()], 2);
    b2.predecessor = Some(gen1.id.clone());
    b2.add_node(a.clone()).unwrap();
    assert_ne!(b1.id, b2.id, "rival builds must differ");
    let mut w1 = TypeDbStore::new(config(&db));
    w1.migrate().await.unwrap();
    let mut w2 = TypeDbStore::new(config(&db));
    w2.migrate().await.unwrap();
    let (r1, r2) = tokio::join!(
        w1.publish(b1.clone(), Some(gen1.id.clone())),
        w2.publish(b2.clone(), Some(gen1.id.clone()))
    );
    assert!(
        r1.is_ok() ^ r2.is_ok(),
        "exactly one publisher wins: {r1:?} vs {r2:?}"
    );

    // The loser retrying with its stale predecessor fails without moving
    // the pointer; last good build stays active.
    let mut late = TypeDbStore::new(config(&db));
    late.migrate().await.unwrap();
    let stale = if r1.is_ok() { b2 } else { b1 };
    let err = late
        .publish(stale, Some(gen1.id.clone()))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("predecessor mismatch")
            || err.to_string().contains("concurrent publisher"),
        "{err}"
    );
}

// Long end-to-end fixture test; splitting it apart is the owning
// session's refactor. Allowed to keep CI unblocked.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn reader_passes_reference_conformance_against_live_backend() {
    let db = test_db("t_conf");
    let Some(mut s) = connected_store(&db).await else {
        return;
    };
    // Seed the reference fixture through the write path: two builds of repo
    // `conf`, the second active, sharing a symbol name across snapshots.
    for snap in ["s1", "s2"] {
        s.ensure_snapshot_files(
            snap,
            "conf",
            &[SnapshotFile {
                snapshot: snap.into(),
                path: "f.rs".into(),
                sha256: "ff".into(),
                bytes: 8,
            }],
        )
        .await
        .unwrap();
    }
    let ent = |snap: &str, file: &str, name: &str| {
        Entity::new(
            EntityKind::Symbol,
            "conf",
            snap,
            file,
            name,
            name,
            span(file),
        )
    };
    let mut b1 = GraphBuild::new("conf", vec!["s1".into()], 1);
    let a1 = ent("s1", "f.rs", "Alpha");
    let b1e = ent("s1", "f.rs", "Beta");
    b1.add_node(a1.clone()).unwrap();
    b1.add_node(b1e.clone()).unwrap();
    let r1 = Relation::new(
        RelationType::Calls,
        &a1.id,
        &b1e.id,
        RelationScope::File,
        &b1.id,
    );
    b1.add_edge(r1.clone()).unwrap();
    let mut b2 = GraphBuild::new("conf", vec!["s2".into()], 2);
    b2.predecessor = Some(b1.id.clone());
    let a2 = ent("s2", "f.rs", "Alpha");
    let c2 = ent("s2", "g.rs", "Gamma");
    b2.add_node(a2.clone()).unwrap();
    b2.add_node(c2.clone()).unwrap();
    b2.add_edge(Relation::new(
        RelationType::References,
        &a2.id,
        &c2.id,
        RelationScope::CrossFile,
        &b2.id,
    ))
    .unwrap();
    s.put_evidence(Evidence {
        id: "ev1".into(),
        class: EvidenceClass::Extracted,
        supports: true,
        text: "[structural] Alpha -> Beta".into(),
        span: None,
        snapshot: "s1".into(),
        source_file_version: "f.rs".into(),
    })
    .await
    .unwrap();
    s.put_claim(Claim {
        id: "claim:conf1".into(),
        relation_id: r1.id.clone(),
        supporting: vec!["ev1".into()],
        contradicting: vec![],
        accepted: true,
    })
    .await
    .unwrap();
    s.publish(b1.clone(), None).await.unwrap();
    s.publish(b2.clone(), Some(b1.id.clone())).await.unwrap();

    // Snapshot fingerprint isolation: a second repo in the same database
    // must never leak into another repo's pinned snapshot list (the typedb
    // `active_build` read once ranged over every build in the database).
    s.ensure_snapshot_files(
        "s9",
        "other",
        &[SnapshotFile {
            snapshot: "s9".into(),
            path: "z.rs".into(),
            sha256: "zz".into(),
            bytes: 3,
        }],
    )
    .await
    .unwrap();
    let zoe = Entity::new(
        EntityKind::Symbol,
        "other",
        "s9",
        "z.rs",
        "Zeta",
        "Zeta",
        span("z.rs"),
    );
    let mut ob = GraphBuild::new("other", vec!["s9".into()], 1);
    ob.add_node(zoe).unwrap();
    s.publish(ob, None).await.unwrap();

    let mut reader = TypeDbReader::new(config(&db));
    reader.connect().await.unwrap();
    let conf_row = reader.active_build("conf").await.unwrap().unwrap();
    assert_eq!(
        conf_row.snapshots,
        ["s2".to_owned()],
        "conf pins only its own active build's snapshots"
    );
    let other_row = reader.active_build("other").await.unwrap().unwrap();
    assert_eq!(
        other_row.snapshots,
        ["s9".to_owned()],
        "other pins only its own active build's snapshots"
    );
    check_conformance(&reader, &a1.id, &b1e.id, &r1.id, &a2.id, &(b1.id, b2.id)).await;
}
