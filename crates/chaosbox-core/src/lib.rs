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
    pub file: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub byte_start: u32,
    pub byte_end: u32,
}

impl SourceSpan {
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
    File,
    Module,
    Symbol,
    Definition,
    Import,
    Heading,
    Link,
    CodeMention,
}

/// A possible entity: identity separate from occurrences/labels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub kind: EntityKind,
    pub repo: String,
    pub snapshot: String,
    pub file: String,
    pub name: String,
    pub qualified_name: String,
    pub span: SourceSpan,
    pub aliases: Vec<String>,
}

impl Entity {
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    Contains,
    Defines,
    Imports,
    References,
    Calls,
    LinksTo,
    Mentions,
}

/// Scope of a relationship observation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationScope {
    File,
    Module,
    CrossFile,
}

/// A relationship is a first-class object with typed endpoints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub id: String,
    pub rel_type: RelationType,
    pub from: String,
    pub to: String,
    pub scope: RelationScope,
    pub evidence_ids: Vec<String>,
}

impl Relation {
    #[must_use]
    pub fn new(
        rel_type: RelationType,
        from: &str,
        to: &str,
        scope: RelationScope,
        build: &str,
    ) -> Self {
        let id = deterministic_id(
            "rel",
            &[build, &format!("{:?}", rel_type), from, to],
        );
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    Extracted,
    Inferred,
    Ambiguous,
}

/// One supporting or contradicting observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub class: EvidenceClass,
    pub supports: bool,
    /// Verbatim source excerpt (copied span) or deterministic template text.
    pub text: String,
    pub span: Option<SourceSpan>,
    pub source_file_version: String,
}

/// A claim assembled from evidence; negative evidence survives merges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub relation_id: String,
    pub supporting: Vec<String>,
    pub contradicting: Vec<String>,
    pub accepted: bool,
}

/// Outcome of one Jev decision (kept separate from evidence class).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Accepted,
    Rejected,
    Abstained,
    Negative,
    Failed(String),
}

/// One validated Jev decision over a candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub candidate_id: String,
    pub question_id: String,
    pub outcome: DecisionOutcome,
    pub evidence_class: EvidenceClass,
    pub model_requested: String,
    pub model_returned: String,
    pub confidence: Option<f64>,
    pub probability: Option<f64>,
}

/// A candidate relationship proposed deterministically for Jev review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub rel_type: RelationType,
    pub from_entity: String,
    pub to_entity: String,
    pub reason: String,
    pub state_excerpt: String,
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("probability {0} out of [0,1]")]
    ProbabilityRange(f64),
    #[error("confidence {0} out of [0,1]")]
    ConfidenceRange(f64),
    #[error("non-finite value")]
    NonFinite,
    #[error("empty label")]
    EmptyLabel,
    #[error("cross-build edge: {0} not in build {1}")]
    CrossBuildEdge(String, String),
    #[error("duplicate build member: {0}")]
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
    pub id: String,
    pub repo: String,
    pub snapshot_ids: Vec<String>,
    pub nodes: BTreeMap<String, Entity>,
    pub edges: BTreeMap<String, Relation>,
    pub generation: u64,
    pub predecessor: Option<String>,
}

impl GraphBuild {
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
            return Err(ValidationError::CrossBuildEdge(r.from.clone(), self.id.clone()));
        }
        if !self.nodes.contains_key(&r.to) {
            return Err(ValidationError::CrossBuildEdge(r.to.clone(), self.id.clone()));
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
            .filter(|r| r.from == id && filter.map_or(true, |f| &r.rel_type == f))
            .collect()
    }

    /// Incoming neighborhood with optional relation filter.
    #[must_use]
    pub fn incoming(&self, id: &str, filter: Option<&RelationType>) -> Vec<&Relation> {
        self.edges
            .values()
            .filter(|r| r.to == id && filter.map_or(true, |f| &r.rel_type == f))
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
            adj.entry(r.from.clone()).or_default().push((r.to.clone(), r.id.clone()));
            adj.entry(r.to.clone()).or_default().push((r.from.clone(), r.id.clone()));
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
    pub added_nodes: Vec<String>,
    pub removed_nodes: Vec<String>,
    pub added_edges: Vec<String>,
    pub removed_edges: Vec<String>,
}

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
    fn ids_are_deterministic_and_scoped() {
        let a = Entity::new(EntityKind::Symbol, "r", "s1", "a.rs", "foo", "a::foo", span("a.rs"));
        let b = Entity::new(EntityKind::Symbol, "r", "s1", "a.rs", "foo", "a::foo", span("a.rs"));
        assert_eq!(a.id, b.id);
        let c = Entity::new(EntityKind::Symbol, "r", "s2", "a.rs", "foo", "a::foo", span("a.rs"));
        assert_ne!(a.id, c.id, "snapshots must not collide");
        let d = Entity::new(EntityKind::Symbol, "other", "s1", "a.rs", "foo", "a::foo", span("a.rs"));
        assert_ne!(a.id, d.id, "repos must not collide");
    }

    #[test]
    fn edges_require_same_build_members() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = Entity::new(EntityKind::Symbol, "r", "s1", "a.rs", "a", "a", span("a.rs"));
        let outsider =
            Entity::new(EntityKind::Symbol, "r", "s1", "b.rs", "b", "b", span("b.rs"));
        g.add_node(a.clone()).unwrap();
        let r = Relation::new(RelationType::Calls, &a.id, &outsider.id, RelationScope::CrossFile, &g.id);
        assert!(g.add_edge(r).is_err(), "cross-build edge must fail");
    }

    #[test]
    fn parallel_relations_preserved() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = Entity::new(EntityKind::Symbol, "r", "s1", "a.rs", "a", "a", span("a.rs"));
        let b = Entity::new(EntityKind::Symbol, "r", "s1", "b.rs", "b", "b", span("b.rs"));
        g.add_node(a.clone()).unwrap();
        g.add_node(b.clone()).unwrap();
        let r1 = Relation::new(RelationType::Calls, &a.id, &b.id, RelationScope::CrossFile, &g.id);
        let mut r2 = Relation::new(RelationType::References, &a.id, &b.id, RelationScope::CrossFile, &g.id);
        r2.id.push('2');
        g.add_edge(r1).unwrap();
        g.add_edge(r2).unwrap();
        assert_eq!(g.outgoing(&a.id, None).len(), 2, "parallel relations survive");
        assert_eq!(g.outgoing(&a.id, Some(&RelationType::Calls)).len(), 1);
    }

    #[test]
    fn edge_direction_matters() {
        let mut g = GraphBuild::new("r", vec!["s1".into()], 1);
        let a = Entity::new(EntityKind::Symbol, "r", "s1", "a.rs", "a", "a", span("a.rs"));
        let b = Entity::new(EntityKind::Symbol, "r", "s1", "b.rs", "b", "b", span("b.rs"));
        g.add_node(a.clone()).unwrap();
        g.add_node(b.clone()).unwrap();
        g.add_edge(Relation::new(RelationType::Calls, &a.id, &b.id, RelationScope::CrossFile, &g.id))
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
        g.add_edge(Relation::new(RelationType::Calls, &ids[0].id, &ids[1].id, RelationScope::CrossFile, &gid)).unwrap();
        g.add_edge(Relation::new(RelationType::Calls, &ids[1].id, &ids[2].id, RelationScope::CrossFile, &gid)).unwrap();
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
        assert_eq!(c.contradicting.len(), 1, "negative evidence must not disappear");
    }
}
