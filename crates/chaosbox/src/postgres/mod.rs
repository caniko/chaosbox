//! Source-backed PostgreSQL catalog capture and zero-model publication.
//! Fixed catalog SQL runs in one repeatable-read, read-only transaction.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};
use chaosbox_core::{
    Claim, Entity, EntityKind, Evidence, EvidenceClass, GraphBuild, Relation, RelationScope,
    RelationType, SourceSpan, deterministic_id, sha256_hex,
};
use chaosbox_extract::{FileVersion, Snapshot};
use chaosbox_store::Store;
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PRODUCER: &str = "postgres-catalog-v1";
const SQL: &str = include_str!("catalog.sql");
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const FILE: &str = "postgres/catalog.jsonl";

/// One verbatim catalog record. `details` retains typed native catalog fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// Scoped native catalog identity, including object class.
    pub key: String,
    /// Object class within the documented coverage subset.
    pub kind: String,
    /// Selected namespace.
    pub schema: String,
    /// Native display name.
    pub name: String,
    /// Parent relation, for columns, indexes and constraints.
    pub parent: Option<String>,
    /// Referenced relation for a foreign key; may be outside the scope.
    pub target: Option<String>,
    /// PostgreSQL-deparsed definition, absent where the catalog cannot provide it.
    pub definition: Option<String>,
    /// Native fields such as column types, FK attnums and privilege visibility.
    pub details: serde_json::Value,
}

/// Catalog response from the fixed SQL. Database rows are never collected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// Connected database, verified against the requested database.
    pub database: String,
    /// Explicit single-schema scope.
    pub schema: String,
    /// Actual PostgreSQL execution identity.
    pub role: String,
    /// Native `server_version_num`.
    pub server_version: String,
    /// Must be `on` for every accepted capture.
    pub read_only: String,
    /// Missing schemas fail instead of publishing an empty graph.
    pub schema_exists: bool,
    /// Explicit schema visibility gate.
    pub schema_usage: bool,
    /// Bounded captured object records.
    pub records: Vec<Record>,
    /// Source-reported omissions and documented extraction limitations.
    pub omissions: serde_json::Value,
}

/// Private immutable receipt binding catalog, collection time and producer SQL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    /// Versioned capture contract.
    pub version: u32,
    /// Exact packaged query digest, rejected on import if it changes.
    pub query_sha256: String,
    /// Unix collection time; this is historical capture, not continuous freshness.
    pub observed_at: u64,
    /// All selected catalog records and explicit omissions.
    pub catalog: Catalog,
    /// Content fingerprint of the receipt inputs.
    pub digest: String,
}

impl Capture {
    /// Validate the bounded catalog and bind its canonical ordered records.
    pub fn new(mut catalog: Catalog, observed_at: u64) -> Result<Self> {
        catalog.records.sort_by(|a, b| a.key.cmp(&b.key));
        let mut capture = Self {
            version: 1,
            query_sha256: sha256_hex(&[SQL]),
            observed_at,
            catalog,
            digest: String::new(),
        };
        capture.digest = capture.fingerprint()?;
        capture.validate()?;
        Ok(capture)
    }

    fn fingerprint(&self) -> Result<String> {
        let mut value = serde_json::to_value(self)?;
        value["digest"] = serde_json::json!("");
        Ok(sha256_hex(&[&serde_json::to_string(&value)?]))
    }

    /// Reject changed receipt bytes, unsupported scope or non-read-only capture.
    pub fn validate(&self) -> Result<()> {
        let c = &self.catalog;
        if self.version != 1
            || self.query_sha256 != sha256_hex(&[SQL])
            || self.digest != self.fingerprint()?
            || c.database.is_empty()
            || c.schema.is_empty()
            || c.role.is_empty()
            || !c.schema_exists
            || !c.schema_usage
            || c.read_only != "on"
            || self.observed_at == 0
            || c.records.len() > 9_998
            || serde_json::to_vec(self)?.len() as u64 > MAX_BYTES
        {
            return Err("invalid PostgreSQL catalog receipt, scope or bounds".into());
        }
        let mut previous: Option<&str> = None;
        let keys: BTreeMap<_, _> = c.records.iter().map(|r| (r.key.as_str(), r)).collect();
        for r in &c.records {
            if previous.is_some_and(|p| p >= r.key.as_str())
                || r.schema != c.schema
                || r.key.is_empty()
                || r.name.is_empty()
                || !matches!(
                    r.kind.as_str(),
                    "table"
                        | "view"
                        | "materialized_view"
                        | "foreign_table"
                        | "column"
                        | "constraint"
                        | "index"
                        | "routine"
                )
                || !r.details.is_object()
                || r.parent
                    .as_ref()
                    .is_some_and(|p| !keys.contains_key(p.as_str()))
            {
                return Err("invalid, duplicate or unscoped PostgreSQL object".into());
            }
            previous = Some(&r.key);
        }
        Ok(())
    }

    /// Snapshot identity includes collection receipt and its explicit scope.
    #[must_use]
    pub fn snapshot(&self) -> String {
        deterministic_id("snap", &[PRODUCER, &self.digest])
    }

    fn source(&self, repo: &str) -> Result<(Snapshot, Vec<SourceSpan>, Vec<String>)> {
        self.validate()?;
        let mut texts = vec![serde_json::to_string(
            &serde_json::json!({"version":self.version,"query_sha256":self.query_sha256,
            "observed_at":self.observed_at,"database":self.catalog.database,"schema":self.catalog.schema,"role":self.catalog.role,
            "server_version":self.catalog.server_version,"read_only":self.catalog.read_only,"omissions":self.catalog.omissions}),
        )?];
        texts.extend(
            self.catalog
                .records
                .iter()
                .map(serde_json::to_string)
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
        let mut source = String::new();
        let mut spans = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let start = u32::try_from(source.len())?;
            source.push_str(text);
            let line = u32::try_from(i + 1)?;
            spans.push(SourceSpan {
                file: FILE.into(),
                start_line: line,
                end_line: line,
                start_col: 1,
                end_col: u32::try_from(text.chars().count() + 1)?,
                byte_start: start,
                byte_end: u32::try_from(source.len())?,
            });
            source.push('\n');
        }
        Ok((
            Snapshot {
                id: self.snapshot(),
                repo: repo.into(),
                scope: vec![self.catalog.schema.clone()],
                files: vec![FileVersion {
                    path: FILE.into(),
                    sha256: sha256_hex(&[&source]),
                    bytes: source.len() as u64,
                }],
                contents: BTreeMap::from([(FILE.into(), source)]),
            },
            spans,
            texts,
        ))
    }
}

/// Capture through the installed PostgreSQL client. Credentials remain in
/// libpq's service/peer/passfile configuration, never a DSN or shell string.
pub fn collect(database: &str, schema: &str) -> Result<Capture> {
    if database.is_empty()
        || database.starts_with('-')
        || database.contains(['=', ':', '\n', '\0'])
        || schema.is_empty()
        || schema.len() > 256
        || schema.contains(['\n', '\0'])
    {
        return Err("database must be a name, and schema must be one explicit namespace".into());
    }
    let mut child = Command::new("psql")
        .args([
            "--no-psqlrc",
            "--no-password",
            "--quiet",
            "--tuples-only",
            "--no-align",
            "--set",
            "ON_ERROR_STOP=1",
            "--set",
            &format!("schema={schema}"),
            "--dbname",
            database,
        ])
        .env("PGCONNECT_TIMEOUT", "10")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("no PostgreSQL input")?
        .write_all(SQL.as_bytes())?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or("no PostgreSQL output")?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        return Err("PostgreSQL capture exceeded output bound or failed".into());
    }
    if !child.wait()?.success() {
        return Err("PostgreSQL catalog query failed".into());
    }
    let catalog: Catalog = serde_json::from_slice(&bytes)?;
    if catalog.database != database || catalog.schema != schema {
        return Err("PostgreSQL source identity mismatch".into());
    }
    Capture::new(
        catalog,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
    )
}

/// Persist one private create-only capture. Existing evidence is never replaced.
pub fn write_capture(path: &Path, capture: &Capture) -> Result<()> {
    capture.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut output, capture)?;
    output.as_file().sync_all()?;
    output.persist_noclobber(path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Load a bounded receipt; imports are checked against the packaged query.
pub fn load_capture(path: &Path) -> Result<Capture> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("PostgreSQL receipt exceeds bound".into());
    }
    let capture: Capture = serde_json::from_slice(&bytes)?;
    capture.validate()?;
    Ok(capture)
}

/// Publish certified catalog observations with quoted JSONL evidence and the
/// shared guarded active-build transaction. No inference or row data is used.
pub async fn publish<S: Store>(
    store: &mut S,
    capture: &Capture,
    repo: &str,
    generation: u64,
    predecessor: Option<String>,
) -> Result<GraphBuild> {
    let (snapshot, spans, texts) = capture.source(repo)?;
    if repo.is_empty() {
        return Err("missing PostgreSQL graph identity".into());
    }
    let mut build = GraphBuild::new(repo, vec![snapshot.id.clone()], generation);
    build.predecessor = predecessor.clone();
    let root = Entity::new(
        EntityKind::File,
        repo,
        &snapshot.id,
        FILE,
        "PostgreSQL catalog",
        FILE,
        spans[0].clone(),
    );
    build.add_node(root.clone())?;
    let metadata = Entity::new(
        EntityKind::Module,
        repo,
        &snapshot.id,
        FILE,
        &capture.catalog.schema,
        &format!(
            "{}:{}:catalog-receipt",
            capture.catalog.database, capture.catalog.schema
        ),
        spans[0].clone(),
    );
    build.add_node(metadata.clone())?;
    let mut entities = BTreeMap::new();
    for (i, record) in capture.catalog.records.iter().enumerate() {
        let entity = catalog_entity(record, capture, &snapshot, spans[i + 1].clone());
        build.add_node(entity.clone())?;
        entities.insert(record.key.as_str(), entity);
    }
    store
        .ensure_snapshot_files(&snapshot.id, repo, &snapshot.snapshot_files())
        .await?;
    let mut observations = vec![(
        root.id.as_str(),
        metadata.id.as_str(),
        RelationType::Contains,
        0,
    )];
    for (i, record) in capture.catalog.records.iter().enumerate() {
        let entity = &entities[record.key.as_str()];
        observations.push((
            root.id.as_str(),
            entity.id.as_str(),
            RelationType::Defines,
            i + 1,
        ));
        if let Some(parent) = &record.parent {
            observations.push((
                entities[parent.as_str()].id.as_str(),
                entity.id.as_str(),
                RelationType::Contains,
                i + 1,
            ));
        }
        if let Some(target) = record
            .target
            .as_ref()
            .and_then(|key| entities.get(key.as_str()))
        {
            observations.push((
                entity.id.as_str(),
                target.id.as_str(),
                RelationType::References,
                i + 1,
            ));
        }
    }
    for (from, to, kind, line) in observations {
        let (relation, evidence, claim) = observation(
            Relation::new(kind, from, to, RelationScope::File, &build.id),
            &snapshot,
            &spans[line],
            &texts[line],
        );
        build.add_edge(relation)?;
        store.put_evidence(evidence).await?;
        store.put_claim(claim).await?;
    }
    build.coverage = Some(chaosbox_core::coverage::BuildCoverage {
        catalog: Some(catalog_coverage(capture, build.edges.len())),
        ..chaosbox_core::coverage::BuildCoverage::default()
    });
    store.publish(build.clone(), predecessor).await?;
    Ok(build)
}

fn catalog_entity(
    record: &Record,
    capture: &Capture,
    snapshot: &Snapshot,
    span: SourceSpan,
) -> Entity {
    Entity::new(
        EntityKind::Definition,
        &snapshot.repo,
        &snapshot.id,
        FILE,
        &record.name,
        &format!(
            "{}:{}:{}:{}",
            capture.catalog.database, record.schema, record.kind, record.key
        ),
        span,
    )
}

fn catalog_coverage(
    capture: &Capture,
    relations: usize,
) -> chaosbox_core::coverage::CatalogCoverage {
    let mut objects = BTreeMap::new();
    let keys: std::collections::BTreeSet<_> = capture
        .catalog
        .records
        .iter()
        .map(|r| r.key.as_str())
        .collect();
    for record in &capture.catalog.records {
        *objects.entry(record.kind.clone()).or_default() += 1;
    }
    chaosbox_core::coverage::CatalogCoverage {
        relations,
        producer: PRODUCER.into(),
        receipt: capture.digest.clone(),
        database: capture.catalog.database.clone(),
        schema: capture.catalog.schema.clone(),
        role: capture.catalog.role.clone(),
        observed_at: capture.observed_at,
        objects,
        unresolved_foreign_keys: capture
            .catalog
            .records
            .iter()
            .filter(|r| {
                r.target
                    .as_ref()
                    .is_some_and(|key| !keys.contains(key.as_str()))
            })
            .count(),
        omissions: capture.catalog.omissions.clone(),
    }
}

fn observation(
    mut relation: Relation,
    snapshot: &Snapshot,
    span: &SourceSpan,
    text: &str,
) -> (Relation, Evidence, Claim) {
    let evidence = Evidence {
        id: deterministic_id("ev", &[&snapshot.id, &relation.id, PRODUCER]),
        class: EvidenceClass::Extracted,
        supports: true,
        text: text.into(),
        span: Some(span.clone()),
        snapshot: snapshot.id.clone(),
        source_file_version: FILE.into(),
        producer: Some(PRODUCER.into()),
    };
    relation.evidence_ids.push(evidence.id.clone());
    let claim = Claim {
        id: deterministic_id("claim", &[&relation.id]),
        relation_id: relation.id.clone(),
        supporting: vec![evidence.id.clone()],
        contradicting: Vec::new(),
        accepted: true,
    };
    (relation, evidence, claim)
}
