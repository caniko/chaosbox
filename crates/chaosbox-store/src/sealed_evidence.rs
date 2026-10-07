//! Publication-time evidence closure. Published readers never re-join mutable claims.

use std::collections::BTreeMap;
use chaosbox_core::{Claim, GraphBuild};
use crate::{EvidenceRow, MemoryStore, SourceCitation, StoreError};

/// Maximum publication-sealed evidence JSON bytes per relationship.
pub const SEALED_EVIDENCE_BYTES_MAX: usize = 1024 * 1024;

struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > SEALED_EVIDENCE_BYTES_MAX.saturating_sub(self.0) {
            return Err(std::io::Error::other("sealed evidence byte limit"));
        }
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl MemoryStore {
    pub(crate) fn validate_claim(&self, claim: &Claim) -> Result<(), StoreError> {
        for id in claim.supporting.iter().chain(&claim.contradicting) {
            if !self.evidence.contains_key(id) {
                return Err(StoreError::Invariant(format!(
                    "claim {} references missing evidence {id}",
                    claim.id
                )));
            }
        }
        Ok(())
    }

    /// Complete immutable evidence rows for one build's relationship, captured
    /// only after publication validation. Legacy/unsealed builds are rejected.
    pub fn sealed_evidence(&self, build: &str, rel: &str) -> Result<&[EvidenceRow], StoreError> {
        self.sealed
            .get(&(build.to_owned(), rel.to_owned()))
            .map(Vec::as_slice)
            .ok_or(StoreError::EvidenceClosureUnavailable)
    }

    pub(crate) fn seal_build(&mut self, build: &GraphBuild) -> Result<(), StoreError> {
        if self.builds.contains_key(&build.id) {
            return Err(StoreError::Invariant(
                "cannot replace a published build".into(),
            ));
        }
        let closure = self.evidence_closure(build)?;
        self.sealed.extend(closure);
        Ok(())
    }

    /// Canonical identity of the complete submitted graph and evidence closure.
    /// `TypeDB` binds the staging build to this value before any membership writes.
    pub fn publication_digest(&self, build: &GraphBuild) -> Result<String, StoreError> {
        let closure = self.publication_evidence(build)?;
        let canonical = serde_json::to_string(&(build, closure))
            .map_err(|e| StoreError::Invariant(e.to_string()))?;
        Ok(chaosbox_core::deterministic_id(
            "publication",
            &[&canonical],
        ))
    }

    /// Canonical complete evidence used to validate publication or a retry.
    /// Does not replace the closure of any already-published build.
    pub fn publication_evidence(
        &self,
        build: &GraphBuild,
    ) -> Result<BTreeMap<String, Vec<EvidenceRow>>, StoreError> {
        Ok(self
            .evidence_closure(build)?
            .into_iter()
            .map(|((_, rel), rows)| (rel, rows))
            .collect())
    }

    fn evidence_closure(
        &self,
        build: &GraphBuild,
    ) -> Result<BTreeMap<(String, String), Vec<EvidenceRow>>, StoreError> {
        let mut closure = BTreeMap::new();
        for edge in build.edges.values() {
            let mut ids = std::collections::BTreeSet::new();
            ids.extend(edge.evidence_ids.iter());
            for claim in self.claims.values().filter(|c| c.relation_id == edge.id) {
                self.validate_claim(claim)?;
                ids.extend(claim.supporting.iter().chain(&claim.contradicting));
            }
            let mut rows = Vec::new();
            for id in ids {
                let e = self
                    .evidence
                    .get(id)
                    .ok_or_else(|| StoreError::Invariant("missing edge evidence".into()))?;
                let (sha256, _) = self
                    .files
                    .get(&(e.snapshot.clone(), e.source_file_version.clone()))
                    .ok_or_else(|| StoreError::Invariant("missing evidence file version".into()))?;
                rows.push(EvidenceRow {
                    evidence_id: e.id.clone(),
                    class: chaosbox_core::evidence_class_name(e.class),
                    supports: e.supports,
                    text: e.text.clone(),
                    producer: e.producer.clone(),
                    citation: Some(SourceCitation {
                        snapshot: e.snapshot.clone(),
                        file: e.source_file_version.clone(),
                        sha256: sha256.clone(),
                        span: e.span.clone(),
                    }),
                });
            }
            if rows.len() > 1000 {
                return Err(StoreError::QueryBudget);
            }
            // Publication bounds keep a single backend attribute from growing
            // without limit. Readers still apply their smaller total budgets.
            serde_json::to_writer(&mut Counter(0), &rows).map_err(|_| StoreError::QueryBudget)?;
            closure.insert((build.id.clone(), edge.id.clone()), rows);
        }
        Ok(closure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use chaosbox_core::{
        Entity, EntityKind, Evidence, EvidenceClass, Relation, RelationScope, RelationType,
        SnapshotFile, SourceSpan,
    };
    use crate::GraphQueries;

    async fn store_with_edge() -> (MemoryStore, GraphBuild, Relation) {
        let mut store = MemoryStore::new();
        store
            .ensure_snapshot_files(
                "s",
                "r",
                &[SnapshotFile {
                    snapshot: "s".into(),
                    path: "f.rs".into(),
                    sha256: "a".repeat(64),
                    bytes: 1,
                }],
            )
            .await
            .unwrap();
        store
            .put_evidence(Evidence {
                id: "support".into(),
                class: EvidenceClass::Extracted,
                supports: true,
                text: "support".into(),
                span: None,
                snapshot: "s".into(),
                source_file_version: "f.rs".into(),
                producer: None,
            })
            .await
            .unwrap();
        let mut build = GraphBuild::new("r", vec!["s".into()], 1);
        let a = Entity::new(
            EntityKind::Symbol,
            "r",
            "s",
            "f.rs",
            "a",
            "a",
            SourceSpan::point("f.rs", 1, 1, 0),
        );
        let b = Entity::new(
            EntityKind::Symbol,
            "r",
            "s",
            "f.rs",
            "b",
            "b",
            SourceSpan::point("f.rs", 1, 1, 0),
        );
        let mut edge = Relation::new(
            RelationType::Calls,
            &a.id,
            &b.id,
            RelationScope::File,
            &build.id,
        );
        edge.evidence_ids.push("support".into());
        build.add_node(a).unwrap();
        build.add_node(b).unwrap();
        build.add_edge(edge.clone()).unwrap();
        store
            .put_claim(Claim {
                id: "claim".into(),
                relation_id: edge.id.clone(),
                supporting: vec!["support".into()],
                contradicting: vec![],
                accepted: true,
            })
            .await
            .unwrap();
        (store, build, edge)
    }

    #[tokio::test]
    async fn missing_contradiction_is_rejected_before_publication() {
        let mut store = MemoryStore::new();
        store
            .ensure_snapshot_files(
                "s",
                "r",
                &[SnapshotFile {
                    snapshot: "s".into(),
                    path: "f.rs".into(),
                    sha256: "a".repeat(64),
                    bytes: 1,
                }],
            )
            .await
            .unwrap();
        store
            .put_evidence(Evidence {
                id: "support".into(),
                class: EvidenceClass::Extracted,
                supports: true,
                text: "support".into(),
                span: None,
                snapshot: "s".into(),
                source_file_version: "f.rs".into(),
                producer: None,
            })
            .await
            .unwrap();
        let claim = Claim {
            id: "claim".into(),
            relation_id: "rel".into(),
            supporting: vec!["support".into()],
            contradicting: vec!["missing".into()],
            accepted: true,
        };
        assert!(store.put_claim(claim).await.is_err());
        assert!(store.claims.is_empty());
    }

    #[tokio::test]
    async fn publication_rechecks_legacy_missing_references() {
        let (mut store, build, edge) = store_with_edge().await;
        // Simulate legacy/inconsistent in-memory state bypassing put_claim.
        store.claims.insert(
            "legacy".into(),
            Claim {
                id: "legacy".into(),
                relation_id: edge.id,
                supporting: vec![],
                contradicting: vec!["missing".into()],
                accepted: true,
            },
        );
        assert!(store.publish(build, None).await.is_err());
        assert!(store.builds.is_empty());
    }

    #[tokio::test]
    async fn historical_evidence_survives_later_claims_and_file_mutation() {
        let (mut store, build, edge) = store_with_edge().await;
        store.publish(build.clone(), None).await.unwrap();
        let before = crate::MemoryReader::from_store(&store)
            .evidence_for(&build.id, &edge.id)
            .await
            .unwrap();
        store
            .put_evidence(Evidence {
                id: "contradiction".into(),
                class: EvidenceClass::Extracted,
                supports: false,
                text: "later contradiction".into(),
                span: None,
                snapshot: "s".into(),
                source_file_version: "f.rs".into(),
                producer: None,
            })
            .await
            .unwrap();
        store
            .put_claim(Claim {
                id: "later".into(),
                relation_id: edge.id.clone(),
                supporting: vec![],
                contradicting: vec!["contradiction".into()],
                accepted: false,
            })
            .await
            .unwrap();
        // Even pre-existing inconsistent mutable file data cannot rewrite the
        // citation captured in an already-published evidence closure.
        store
            .files
            .insert(("s".into(), "f.rs".into()), ("b".repeat(64), 2));
        let after = crate::MemoryReader::from_store(&store)
            .evidence_for(&build.id, &edge.id)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        let mut next = GraphBuild::new("r", vec!["s".into()], 2);
        next.nodes = build.nodes.clone();
        next.edges = build.edges.clone();
        store
            .publish(next.clone(), Some(build.id.clone()))
            .await
            .unwrap();
        assert_eq!(store.sealed_evidence(&next.id, &edge.id).unwrap().len(), 2);
        assert_eq!(store.sealed_evidence(&build.id, &edge.id).unwrap().len(), 1);
    }
}
