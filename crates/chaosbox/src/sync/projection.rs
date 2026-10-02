//! Canonical receipt DAGs, conservative concurrent lifecycle handling, and views.
use std::collections::{BTreeMap, BTreeSet};
use chaosbox_core::{
    sha256_hex,
    intelligence::{Intelligence, IntelligenceStatus},
};
use serde::{Deserialize, Serialize};
use crate::intelligence::{Assessment, Bundle, Outcome};
use super::{Payload, Replica, Action};

/// A replayed relationship or an explicit conflict between concurrent model receipts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    /// Left item identity.
    pub left: String,
    /// Right item identity.
    pub right: String,
    /// Agreed action, or abstention when competing receipts disagree.
    pub action: Action,
    /// Maximal signed receipt events backing this relationship.
    pub receipts: Vec<String>,
    /// True when concurrent assessments disagree.
    pub unresolved: bool,
}

/// An immutable locally published read snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    /// Canonical history identity; comparable across hosts, unlike local generations.
    pub digest: String,
    /// The complete causal frontier used for this snapshot.
    pub heads: Vec<String>,
    /// Validated baseline knowledge with causally ordered receipts.
    pub bundle: Bundle,
    /// Semantics projected from replicated receipts; never independent evidence.
    pub relationships: Vec<Relationship>,
    /// Complete replayable semantic receipts, distinct from source occurrences.
    pub semantic_receipts: BTreeMap<String, super::Resolution>,
}

impl View {
    /// Validate persisted consumer references before projecting or indexing them.
    pub fn validate(&self) -> Result<(), String> {
        self.bundle.validate()?;
        super::validate_metadata(&self.bundle)?;
        let ids: BTreeSet<_> = self.bundle.records.iter().map(|r| &r.id).collect();
        let mut pairs = BTreeSet::new();
        if !super::is_digest(&self.digest)
            || self.heads.len() > super::MAX_EVENTS
            || self.heads.windows(2).any(|w| w[0] >= w[1])
            || self.heads.iter().any(|id| !super::is_digest(id))
        {
            return Err("invalid current view frontier".into());
        }
        for r in &self.relationships {
            if r.left >= r.right
                || !ids.contains(&r.left)
                || !ids.contains(&r.right)
                || !pairs.insert((&r.left, &r.right))
                || r.receipts.is_empty()
                || r.receipts.windows(2).any(|w| w[0] >= w[1])
                || r.receipts.iter().any(|id| !super::is_digest(id))
                || r.receipts.iter().any(|id| {
                    self.semantic_receipts
                        .get(id)
                        .is_none_or(|a| a.job.left != r.left || a.job.right != r.right)
                })
                || r.unresolved && r.action != Action::Abstain
            {
                return Err("invalid current view relationship".into());
            }
        }
        for (id, receipt) in &self.semantic_receipts {
            if !super::is_digest(id) || receipt.job.scope != self.bundle.scope {
                return Err("invalid semantic receipt scope/identity".into());
            }
            receipt.validate()?;
        }
        Ok(())
    }
}

pub(crate) fn project(replica: &Replica) -> Result<View, String> {
    // Kahn's algorithm detects missing dependencies and cycles without recursion.
    let graph: BTreeMap<_, _> = replica
        .events
        .iter()
        .map(|(id, e)| (id.clone(), e.parents.iter().cloned().collect()))
        .collect();
    ordered(&graph, |_| false)?;
    let bundle = baseline(replica)?;
    let mut groups: BTreeMap<(String, String), Vec<(&super::SignedEvent, &super::Resolution)>> =
        BTreeMap::new();
    for event in replica.events.values() {
        let Payload::Resolution(receipt) = &event.payload else {
            continue;
        };
        let ancestors = ancestors(replica, &event.parents)?;
        let source = baseline(&ancestors)?;
        let left = source
            .records
            .iter()
            .find(|r| r.id == receipt.job.left)
            .ok_or("reconciliation left source missing")?;
        let right = source
            .records
            .iter()
            .find(|r| r.id == receipt.job.right)
            .ok_or("reconciliation right source missing")?;
        let expected =
            super::reconcile::pair_job(left, right, &ancestors, receipt.job.prior.clone())?;
        let known_heads = super::reconcile::receipt_heads(&ancestors, &expected);
        if expected != receipt.job
            || receipt.job.prior.iter().any(|id| {
                !matches!(
                    ancestors.events.get(id).map(|e| &e.payload),
                    Some(Payload::Resolution(_))
                )
            })
        {
            return Err("reconciliation input does not match its causal source frontier".into());
        }
        if !receipt.job.prior.is_empty() && receipt.job.prior != known_heads {
            return Err(
                "reconciliation must consider every maximal decision for the same inputs".into(),
            );
        }
        let Some(current_left) = bundle.records.iter().find(|r| r.id == receipt.job.left) else {
            continue;
        };
        let Some(current_right) = bundle.records.iter().find(|r| r.id == receipt.job.right) else {
            continue;
        };
        let current = super::reconcile::pair_job(current_left, current_right, replica, vec![])?;
        if current.left_version == receipt.job.left_version
            && current.right_version == receipt.job.right_version
        {
            groups
                .entry((receipt.job.left.clone(), receipt.job.right.clone()))
                .or_default()
                .push((event, receipt));
        }
    }
    let mut relationships = Vec::new();
    for ((left, right), receipts) in groups {
        let consumed: BTreeSet<_> = receipts.iter().flat_map(|(_, r)| &r.job.prior).collect();
        let heads: Vec<_> = receipts
            .iter()
            .filter(|(e, _)| !consumed.contains(&e.id))
            .collect();
        let actions: BTreeSet<_> = heads.iter().map(|(_, r)| r.action).collect();
        relationships.push(Relationship {
            left,
            right,
            action: if actions.len() == 1 {
                *actions.first().ok_or("missing semantic action")?
            } else {
                Action::Abstain
            },
            receipts: heads.iter().map(|(e, _)| e.id.clone()).collect(),
            unresolved: actions.len() > 1,
        });
    }
    Ok(View {
        digest: replica.digest()?,
        heads: replica.heads(),
        bundle,
        relationships,
        semantic_receipts: replica
            .events
            .iter()
            .filter_map(|(id, e)| {
                if let Payload::Resolution(r) = &e.payload {
                    Some((id.clone(), r.clone()))
                } else {
                    None
                }
            })
            .collect(),
    })
}

fn ancestors(replica: &Replica, roots: &[String]) -> Result<Replica, String> {
    let mut result = Replica::new(&replica.user, &replica.scope);
    let mut pending = roots.to_vec();
    while let Some(id) = pending.pop() {
        if result.events.contains_key(&id) {
            continue;
        }
        let event = replica.events.get(&id).ok_or("missing causal source")?;
        pending.extend(event.parents.iter().cloned());
        result.events.insert(id, event.clone());
    }
    Ok(result)
}

fn baseline(replica: &Replica) -> Result<Bundle, String> {
    let mut records: BTreeMap<String, Intelligence> = BTreeMap::new();
    let mut receipts: BTreeMap<String, Assessment> = BTreeMap::new();
    let mut chains: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    let mut coverage = BTreeMap::new();
    for event in replica.events.values() {
        let Payload::Publication(publication) = &event.payload else {
            continue;
        };
        for receipt in &publication.bundle.assessments {
            if let Some(old) = receipts.insert(receipt.id.clone(), receipt.clone()) {
                if serde_json::to_value(old).map_err(|_| "encode receipt")?
                    != serde_json::to_value(receipt).map_err(|_| "encode receipt")?
                {
                    return Err("receipt identity collision".into());
                }
            }
        }
        for c in &publication.bundle.coverage {
            coverage.insert(
                serde_json::to_string(c).map_err(|_| "encode coverage")?,
                c.clone(),
            );
        }
        for record in &publication.bundle.records {
            let chain = chains.entry(record.id.clone()).or_default();
            for id in &record.assessments {
                chain.entry(id.clone()).or_default();
            }
            for pair in record.assessments.windows(2) {
                chain
                    .entry(pair[1].clone())
                    .or_default()
                    .insert(pair[0].clone());
            }
            match records.get_mut(&record.id) {
                None => {
                    records.insert(record.id.clone(), record.clone());
                }
                Some(old) => {
                    if old.scope != record.scope
                        || old.statement != record.statement
                        || old.repositories != record.repositories
                        || old.evidence.first() != record.evidence.first()
                    {
                        return Err("incompatible intelligence identity".into());
                    }
                    old.evidence.extend(record.evidence.iter().cloned());
                    old.contradicts.extend(record.contradicts.iter().cloned());
                    if old.supersedes.is_some()
                        && record.supersedes.is_some()
                        && old.supersedes != record.supersedes
                    {
                        return Err("incompatible supersession identity".into());
                    }
                    old.supersedes = old.supersedes.clone().or_else(|| record.supersedes.clone());
                }
            }
        }
    }
    for record in records.values_mut() {
        finalize_record(record, &chains[&record.id], &receipts)?;
    }
    let bundle = Bundle {
        version: 1,
        scope: replica.scope.clone(),
        records: records.into_values().collect(),
        assessments: receipts.into_values().collect(),
        coverage: coverage.into_values().collect(),
    };
    bundle.validate()?;
    Ok(bundle)
}

fn finalize_record(
    record: &mut Intelligence,
    chain: &BTreeMap<String, BTreeSet<String>>,
    receipts: &BTreeMap<String, Assessment>,
) -> Result<(), String> {
    // Concurrent withholding wins over admission until a causally later readmission.
    let predecessors: BTreeSet<_> = chain.values().flatten().collect();
    // Only maximal withholding is deferred. Deferring intermediate negative
    // receipts would let one branch's child outrank another branch's head.
    record.assessments = ordered(chain, |id| {
        !predecessors.contains(&id.to_owned())
            && receipts
                .get(id)
                .is_some_and(|r| matches!(r.outcome, Outcome::Rejected | Outcome::Abstained))
    })?;
    // Put a valid original admission first, preserving the legacy bundle invariant.
    let first = record
        .evidence
        .first()
        .cloned()
        .ok_or("missing original evidence")?;
    let first_digest =
        sha256_hex(&[&serde_json::to_string(&first).map_err(|_| "encode evidence")?]);
    let admission = record
        .assessments
        .iter()
        .position(|id| {
            receipts.get(id).is_some_and(|a| {
                record.id == format!("intel:{}", sha256_hex(&[&a.candidate_id]))
                    && a.evidence_digest == first_digest
                    && matches!(
                        a.outcome,
                        Outcome::Admitted | Outcome::Contradiction(_) | Outcome::Supersession(_)
                    )
            })
        })
        .ok_or("missing original admission")?;
    if admission != 0 {
        let id = record.assessments.remove(admission);
        record.assessments.insert(0, id);
    }
    record.kind = crate::intelligence::replication_kind(
        receipts
            .get(&record.assessments[0])
            .ok_or("missing original classification")?,
    )?;
    let mut unique = BTreeMap::new();
    for e in &record.evidence {
        unique.insert(
            serde_json::to_string(e).map_err(|_| "encode evidence")?,
            e.clone(),
        );
    }
    record.evidence = vec![first.clone()];
    record
        .evidence
        .extend(unique.into_values().filter(|e| e != &first));
    record.contradicts.sort();
    record.contradicts.dedup();
    if receipts
        .values()
        .any(|a| matches!(&a.outcome, Outcome::Supersession(id) if id == &record.id))
    {
        record.status = IntelligenceStatus::Superseded;
    } else {
        let latest = record
            .assessments
            .iter()
            .rev()
            .filter_map(|id| receipts.get(id))
            .find(|a| {
                record.evidence.iter().any(|e| {
                    serde_json::to_string(e).is_ok_and(|s| sha256_hex(&[&s]) == a.evidence_digest)
                })
            });
        record.status = if latest
            .is_some_and(|a| matches!(a.outcome, Outcome::Rejected | Outcome::Abstained))
        {
            IntelligenceStatus::Withheld
        } else if record.contradicts.is_empty() {
            IntelligenceStatus::Admitted
        } else {
            IntelligenceStatus::Disputed
        };
    }
    Ok(())
}

fn ordered(
    graph: &BTreeMap<String, BTreeSet<String>>,
    negative: impl Fn(&str) -> bool,
) -> Result<Vec<String>, String> {
    let mut counts = BTreeMap::new();
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut ready = BTreeSet::new();
    for (id, parents) in graph {
        counts.insert(id.as_str(), parents.len());
        if parents.is_empty() {
            ready.insert((negative(id), id.as_str()));
        }
        for parent in parents {
            if !graph.contains_key(parent) {
                return Err("incomplete causal history; last good snapshot retained".into());
            }
            children.entry(parent).or_default().push(id);
        }
    }
    let mut result = Vec::new();
    while let Some((_, next)) = ready.pop_first() {
        result.push(next.to_owned());
        for child in children.get(next).into_iter().flatten() {
            let count = counts.get_mut(child).ok_or("missing causal child")?;
            *count -= 1;
            if *count == 0 {
                ready.insert((negative(child), *child));
            }
        }
    }
    if result.len() != graph.len() {
        return Err("cyclic causal history; last good snapshot retained".into());
    }
    Ok(result)
}
