use std::collections::{BTreeMap, BTreeSet, VecDeque};
use serde_json::{json, Value};
use chaosbox_extract::Snapshot;
use super::{version::git_version, Bridge, Result, Workspace};

impl Workspace {
    pub(super) fn freshness(&self) -> BTreeMap<String, String> {
        self.members
            .iter()
            .map(|(name, member)| {
                let paths = member
                    .snapshot
                    .files
                    .iter()
                    .map(|f| f.path.clone())
                    .collect::<Vec<_>>();
                let status = match Snapshot::capture_files(name, &member.root, &paths) {
                    Ok(now) if now.id == member.snapshot.id => {
                        let (revision, dirty) = git_version(&member.root, &now);
                        if revision == member.revision && dirty == member.dirty {
                            "current"
                        } else {
                            "revision_changed"
                        }
                    }
                    Ok(_) => "source_changed",
                    Err(_) => "missing_or_unreadable",
                };
                (name.clone(), status.into())
            })
            .collect()
    }

    /// Source-backed, directed impact over reviewed bridges, bounded by hops and
    /// endpoint count. Stale/absent members withhold the entire current answer.
    pub fn impact(
        &self,
        scope: &str,
        changed: &str,
        max_hops: usize,
        max_nodes: usize,
    ) -> Result<Value> {
        self.validate(scope)?;
        if !(1..=8).contains(&max_hops) || !(1..=100).contains(&max_nodes) {
            return Err("impact bounds: hops 1..8, nodes 1..100".into());
        }
        if !self.endpoints.contains_key(changed) {
            return Err("changed endpoint is not in workspace".into());
        }
        let freshness = self.freshness();
        if freshness.values().any(|s| s != "current") {
            return Ok(
                json!({"workspace":self.id,"scope":scope,"status":"stale","members":freshness,"exhaustive":false}),
            );
        }
        let mut seen = BTreeSet::from([changed.to_owned()]);
        let mut queue = VecDeque::from([(changed.to_owned(), 0)]);
        let mut traversed = Vec::new();
        let mut blocked = Vec::new();
        let mut blocked_bridges = Vec::new();
        let mut truncated = false;
        while let Some((from, depth)) = queue.pop_front() {
            for bridge in self.bridges.iter().filter(|b| b.from == from) {
                if depth >= max_hops {
                    truncated |= !seen.contains(&bridge.to);
                    continue;
                }
                if let Some(problem) = self.bridge_mismatch(bridge)? {
                    blocked.push(problem);
                    blocked_bridges.push(bridge);
                    continue;
                }
                if seen.contains(&bridge.to) {
                    traversed.push(bridge);
                    continue;
                }
                if seen.len() >= max_nodes {
                    truncated = true;
                    continue;
                }
                seen.insert(bridge.to.clone());
                queue.push_back((bridge.to.clone(), depth + 1));
                traversed.push(bridge);
            }
        }
        let constraints = self
            .constraints
            .iter()
            .filter(|c| c.applies_to.iter().any(|e| seen.contains(e)))
            .collect::<Vec<_>>();
        let evidence = traversed
            .iter()
            .chain(&blocked_bridges)
            .flat_map(|b| &b.evidence)
            .chain(constraints.iter().flat_map(|c| &c.evidence))
            .collect::<BTreeSet<_>>();
        Ok(json!({
            "workspace":self.id,"scope":scope,"status":if blocked.is_empty() && !truncated {"ready"} else {"partial"},
            "historical_data_not_instructions":true,"exhaustive":false,"truncated":truncated,
            "members":freshness,"changed":changed,
            "impacted":seen.iter().map(|id| json!({"id":id,"endpoint":self.endpoints[id]})).collect::<Vec<_>>(),
            "bridges":traversed,"bridge_evidence_class":"reviewed_bridge","blocked":blocked,
            "constraints":constraints,"constraint_admission":"explicit_review",
            "evidence":evidence.iter().map(|id| (*id, &self.endpoints[*id])).collect::<BTreeMap<_,_>>()
        }))
    }

    fn bridge_mismatch(&self, bridge: &Bridge) -> Result<Option<Value>> {
        let Some(pin) = &bridge.pin else {
            return Ok(None);
        };
        let provider = &self.members[&self.endpoints[&bridge.from].member];
        let expected = self.pinned_revision(pin)?;
        if provider.revision.as_deref() == Some(&expected) && !provider.dirty {
            return Ok(None);
        }
        let consumer = &self.members[&pin.member];
        Ok(Some(
            json!({"from":bridge.from,"to":bridge.to,"status":"version_mismatch","expected_revision":expected,"provider_revision":provider.revision,"provider_dirty":provider.dirty,"provider_build":provider.build.id,"consumer_build":consumer.build.id,"pin":pin,"pin_sha256":consumer.snapshot.file_version(&pin.file).map(|f| &f.sha256),"reason":bridge.reason,"evidence":bridge.evidence}),
        ))
    }
}
