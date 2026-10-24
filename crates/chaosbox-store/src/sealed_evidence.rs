//! Publication-time evidence closure. Published readers never re-join mutable claims.

use std::collections::BTreeMap;
use chaosbox_core::{Claim, GraphBuild, Relation};
use serde::{ser::SerializeMap, Serializer as _};
use sha2::{Digest, Sha256};
use crate::{EvidenceRow, MemoryStore, SourceCitation, StoreError};

/// Maximum publication-sealed evidence JSON bytes per relationship.
pub const SEALED_EVIDENCE_BYTES_MAX: usize = 1024 * 1024;

/// One relationship's evidence class for analytics. Rejected decisions
/// survive as contradicting rows, so any contradiction or ambiguous support
/// is `ambiguous`; otherwise the weakest supporting class wins.
fn edge_class(rows: &[EvidenceRow]) -> &'static str {
    if rows.iter().any(|r| !r.supports || r.class == "ambiguous") {
        "ambiguous"
    } else if rows.iter().any(|r| r.class == "inferred") {
        "inferred"
    } else if rows.is_empty() {
        "unknown"
    } else {
        "extracted"
    }
}

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

    /// Sealed evidence class of one build relationship for publication
    /// analytics; `None` for unsealed builds.
    #[must_use]
    pub fn edge_class(&self, build: &str, rel: &str) -> Option<&'static str> {
        self.sealed_evidence(build, rel).ok().map(edge_class)
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
        let encode_error = |error: serde_json::Error| StoreError::Invariant(error.to_string());
        let claims = self.claims_by_relation();
        // Match evidence_closure's member-id keys even for deserialized maps
        // whose keys differ from their values' ids. Only references are held.
        let mut edges = BTreeMap::new();
        for edge in build.edges.values() {
            if let Some(shadowed) = edges.insert(edge.id.as_str(), edge) {
                self.evidence_rows(
                    shadowed,
                    claims.get(shadowed.id.as_str()).map_or(&[], Vec::as_slice),
                )?;
            }
        }
        // Exactly the original canonical `(build, closure)` JSON plus the NUL
        // separator used by deterministic_id, without retaining either copy.
        let mut digest = Sha256::new();
        digest.update(b"[");
        serde_json::to_writer(&mut digest, build).map_err(encode_error)?;
        digest.update(b",");
        let mut serializer = serde_json::Serializer::new(&mut digest);
        let mut closure = serializer
            .serialize_map(Some(edges.len()))
            .map_err(encode_error)?;
        for (id, edge) in edges {
            let rows = self.evidence_rows(
                edge,
                claims.get(edge.id.as_str()).map_or(&[], Vec::as_slice),
            )?;
            closure.serialize_entry(id, &rows).map_err(encode_error)?;
        }
        closure.end().map_err(encode_error)?;
        digest.update(b"]\0");
        let hash = format!("{:x}", digest.finalize());
        Ok(format!("publication:{}", &hash[..12]))
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
        let claims = self.claims_by_relation();
        let mut closure = BTreeMap::new();
        for edge in build.edges.values() {
            let rows = self.evidence_rows(
                edge,
                claims.get(edge.id.as_str()).map_or(&[], Vec::as_slice),
            )?;
            closure.insert((build.id.clone(), edge.id.clone()), rows);
        }
        Ok(closure)
    }

    fn claims_by_relation(&self) -> BTreeMap<&str, Vec<&Claim>> {
        let mut claims: BTreeMap<&str, Vec<&Claim>> = BTreeMap::new();
        for claim in self.claims.values() {
            claims.entry(&claim.relation_id).or_default().push(claim);
        }
        claims
    }

    fn evidence_rows(
        &self,
        edge: &Relation,
        claims: &[&Claim],
    ) -> Result<Vec<EvidenceRow>, StoreError> {
        let mut ids = std::collections::BTreeSet::new();
        ids.extend(edge.evidence_ids.iter());
        for claim in claims {
            self.validate_claim(claim)?;
            ids.extend(claim.supporting.iter().chain(&claim.contradicting));
        }
        if ids.len() > 1000 {
            return Err(StoreError::QueryBudget);
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
        // Retain the per-relationship publication limit while streaming.
        serde_json::to_writer(&mut Counter(0), &rows).map_err(|_| StoreError::QueryBudget)?;
        Ok(rows)
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
    async fn publication_and_flush_views_borrow_validated_payloads() {
        let (mut store, build, _) = store_with_edge().await;
        let view = store.staged();
        assert!(std::ptr::eq(view.evidence, &raw const store.evidence));
        assert!(std::ptr::eq(view.claims, &raw const store.claims));
        assert_eq!(view.evidence.len(), store.export_staged().evidence.len());
        let id = build.id.clone();
        store.publish(build, None).await.unwrap();
        let published = store.get_ref(&id).unwrap();
        assert!(std::ptr::eq(published, store.active_ref("r").unwrap()));
        assert_eq!(store.get(&id).unwrap(), *published);
        assert!(store.get_ref("missing").is_none());
        assert!(store.active_ref("missing").is_none());
    }

    #[tokio::test]
    async fn publication_digest_matches_canonical_envelope() {
        let (mut store, build, edge) = store_with_edge().await;
        store
            .put_evidence(Evidence {
                id: "contradiction".into(),
                class: EvidenceClass::Extracted,
                supports: false,
                text: "quoted \"text\", newline\n and Unicode: λ".into(),
                span: None,
                snapshot: "s".into(),
                source_file_version: "f.rs".into(),
                producer: None,
            })
            .await
            .unwrap();
        store
            .put_claim(Claim {
                id: "another-claim".into(),
                relation_id: edge.id,
                supporting: vec!["support".into()],
                contradicting: vec!["contradiction".into()],
                accepted: false,
            })
            .await
            .unwrap();
        let canonical =
            serde_json::to_string(&(&build, store.publication_evidence(&build).unwrap())).unwrap();
        assert_eq!(
            store.publication_digest(&build).unwrap(),
            chaosbox_core::deterministic_id("publication", &[&canonical])
        );
        let empty = GraphBuild::new("empty", vec![], 1);
        let canonical =
            serde_json::to_string(&(&empty, BTreeMap::<String, Vec<crate::EvidenceRow>>::new()))
                .unwrap();
        assert_eq!(
            store.publication_digest(&empty).unwrap(),
            chaosbox_core::deterministic_id("publication", &[&canonical])
        );
    }

    #[tokio::test]
    async fn publication_digest_preserves_evidence_key_order_and_deduplication() {
        let (store, mut build, edge) = store_with_edge().await;
        build.edges.clear();
        let mut earlier = edge.clone();
        earlier.id = "a-first".into();
        // Deserialized public maps can have keys unlike their member ids. The
        // canonical evidence map sorts/deduplicates by member id, not map key.
        build.edges.insert("first-key".into(), edge.clone());
        build.edges.insert("second-key".into(), earlier);
        build.edges.insert("third-key".into(), edge);
        let canonical =
            serde_json::to_string(&(&build, store.publication_evidence(&build).unwrap())).unwrap();
        assert_eq!(
            store.publication_digest(&build).unwrap(),
            chaosbox_core::deterministic_id("publication", &[&canonical])
        );
        build
            .edges
            .get_mut("first-key")
            .unwrap()
            .evidence_ids
            .push("missing".into());
        assert!(store.publication_evidence(&build).is_err());
        assert!(store.publication_digest(&build).is_err());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "isolated RSS regression; run explicitly with --test-threads=1"]
    async fn publication_digest_memory_budget() {
        fn peak_kib() -> u64 {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find_map(|line| {
                    line.strip_prefix("VmHWM:").map(|value| {
                        value
                            .split_whitespace()
                            .next()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    })
                })
                .unwrap()
        }
        let (mut store, mut build, edge) = store_with_edge().await;
        for n in 0..12_000 {
            let id = format!("evidence-{n:05}");
            store
                .put_evidence(Evidence {
                    id: id.clone(),
                    class: EvidenceClass::Extracted,
                    supports: true,
                    text: "x".repeat(2048),
                    span: None,
                    snapshot: "s".into(),
                    source_file_version: "f.rs".into(),
                    producer: None,
                })
                .await
                .unwrap();
            let mut relation = edge.clone();
            relation.id = format!("relation-{n:05}");
            relation.evidence_ids = vec![id.clone()];
            store
                .put_claim(Claim {
                    id: format!("claim-{n:05}"),
                    relation_id: relation.id.clone(),
                    supporting: vec![id],
                    contradicting: vec![],
                    accepted: true,
                })
                .await
                .unwrap();
            build.add_edge(relation).unwrap();
        }
        let before = peak_kib();
        let started = std::time::Instant::now();
        let digest = store.publication_digest(&build).unwrap();
        let growth = peak_kib().saturating_sub(before);
        eprintln!(
            "publication digest: {digest}; peak growth={growth} KiB; elapsed={:?}",
            started.elapsed()
        );
        assert!(
            growth < 16 * 1024,
            "digest must stream, not copy the graph's evidence/JSON: grew {growth} KiB"
        );
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
