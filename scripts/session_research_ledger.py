"""Reviewable candidate inventory; cwd never determines implementation ownership."""

import hashlib
import json


def candidate_id(finding):
    identity = {key: finding[key] for key in ("session", "kind", "claim", "citations")}
    return hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()


def build_ledger(findings, bundles, decisions=None):
    decisions = {} if decisions is None else decisions
    if not isinstance(decisions, dict):
        raise ValueError("candidate decisions must be an object")
    sources = {record["ref"]: record["text"] for bundle in bundles
               for chunk in bundle["chunks"] for record in chunk["records"]}
    candidates = {}
    for cwd, rows in findings.items():
        for finding in rows:
            if finding["kind"] not in ("feature", "unfinished", "friction"):
                continue
            ident = candidate_id(finding)
            candidates[ident] = {
                "id": ident, "sourceCwdHint": cwd, "finding": finding,
                "targetRepository": None, "targetComponent": None,
                "intent": "ambiguous", "implementation": "unknown",
                "deployment": "unknown", "action": "investigate",
                "disposition": "needs-review", "ownershipEvidence": [],
            }
    if set(decisions) - candidates.keys():
        raise ValueError("review refers to unknown candidate")
    for ident, decision in decisions.items():
        if not isinstance(decision, dict):
            raise ValueError("invalid candidate decision")
        allowed = {"targetRepository", "targetComponent", "ownershipEvidence", "intent",
                   "implementation", "deployment", "action", "disposition", "duplicateOf",
                   "rationale"}
        if set(decision) - allowed:
            raise ValueError("unknown decision field")
        entry = {**candidates[ident], **decision}
        for field, values in {
            "intent": {"requested", "accepted", "rejected", "ambiguous"},
            "implementation": {"present", "partial", "unknown", "retired"},
            "deployment": {"unknown"},
            "action": {"fix", "verify", "investigate", "none"},
            "disposition": {"selected", "duplicate", "implemented", "retired", "deferred",
                            "needs-review", "needs-verification"},
        }.items():
            if entry[field] not in values:
                raise ValueError(f"invalid ledger {field}")
        if not isinstance(entry.get("rationale"), str) or not entry["rationale"].strip():
            raise ValueError("review requires rationale")
        if entry["targetRepository"] is not None:
            if (not isinstance(entry["targetRepository"], str)
                    or not entry["targetRepository"].strip()
                    or not isinstance(entry["targetComponent"], str)
                    or not entry["targetComponent"].strip()
                    or not entry["ownershipEvidence"]):
                raise ValueError("ownership requires target and evidence")
            for citation in entry["ownershipEvidence"]:
                if (not isinstance(citation, dict) or citation.get("ref") not in sources
                        or not isinstance(citation.get("quote"), str)
                        or not citation["quote"].strip()
                        or citation["quote"] not in sources[citation["ref"]]):
                    raise ValueError("invalid ownership evidence")
        if entry["disposition"] == "selected":
            if not entry["targetRepository"]:
                raise ValueError("selected candidate requires resolved ownership")
            if entry["intent"] not in {"requested", "accepted"} or entry["action"] == "none":
                raise ValueError("selected candidate requires accepted intent and an action")
        if entry["action"] == "fix" and (entry["implementation"] in {"present", "retired"}
                                           or entry["intent"] == "rejected"):
            raise ValueError("fix contradicts reviewed status")
        duplicate = entry.get("duplicateOf")
        if entry["disposition"] == "duplicate":
            if duplicate not in candidates or duplicate == ident:
                raise ValueError("invalid duplicate target")
        elif duplicate is not None:
            raise ValueError("duplicate target requires duplicate disposition")
        candidates[ident] = entry
    for ident in candidates:
        seen = set()
        current = ident
        while current is not None:
            if current in seen:
                raise ValueError("cyclic duplicate decisions")
            seen.add(current)
            current = candidates[current].get("duplicateOf")
    return {"version": 1, "candidates": list(candidates.values()),
            "conversations": [{"session": bundle["session"],
                               "records": [record for chunk in bundle["chunks"]
                                           for record in chunk["records"]]}
                              for bundle in bundles],
            "note": "Review annotations are not verification receipts. All exported conversation "
                    "context is retained, including decisions and assistant proposals. "
                    "Deployment remains unknown until separately verified."}


def review_context(ledger, repo):
    """Keep original proposals and later decisions together, without a cwd filter."""
    candidates = [row for row in ledger["candidates"]
                  if row["targetRepository"] == repo and row["disposition"] == "selected"]
    sessions = {row["finding"]["session"] for row in candidates}
    evidence_refs = {citation["ref"] for row in candidates
                     for citation in row["ownershipEvidence"]}
    conversations = [row for row in ledger["conversations"]
                     if row["session"] in sessions
                     or any(record["ref"] in evidence_refs for record in row["records"])]
    return {"candidates": candidates, "conversations": conversations}


def validate_candidate_view(report, candidates):
    """A top-eight summary cannot silently add candidates or change reviewed intent."""
    indexed = {row["id"]: row for row in candidates}
    used = set()
    for opportunity in report["opportunities"]:
        ids = opportunity.get("candidateIds")
        if not isinstance(ids, list) or not ids or not all(isinstance(i, str) for i in ids):
            raise ValueError("opportunity requires reviewed candidateIds")
        if len(set(ids)) != len(ids) or set(ids) - indexed.keys() or used.intersection(ids):
            raise ValueError("unknown or repeated candidateIds")
        used.update(ids)
        for ident in ids:
            row = indexed[ident]
            if (opportunity.get("action") != row["action"]
                    or opportunity.get("implementation") != row["implementation"]
                    or opportunity.get("deployment") != row["deployment"]):
                raise ValueError("opportunity contradicts reviewed candidate status")
    return report
