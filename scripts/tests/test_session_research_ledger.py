import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from session_research_ledger import build_ledger, candidate_id, review_context, validate_candidate_view


class CandidateLedgerTest(unittest.TestCase):
    def setUp(self):
        self.record = {"ref": "user", "text": "Use nix-rust inside nix-cache-pin", "role": "user"}
        self.bundles = [{"session": "session", "chunks": [{"records": [
            self.record, {"ref": "later", "text": "Do not create a new framework", "role": "user"}]}]}]
        self.finding = {"session": "session", "kind": "feature", "claim": "Add locking",
                        "citations": [{"ref": "user", "quote": self.record["text"]}]}
        self.findings = {"canix": [self.finding]}
        self.ident = candidate_id(self.finding)

    def test_cwd_does_not_assign_owner_and_later_decision_is_retained(self):
        result = build_ledger(self.findings, self.bundles)
        self.assertIsNone(result["candidates"][0]["targetRepository"])
        self.assertEqual(result["conversations"][0]["records"][-1]["ref"], "later")

    def test_owner_can_be_outside_sampled_repositories(self):
        result = build_ledger(self.findings, self.bundles, {self.ident: {
            "targetRepository": "nix-cache-pin", "targetComponent": "mutation locking",
            "ownershipEvidence": self.finding["citations"], "intent": "requested",
            "disposition": "selected", "rationale": "Explicit repository in request"}})
        self.assertEqual(result["candidates"][0]["targetRepository"], "nix-cache-pin")
        self.assertEqual(result["candidates"][0]["sourceCwdHint"], "canix")

    def test_delivered_code_cannot_be_reopened_as_missing(self):
        with self.assertRaisesRegex(ValueError, "contradicts"):
            build_ledger(self.findings, self.bundles, {self.ident: {
                "implementation": "present", "action": "fix", "rationale": "Already shipped"}})

    def test_no_top_eight_truncation(self):
        rows = [{**self.finding, "claim": f"Request {i}"} for i in range(15)]
        result = build_ledger({"canix": rows}, self.bundles)
        self.assertEqual(len(result["candidates"]), 15)
        self.assertEqual(len({row["id"] for row in result["candidates"]}), 15)

    def test_review_context_routes_by_owner_and_retains_approval_context(self):
        self.bundles[0]["chunks"][0]["records"].insert(1, {
            "ref": "proposal", "role": "assistant", "text": "Use the existing lock primitive"})
        ledger = build_ledger(self.findings, self.bundles, {self.ident: {
            "targetRepository": "nix-cache-pin", "targetComponent": "mutation",
            "ownershipEvidence": self.finding["citations"], "intent": "accepted",
            "disposition": "selected", "rationale": "User approved existing primitive"}})
        self.assertFalse(review_context(ledger, "canix")["candidates"])
        context = review_context(ledger, "nix-cache-pin")
        self.assertEqual([r["ref"] for r in context["conversations"][0]["records"]],
                         ["user", "proposal", "later"])
        report = {"opportunities": [{"candidateIds": [self.ident], "action": "investigate",
                                    "implementation": "unknown", "deployment": "unknown"}]}
        validate_candidate_view(report, context["candidates"])
        report["opportunities"][0]["action"] = "fix"
        with self.assertRaisesRegex(ValueError, "contradicts"):
            validate_candidate_view(report, context["candidates"])

    def test_rejected_candidates_cannot_enter_selected_view(self):
        with self.assertRaisesRegex(ValueError, "accepted intent"):
            build_ledger(self.findings, self.bundles, {self.ident: {
                "targetRepository": "canix", "targetComponent": "locking",
                "ownershipEvidence": self.finding["citations"], "intent": "rejected",
                "disposition": "selected", "rationale": "Do not implement"}})

    def test_duplicate_cycles_and_forged_ownership_are_rejected(self):
        other = {**self.finding, "claim": "Other"}
        other_id = candidate_id(other)
        with self.assertRaisesRegex(ValueError, "cyclic"):
            build_ledger({"canix": [self.finding, other]}, self.bundles, {
                self.ident: {"disposition": "duplicate", "duplicateOf": other_id, "rationale": "same"},
                other_id: {"disposition": "duplicate", "duplicateOf": self.ident, "rationale": "same"}})
        with self.assertRaisesRegex(ValueError, "ownership evidence"):
            build_ledger(self.findings, self.bundles, {self.ident: {
                "targetRepository": "elsewhere", "targetComponent": "thing",
                "ownershipEvidence": [{"ref": "user", "quote": "never said"}],
                "rationale": "Incorrect attribution"}})


if __name__ == "__main__":
    unittest.main()
