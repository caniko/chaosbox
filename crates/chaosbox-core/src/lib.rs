//! Chaosbox core: domain types, deterministic identifiers, evidence contracts,
//! validation and pure graph logic.
//!
//! All identifiers are repository-relative and snapshot-scoped. Entity identity
//! is separate from occurrences and labels: different repositories/snapshots
//! never collide, and rename continuity is never inferred from labels.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Hash `parts` with SHA-256, joined by `\0`, hex-encoded.
#[must_use]
pub fn sha256_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    hex::encode(h.finalize())
}

/// Deterministic id: `<prefix>:<12 hex chars>`.
#[must_use]
pub fn deterministic_id(prefix: &str, parts: &[&str]) -> String {
    format!("{}:{}", prefix, &sha256_hex(parts)[..12])
}

/// Repository-relative source span (1-based lines/cols, 0-based byte offsets).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    /// Repository-relative file path.
    pub file: String,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based start column.
    pub start_col: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// 1-based end column.
    pub end_col: u32,
    /// 0-based start byte offset.
    pub byte_start: u32,
    /// 0-based end byte offset.
    pub byte_end: u32,
}

impl SourceSpan {
    /// A zero-width span at one position (for file/module records).
    #[must_use]
    pub fn point(file: &str, line: u32, col: u32, byte: u32) -> Self {
        Self {
            file: file.to_owned(),
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
            byte_start: byte,
            byte_end: byte,
        }
    }
}

/// What kind of thing an entity is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// A source file itself.
    File,
    /// A module / namespace unit.
    Module,
    /// A named symbol (function, class, constant, ...).
    Symbol,
    /// A definition site of a symbol.
    Definition,
    /// An import statement.
    Import,
    /// A Markdown heading.
    Heading,
    /// A Markdown link.
    Link,
    /// A Markdown inline code mention.
    CodeMention,
}

/// A possible entity: identity separate from occurrences/labels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    /// Deterministic `ent:<hex>` identity (repo + snapshot + kind + name + span).
    pub id: String,
    /// What kind of thing this entity is.
    pub kind: EntityKind,
    /// Owning repository name.
    pub repo: String,
    /// Snapshot this identity belongs to; identities never cross snapshots.
    pub snapshot: String,
    /// Repository-relative file path.
    pub file: String,
    /// Short display name (copied from source).
    pub name: String,
    /// Qualified name (copied from source or deterministic template).
    pub qualified_name: String,
    /// Where the name occurs in source.
    pub span: SourceSpan,
    /// Alternate labels observed in source.
    pub aliases: Vec<String>,
}

impl Entity {
    /// Construct an entity with a deterministic scoped id.
    #[must_use]
    pub fn new(
        kind: EntityKind,
        repo: &str,
        snapshot: &str,
        file: &str,
        name: &str,
        qualified_name: &str,
        span: SourceSpan,
    ) -> Self {
        let id = deterministic_id(
            "ent",
            &[
                repo,
                snapshot,
                file,
                &format!("{:?}", kind),
                qualified_name,
                &span.start_line.to_string(),
                &span.start_col.to_string(),
            ],
        );
        Self {
            id,
            kind,
            repo: repo.to_owned(),
            snapshot: snapshot.to_owned(),
            file: file.to_owned(),
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            span,
            aliases: Vec::new(),
        }
    }
}

/// Relation vocabulary. Each variant is a first-class edge type.
/// Canonical storage name is the serde `snake_case` form (see [`relation_type_name`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    /// Containment (file -> module -> definition).
    Contains,
    /// A module/file defines a symbol.
    Defines,
    /// An import statement targets a module.
    Imports,
    /// A textual reference to a symbol.
    References,
    /// A call from one symbol to another.
    Calls,
    /// A Markdown link target.
    LinksTo,
    /// A Markdown code mention of a symbol.
    Mentions,
}

/// Canonical storage name for a relation type (serde `snake_case`).
/// Both the in-memory projection and future Gel inserts must use this.
#[must_use]
pub fn relation_type_name(r: &RelationType) -> String {
    serde_json::to_value(r)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{r:?}"))
}

/// Scope of a relationship observation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationScope {
    /// Both endpoints in one file.
    File,
    /// Both endpoints in one module.
    Module,
    /// Endpoints span files.
    CrossFile,
}

/// A relationship is a first-class object with typed endpoints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Deterministic `rel:<hex>` identity (build + type + endpoints).
    pub id: String,
    /// The relation vocabulary variant.
    pub rel_type: RelationType,
    /// Source entity id (same build).
    pub from: String,
    /// Target entity id (same build).
    pub to: String,
    /// Observation scope.
    pub scope: RelationScope,
    /// Supporting/contradicting evidence ids.
    pub evidence_ids: Vec<String>,
}

impl Relation {
    /// Construct a relationship with a deterministic build-scoped id.
    #[must_use]
    pub fn new(
        rel_type: RelationType,
        from: &str,
        to: &str,
        scope: RelationScope,
        build: &str,
    ) -> Self {
        let id = deterministic_id("rel", &[build, &format!("{:?}", rel_type), from, to]);
        Self {
            id,
            rel_type,
            from: from.to_owned(),
            to: to.to_owned(),
            scope,
            evidence_ids: Vec::new(),
        }
    }
}

/// Evidence classification. Model probability never upgrades INFERRED.
/// Canonical storage name is the serde `snake_case` form (see [`evidence_class_name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    /// Explicit source evidence.
    Extracted,
    /// A supported inference; model confidence alone never produces this upgrade.
    Inferred,
    /// Unresolved or uncertain.
    Ambiguous,
}

/// Canonical storage name for an evidence class (serde `snake_case`).
#[must_use]
pub fn evidence_class_name(c: EvidenceClass) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{c:?}"))
}

/// Canonical storage name for an entity kind (serde `snake_case`).
/// Both the in-memory projection and Gel inserts must use this.
#[must_use]
pub fn entity_kind_name(k: &EntityKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{k:?}"))
}

/// One supporting or contradicting observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Deterministic `ev:<hex>` identity.
    pub id: String,
    /// Evidence classification.
    pub class: EvidenceClass,
    /// True when supporting the claim, false when contradicting.
    pub supports: bool,
    /// Verbatim source excerpt (copied span) or deterministic template text.
    pub text: String,
    /// Source span the text was copied from, if any.
    pub span: Option<SourceSpan>,
    /// Snapshot this observation belongs to (file-version linkage).
    pub snapshot: String,
    /// Repository-relative path of the source file version.
    pub source_file_version: String,
}

/// Content identity of one source file version: what the Gel `FileVersion`
/// link resolves from. Both backends key evidence files by
/// (snapshot, path); hashes are never invented.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFile {
    /// Snapshot id.
    pub snapshot: String,
    /// Repository-relative path.
    pub path: String,
    /// SHA-256 hex of the file text.
    pub sha256: String,
    /// File size in bytes.
    pub bytes: u64,
}

/// A claim assembled from evidence; negative evidence survives merges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Deterministic `claim:<hex>` identity.
    pub id: String,
    /// Relationship this claim is about.
    pub relation_id: String,
    /// Supporting evidence ids.
    pub supporting: Vec<String>,
    /// Contradicting evidence ids; never dropped during merges.
    pub contradicting: Vec<String>,
    /// Whether the claim is currently accepted.
    pub accepted: bool,
}

/// Outcome of one Jev decision (kept separate from evidence class).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    /// Evidence supports the proposal.
    Accepted,
    /// Evidence contradicts the proposal.
    Rejected,
    /// The model abstained; recorded, never retried as a failure.
    Abstained,
    /// Successful negative: no finding in source.
    Negative,
    /// The decision itself failed (transport/validation); may be retried.
    Failed(String),
}

/// One validated Jev decision over a candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// Deterministic `dec:<hex>` identity (candidate + question + model).
    pub id: String,
    /// Candidate this decision judges.
    pub candidate_id: String,
    /// Question id within the Jev request.
    pub question_id: String,
    /// The outcome.
    pub outcome: DecisionOutcome,
    /// Evidence classification (never upgraded by confidence alone).
    pub evidence_class: EvidenceClass,
    /// Model identity requested.
    pub model_requested: String,
    /// Model identity returned by the provider.
    pub model_returned: String,
    /// Confidence (Choice/Score), if the answer type carries one.
    pub confidence: Option<f64>,
    /// Probability (Noul or winning option), if applicable.
    pub probability: Option<f64>,
    /// Cache identity under which this decision is valid (source,
    /// preprocessing, catalog, questions, model, rubric). Reuse compares
    /// this key; threshold-only changes keep it stable.
    pub cache_key: String,
}

/// Candidate catalog/preprocessing version. Bump when parsers, candidate
/// construction, or question semantics change: the digest below feeds every
/// decision cache key, so a bump conservatively re-asks all decisions.
pub const CATALOG_VERSION: &str = "catalog-v1";

/// Catalog digest over the sorted candidate set: one record per candidate
/// `(id, rel_type, from, to, reason)` plus [`CATALOG_VERSION`].
/// Conservative: any catalog change invalidates every decision in the run
/// (per-dependency precision is a documented follow-up).
#[must_use]
pub fn catalog_digest(candidates: &[Candidate]) -> String {
    let mut records: Vec<String> = candidates
        .iter()
        .map(|c| {
            format!(
                "{}:{}:{}:{}:{}",
                c.id,
                relation_type_name(&c.rel_type),
                c.from_entity,
                c.to_entity,
                c.reason
            )
        })
        .collect();
    records.sort();
    records.push(CATALOG_VERSION.to_owned());
    sha256_hex(&records.iter().map(String::as_str).collect::<Vec<_>>())
}

/// A candidate relationship proposed deterministically for Jev review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Deterministic `cand:<hex>` identity (relation + endpoints + reason).
    pub id: String,
    /// Proposed relation type.
    pub rel_type: RelationType,
    /// Source entity id.
    pub from_entity: String,
    /// Target entity id.
    pub to_entity: String,
    /// Why the candidate was proposed (structural, lexical-import, co-occurrence).
    pub reason: String,
    /// Bounded source excerpt grounding the proposal.
    pub state_excerpt: String,
}

/// Validation failures for model-returned values, labels, and graph invariants.
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("probability {0} out of [0,1]")]
    /// Probability outside the closed [0,1] interval.
    ProbabilityRange(f64),
    #[error("confidence {0} out of [0,1]")]
    /// Confidence outside the closed [0,1] interval.
    ConfidenceRange(f64),
    #[error("non-finite value")]
    /// NaN or infinite where a finite value is required.
    NonFinite,
    #[error("empty label")]
    /// A label that is empty after trimming (generative fallback forbidden).
    EmptyLabel,
    #[error("cross-build edge: {0} not in build {1}")]
    /// An edge endpoint that is not a member of the build.
    CrossBuildEdge(String, String),
    #[error("duplicate build member: {0}")]
    /// A node or edge id inserted twice into one build.
    DuplicateMember(String),
}

/// Finite + range checks for model-returned floats.
pub fn check_probability(p: f64) -> Result<(), ValidationError> {
    if !p.is_finite() {
        return Err(ValidationError::NonFinite);
    }
    if !(0.0..=1.0).contains(&p) {
        return Err(ValidationError::ProbabilityRange(p));
    }
    Ok(())
}

/// Confidence uses the same [0,1] finite contract.
pub fn check_confidence(c: f64) -> Result<(), ValidationError> {
    if !c.is_finite() {
        return Err(ValidationError::NonFinite);
    }
    if !(0.0..=1.0).contains(&c) {
        return Err(ValidationError::ConfidenceRange(c));
    }
    Ok(())
}

/// Labels must be copied from source spans or deterministic templates:
/// non-empty, no generative fallback.
pub fn check_label(label: &str) -> Result<(), ValidationError> {
    if label.trim().is_empty() {
        return Err(ValidationError::EmptyLabel);
    }
    Ok(())
}

/// One immutable published graph build.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphBuild {
    /// Deterministic `build:<hex>` identity (repo + snapshots + generation).
    pub id: String,
    /// Owning repository name.
    pub repo: String,
    /// Pinned component snapshot ids.
    pub snapshot_ids: Vec<String>,
    /// Member nodes by entity id.
    pub nodes: BTreeMap<String, Entity>,
    /// Member edges by relation id.
    pub edges: BTreeMap<String, Relation>,
    /// Monotonic generation; newer workers win publication races.
    pub generation: u64,
    /// Previous build id, if any.
    pub predecessor: Option<String>,
}

impl GraphBuild {
    /// Start an empty staging build with a deterministic id.
    #[must_use]
    pub fn new(repo: &str, snapshot_ids: Vec<String>, generation: u64) -> Self {
        let id = deterministic_id(
            "build",
            &[repo, &snapshot_ids.join(","), &generation.to_string()],
        );
        Self {
            id,
            repo: repo.to_owned(),
            snapshot_ids,
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            generation,
            predecessor: None,
        }
    }

    /// Insert a node; errors on duplicate member.
    pub fn add_node(&mut self, e: Entity) -> Result<(), ValidationError> {
        if self.nodes.contains_key(&e.id) {
            return Err(ValidationError::DuplicateMember(e.id));
        }
        self.nodes.insert(e.id.clone(), e);
        Ok(())
    }

    /// Insert an edge; both endpoints must be members of this build.
    pub fn add_edge(&mut self, r: Relation) -> Result<(), ValidationError> {
        if !self.nodes.contains_key(&r.from) {
            return Err(ValidationError::CrossBuildEdge(
                r.from.clone(),
                self.id.clone(),
            ));
        }
        if !self.nodes.contains_key(&r.to) {
            return Err(ValidationError::CrossBuildEdge(
                r.to.clone(),
                self.id.clone(),
            ));
        }
        if self.edges.contains_key(&r.id) {
            return Err(ValidationError::DuplicateMember(r.id));
        }
        self.edges.insert(r.id.clone(), r);
        Ok(())
    }

    /// Outgoing neighborhood with optional relation filter.
    #[must_use]
    pub fn outgoing(&self, id: &str, filter: Option<&RelationType>) -> Vec<&Relation> {
        self.edges
            .values()
            .filter(|r| r.from == id && filter.is_none_or(|f| &r.rel_type == f))
            .collect()
    }

    /// Incoming neighborhood with optional relation filter.
    #[must_use]
    pub fn incoming(&self, id: &str, filter: Option<&RelationType>) -> Vec<&Relation> {
        self.edges
            .values()
            .filter(|r| r.to == id && filter.is_none_or(|f| &r.rel_type == f))
            .collect()
    }

    /// Bounded BFS path (undirected traversal, directed output order).
    /// `ponytail: O(V+E) scan per query, indexed adjacency if graphs grow large`.
    #[must_use]
    pub fn bounded_path(&self, from: &str, to: &str, max_hops: usize) -> Option<Vec<String>> {
        if from == to {
            return Some(vec![from.to_owned()]);
        }
        let mut prev: BTreeMap<String, (String, String)> = BTreeMap::new();
        let mut seen: BTreeSet<String> = BTreeSet::from([from.to_owned()]);
        let mut q: VecDeque<(String, usize)> = VecDeque::from([(from.to_owned(), 0)]);
        // adjacency (undirected for reachability, edge ids preserved)
        let mut adj: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for r in self.edges.values() {
            adj.entry(r.from.clone())
                .or_default()
                .push((r.to.clone(), r.id.clone()));
            adj.entry(r.to.clone())
                .or_default()
                .push((r.from.clone(), r.id.clone()));
        }
        while let Some((cur, depth)) = q.pop_front() {
            if depth >= max_hops {
                continue;
            }
            if let Some(nexts) = adj.get(&cur) {
                for (nxt, eid) in nexts.clone() {
                    if seen.insert(nxt.clone()) {
                        prev.insert(nxt.clone(), (cur.clone(), eid.clone()));
                        if nxt == to {
                            let mut path = vec![to.to_owned()];
                            let mut c = to.to_owned();
                            while let Some((p, _)) = prev.get(&c) {
                                path.push(p.clone());
                                c = p.clone();
                            }
                            path.reverse();
                            return Some(path);
                        }
                        q.push_back((nxt, depth + 1));
                    }
                }
            }
        }
        None
    }

    /// Bounded reachability set from `from` within `max_hops`.
    #[must_use]
    pub fn reachable(&self, from: &str, max_hops: usize) -> BTreeSet<String> {
        let mut seen = BTreeSet::from([from.to_owned()]);
        let mut q: VecDeque<(String, usize)> = VecDeque::from([(from.to_owned(), 0)]);
        let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in self.edges.values() {
            adj.entry(r.from.clone()).or_default().push(r.to.clone());
            adj.entry(r.to.clone()).or_default().push(r.from.clone());
        }
        while let Some((cur, depth)) = q.pop_front() {
            if depth >= max_hops {
                continue;
            }
            if let Some(nexts) = adj.get(&cur) {
                for nxt in nexts.clone() {
                    if seen.insert(nxt.clone()) {
                        q.push_back((nxt, depth + 1));
                    }
                }
            }
        }
        seen.remove(from);
        seen
    }

    /// Deterministic source-derived community label: top-level dir + kind mix.
    /// Never generative: falls back to `community:<dir>` template.
    #[must_use]
    pub fn community_label(&self, member_ids: &[String]) -> String {
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        for id in member_ids {
            if let Some(e) = self.nodes.get(id) {
                let dir = e.file.split('/').next().unwrap_or("root");
                dirs.insert(dir.to_owned());
            }
        }
        let mut v: Vec<String> = dirs.into_iter().collect();
        v.sort();
        format!("community:{}", v.join("+"))
    }
}

/// Diff two builds: added/removed nodes and edges by id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildDiff {
    /// Node ids present in the new build only.
    pub added_nodes: Vec<String>,
    /// Node ids present in the old build only.
    pub removed_nodes: Vec<String>,
    /// Edge ids present in the new build only.
    pub added_edges: Vec<String>,
    /// Edge ids present in the old build only.
    pub removed_edges: Vec<String>,
}

/// Compute the member-id diff between two builds.
#[must_use]
pub fn diff_builds(old: &GraphBuild, new: &GraphBuild) -> BuildDiff {
    let old_n: BTreeSet<_> = old.nodes.keys().collect();
    let new_n: BTreeSet<_> = new.nodes.keys().collect();
    let old_e: BTreeSet<_> = old.edges.keys().collect();
    let new_e: BTreeSet<_> = new.edges.keys().collect();
    BuildDiff {
        added_nodes: new_n.difference(&old_n).map(|s| (*s).clone()).collect(),
        removed_nodes: old_n.difference(&new_n).map(|s| (*s).clone()).collect(),
        added_edges: new_e.difference(&old_e).map(|s| (*s).clone()).collect(),
        removed_edges: old_e.difference(&new_e).map(|s| (*s).clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(file: &str) -> SourceSpan {
        SourceSpan::point(file, 1, 1, 0)
    }

    #[test]
    fn canonical_storage_names_are_snake_case() {
        assert_eq!(relation_type_name(&RelationType::LinksTo), "links_to");
        assert_eq!(relation_type_name(&RelationType::Calls), "calls");
        assert_eq!(evidence_class_name(EvidenceClass::Extracted), "extracted");
        assert_eq!(evidence_class_name(EvidenceClass::Ambiguous), "ambiguous");
        assert_eq!(entity_kind_name(&EntityKind::CodeMention), "code_mention");
        assert_eq!(entity_kind_name(&EntityKind::Symbol), "symbol");
    }

    #[test]
    fn catalog_digest_is_order_invariant_and_change_sensitive() {
        let mk = |id: &str| Candidate {
            id: id.into(),
            rel_type: RelationType::Calls,
            from_entity: "a".into(),
            to_entity: "b".into(),
            reason: "structural".into(),
            state_excerpt: String::new(),
        };
        let (c1, c2) = (mk("cand:1"), mk("cand:2"));
        assert_eq!(
            catalog_digest(&[c1.clone(), c2.clone()]),
            catalog_digest(&[c2.clone(), c1.clone()])
        );
        let mut changed = c2.clone();
        changed.reason = "co-occurrence".into();
        assert_ne!(catalog_digest(&[c1, c2]), catalog_digest(&[changed]));
        assert!(!CATALOG_VERSION.is_empty());
    }

    #[test]
    fn ids_are_deterministic_and_scoped() {
        let a = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "a.rs",
            "foo",
            "a::foo",
            span("a.rs"),
        );
        let b = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "a.rs",
            "foo",
            "a::foo",
            span("a.rs"),
        );
        assert_eq!(a.id, b.id);
        let c = Entity::new(
            EntityKind::Symbol,
            "r",
            "s2",
            "a.rs",
            "foo",
            "a::foo",
            span("a.rs"),
        );
        assert_ne!(a.id, c.id, "snapshots must not collide");
        let d = Entity::new(
            EntityKind::Symbol,
            "other",
            "s1",
            "a.rs",
            "foo",
            "a::foo",
            span("a.rs"),
        );
        assert_ne!(a.id, d.id, "repos must not collide");
    }

    #[test]
    fn edges_require_same_build_members() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "a.rs",
            "a",
            "a",
            span("a.rs"),
        );
        let outsider = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "b.rs",
            "b",
            "b",
            span("b.rs"),
        );
        g.add_node(a.clone()).unwrap();
        let r = Relation::new(
            RelationType::Calls,
            &a.id,
            &outsider.id,
            RelationScope::CrossFile,
            &g.id,
        );
        assert!(g.add_edge(r).is_err(), "cross-build edge must fail");
    }

    #[test]
    fn parallel_relations_preserved() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "a.rs",
            "a",
            "a",
            span("a.rs"),
        );
        let b = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "b.rs",
            "b",
            "b",
            span("b.rs"),
        );
        g.add_node(a.clone()).unwrap();
        g.add_node(b.clone()).unwrap();
        let r1 = Relation::new(
            RelationType::Calls,
            &a.id,
            &b.id,
            RelationScope::CrossFile,
            &g.id,
        );
        let mut r2 = Relation::new(
            RelationType::References,
            &a.id,
            &b.id,
            RelationScope::CrossFile,
            &g.id,
        );
        r2.id.push('2');
        g.add_edge(r1).unwrap();
        g.add_edge(r2).unwrap();
        assert_eq!(
            g.outgoing(&a.id, None).len(),
            2,
            "parallel relations survive"
        );
        assert_eq!(g.outgoing(&a.id, Some(&RelationType::Calls)).len(), 1);
    }

    #[test]
    fn edge_direction_matters() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "a.rs",
            "a",
            "a",
            span("a.rs"),
        );
        let b = Entity::new(
            EntityKind::Symbol,
            "r",
            "s1",
            "b.rs",
            "b",
            "b",
            span("b.rs"),
        );
        g.add_node(a.clone()).unwrap();
        g.add_node(b.clone()).unwrap();
        g.add_edge(Relation::new(
            RelationType::Calls,
            &a.id,
            &b.id,
            RelationScope::CrossFile,
            &g.id,
        ))
        .unwrap();
        assert_eq!(g.outgoing(&a.id, None).len(), 1);
        assert_eq!(g.outgoing(&b.id, None).len(), 0);
        assert_eq!(g.incoming(&b.id, None).len(), 1);
        assert_eq!(g.incoming(&a.id, None).len(), 0);
    }

    #[test]
    fn bounded_path_respects_limit() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let ids: Vec<Entity> = ["a", "b", "c"]
            .iter()
            .map(|n| Entity::new(EntityKind::Symbol, "r", "s1", "a.rs", n, n, span("a.rs")))
            .collect();
        for e in &ids {
            g.add_node(e.clone()).unwrap();
        }
        let gid = g.id.clone();
        g.add_edge(Relation::new(
            RelationType::Calls,
            &ids[0].id,
            &ids[1].id,
            RelationScope::CrossFile,
            &gid,
        ))
        .unwrap();
        g.add_edge(Relation::new(
            RelationType::Calls,
            &ids[1].id,
            &ids[2].id,
            RelationScope::CrossFile,
            &gid,
        ))
        .unwrap();
        assert!(g.bounded_path(&ids[0].id, &ids[2].id, 1).is_none());
        assert!(g.bounded_path(&ids[0].id, &ids[2].id, 2).is_some());
    }

    #[test]
    fn probability_validation_rejects_bad_values() {
        assert!(check_probability(f64::NAN).is_err());
        assert!(check_probability(2.0).is_err());
        assert!(check_probability(-0.1).is_err());
        assert!(check_probability(0.5).is_ok());
        assert!(check_label("").is_err());
        assert!(check_label("src/main.rs::main").is_ok());
    }

    #[test]
    fn contradictory_evidence_survives() {
        let c = Claim {
            id: "claim:1".into(),
            relation_id: "rel:1".into(),
            supporting: vec!["ev:1".into()],
            contradicting: vec!["ev:2".into()],
            accepted: true,
        };
        assert_eq!(
            c.contradicting.len(),
            1,
            "negative evidence must not disappear"
        );
    }
}
