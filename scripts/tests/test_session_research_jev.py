"""Typed decisions must never invent citations or silently discard abstentions."""

import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import session_research_jev as jev


def response(questions, selected=None):
    selected = selected or {}
    return {"model": jev.MODEL, "usage": {"input_tokens": 100, "output_tokens": 10},
            "answers": {key: {"type": "choice", "choice": selected.get(key, next(iter(q["criteria"]))),
                        "probabilities": {option: float(option == selected.get(key, next(iter(q["criteria"]))))
                                          for option in q["criteria"]}, "confidence": 1.0}
                        for key, q in questions.items()}}


class TypedResearchTest(unittest.TestCase):
    def test_wire_rejects_missing_options_wrong_model_and_nonfinite_values(self):
        questions = {"q": jev.choice("Classify", {"feature": "Feature", "none": "None"})}
        valid = response(questions)
        jev.validate_response(valid, questions)
        for mutate in (
            lambda r: r.update(model="jev-latest"),
            lambda r: r["answers"]["q"]["probabilities"].pop("none"),
            lambda r: r["answers"]["q"].update(confidence=float("nan")),
            lambda r: r["answers"]["q"].update(choice="none"),
            lambda r: r["answers"]["q"].update(type="noul"),
            lambda r: r["answers"].update(extra=r["answers"]["q"]),
        ):
            with self.subTest(mutate=mutate):
                bad = copy.deepcopy(valid)
                mutate(bad)
                with self.assertRaises(ValueError):
                    jev.validate_response(bad, questions)

    def test_extraction_uses_exact_spans_and_accounts_for_every_decision(self):
        record = {"ref": "ref", "text": "Add export.\nKeep the existing scope.", "role": "user"}
        chunk = {"id": "chunk", "records": [record]}
        seen = []

        def evaluate(state, questions):
            seen.append((state, questions))
            answer = response(questions, {key: "feature" for key in questions})
            # Uncertain classification must stay in coverage, not become a claim.
            last = next(reversed(answer["answers"]))
            answer["answers"][last]["confidence"] = 0.1
            return answer

        result = jev.extract(chunk, {}, evaluate)
        self.assertEqual(result["signals"][0]["claim"], "Add export.")
        self.assertEqual(result["signals"][0]["citations"], [{"ref": "ref", "quote": "Add export."}])
        self.assertEqual(result["coverage"]["abstained"], 1)
        self.assertEqual(result["coverage"]["candidates"], 2)
        self.assertTrue(all("ref" in q["instructions"] for _, qs in seen for q in qs.values()))

    def test_extraction_limit_retains_omitted_span_accounting(self):
        chunk = {"id": "long", "records": [{"ref": "ref", "role": "user",
                  "text": "\n".join(f"Add feature {i}." for i in range(20))}]}
        result = jev.extract(chunk, {}, lambda s, qs: response(qs, {q: "feature" for q in qs}))
        self.assertEqual(len(result["signals"]), 12)
        self.assertEqual(result["coverage"]["omitted"], 8)
        self.assertEqual(len(result["decisions"]), 20)

    def test_short_user_reply_is_citable_but_short_fragment_is_accounted_without_inference(self):
        chunk = {"id": "short", "records": [
            {"ref": "reply", "role": "user", "text": "go"},
            {"ref": "longer", "role": "user", "text": "Yes. Add exports."}]}
        asked = []

        def evaluate(state, questions):
            asked.extend(questions.values())
            return response(questions, {key: "decision" for key in questions})

        result = jev.extract(chunk, {}, evaluate)
        self.assertEqual(len(asked), 2)
        self.assertEqual(result["signals"][0]["citations"][0]["quote"], "go")
        self.assertEqual(result["coverage"]["citationExcluded"], 1)
        self.assertEqual(result["excludedSpans"][0]["quote"], "Yes.")

    @staticmethod
    def payload(count=1):
        rows = [{"id": f"c{i}", "targetRepository": "canix", "targetComponent": "exports",
                 "action": "verify", "implementation": "present", "deployment": "unknown",
                 "finding": {"claim": f"Add export {i}.", "kind": "feature", "citations": [
                     {"ref": f"r{i}", "quote": f"Add export {i}."}]}} for i in range(count)]
        return {"candidates": rows, "targetRepository": "canix", "repoContext": {},
                "conversations": [], "sourceMeta": {}, "reproductions": {}}

    def test_report_preserves_review_and_retains_uncertain_candidates_and_top_eight_omissions(self):
        payload = self.payload(10)
        result = jev.report(payload, lambda s, qs: response(qs, {q: "uncertain" for q in qs}))
        self.assertEqual(len(result["opportunities"]), 8)
        self.assertEqual(result["coverage"]["omittedCandidateIds"], ["c8", "c9"])
        for item in result["opportunities"]:
            self.assertEqual(item["priority"], "unverified")
            self.assertEqual(item["currentStatus"], "unverified")
            self.assertEqual(item["action"], "verify")
            self.assertEqual(item["implementation"], "present")
            self.assertEqual(item["deployment"], "unknown")
            self.assertFalse(item["repoEvidence"])

    def test_current_status_follows_selected_exact_evidence_and_valid_reproduction_only(self):
        payload = self.payload()
        revision = "a" * 40
        payload["repoContext"] = {f"{revision}:export.rs#L1": "fn export() { fail(); }"}
        good = {"sourceRevision": revision, "sourceState": "clean", "command": ["check"],
                "expected": "Export succeeds", "observed": "Export fails", "outputSha256": "b" * 64,
                "result": "reproduced-defect"}
        payload["reproductions"] = {"matching": good,
                                    "stale": {**good, "sourceRevision": "c" * 40},
                                    "dirty": {**good, "sourceState": "dirty"}}
        seen = []

        def evaluate(state, qs):
            seen.append(qs)
            return response(qs, {key: "low" if key.startswith("p") else "e0" if key.startswith("e")
                                 else "gap0" for key in qs})

        result = jev.report(payload, evaluate)
        item = result["opportunities"][0]
        self.assertEqual(item["currentStatus"], "confirmed-gap")
        self.assertEqual(item["reproductionRef"], "matching")
        self.assertEqual(item["repoEvidence"], [{"ref": next(iter(payload["repoContext"])),
                                              "quote": "fn export() { fail(); }"}])
        self.assertEqual(set(seen[-1]["c0"]["criteria"]),
                         {"unverified", "present", "partial", "rejected", "gap0"})
        payload["reproductions"] = {}
        # A status cannot invent a receipt/option; fail rather than materialize it.
        with self.assertRaises(ValueError):
            jev.report(payload, evaluate)

    def test_portfolio_reuses_reviewed_items_and_excludes_same_lineage_pairs(self):
        first = jev.report(self.payload(), lambda s, qs: response(qs))
        second = copy.deepcopy(first)
        second["opportunities"][0].update(candidateIds=["other"], sessionEvidence=[
            {"ref": "r-other", "quote": "Need export elsewhere"}])
        meta = {"r0": {"session": "ses_a", "lineage": "family"},
                "r-other": {"session": "ses_b", "lineage": "family"}}
        calls = []

        def evaluate(state, qs):
            calls.append(state)
            return response(qs, {q: "canix" for q in qs})

        result = jev.portfolio({"canix": first, "SynDB": second}, meta, evaluate)
        self.assertFalse(result["shared"])
        self.assertFalse(calls)
        self.assertEqual(result["coverage"]["sameLineagePairsExcluded"], 1)
        meta["r-other"]["lineage"] = "independent"
        result = jev.portfolio({"canix": first, "SynDB": second}, meta, evaluate)
        self.assertEqual(result["shared"][0]["owner"], "canix")
        self.assertEqual(result["shared"][0]["candidateIds"], ["c0", "other"])
        self.assertEqual(set(calls[0]["items"]), {"c0", "other"})
        self.assertEqual(calls[0]["pairs"]["pair0"]["left"], "c0")


if __name__ == "__main__":
    unittest.main()
