"""Behavioral tests for the frozen-session product research pilot."""

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import session_research as research


class ResearchPilotTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.archive = self.root / "archive"
        (self.archive / "snapshots").mkdir(parents=True)
        self.db = self.archive / "snapshots" / "primary.db"
        db = sqlite3.connect(self.db)
        db.executescript("""
            create table session (id text primary key, title text, directory text);
            create table message (id text primary key, session_id text, time_created integer, data text);
            create table part (id text primary key, message_id text, session_id text,
                               time_created integer, data text);
        """)
        rows = [
            ("ses_a", "Add export to pink-raven", "text", "Please add an export command"),
            ("ses_b", "Fix regression in canix", "text", "It fails on startup"),
            ("ses_c", "Unfinished indexing in SynDB", "text", "Indexing remains unfinished"),
        ]
        for i, (session, title, kind, text) in enumerate(rows):
            repo = {"ses_a": "pink-raven", "ses_b": "canix", "ses_c": "SynDB"}[session]
            db.execute("insert into session values (?,?,?)", (session, title, f"/old/{repo}"))
            db.execute("insert into message values (?,?,?,?)",
                       (f"msg_{i}", session, i, json.dumps({"role": "user"})))
            db.execute("insert into part values (?,?,?,?,?)",
                       (f"prt_{i}", f"msg_{i}", session, i,
                        json.dumps({"type": kind, "text": text})))
        db.execute("insert into message values (?,?,?,?)",
                   ("msg_summary", "ses_a", 10, json.dumps({"role": "assistant", "summary": True})))
        db.execute("insert into part values (?,?,?,?,?)",
                   ("prt_summary", "msg_summary", "ses_a", 10,
                    json.dumps({"type": "text", "text": "fake repeated request"})))
        db.execute("insert into message values (?,?,?,?)",
                   ("msg_tool", "ses_a", 11, json.dumps({"role": "assistant"})))
        db.execute("insert into part values (?,?,?,?,?)",
                   ("prt_tool", "msg_tool", "ses_a", 11,
                    json.dumps({"type": "tool", "state": {"status": "completed", "output": "secret"}})))
        db.commit()
        db.close()
        self.digest = hashlib.sha256(self.db.read_bytes()).hexdigest()
        (self.archive / "manifest.json").write_text(json.dumps({"sources": [
            {"name": "primary", "destination": str(self.db), "bytes": self.db.stat().st_size,
             "sha256": self.digest}]}))
        (self.archive / "reconciliation.json").write_text(json.dumps({"sessions": [
            {"id": id, "source": "primary", "directory": f"/old/{repo}",
             "parentID": None, "messages": 2, "timeUpdated": 100 + i, "variants": []}
            for i, (id, repo) in enumerate([("ses_a", "pink-raven"),
                                            ("ses_b", "canix"), ("ses_c", "SynDB")])
        ]}))

    def test_prepare_reads_pinned_canonical_roots_and_exports_original_ids(self):
        work = self.root / "work"
        plan = research.prepare(self.archive, work, per_repo=1)
        self.assertEqual({x["repo"] for x in plan["threads"]},
                         {"pink-raven", "canix", "SynDB"})
        self.assertEqual(plan["sourceSha256"], self.digest)
        bundles = research.export(work, max_chars=15, chunk_chars=160)
        a = next(x for x in bundles if x["session"] == "ses_a")
        self.assertEqual(a["excludedDerived"], 1)
        self.assertEqual(a["excludedTool"], 1)
        records = [r for chunk in a["chunks"] for r in chunk["records"]]
        self.assertEqual("".join(r["text"] for r in records), "Please add an export command")
        self.assertTrue(all("prt_0" in r["ref"] for r in records))
        self.assertFalse(any("secret" in json.dumps(x) for x in bundles))
        with self.assertRaises(FileExistsError):
            research.prepare(self.archive, work, per_repo=1)

    def test_changed_source_is_refused_before_export(self):
        work = self.root / "work"
        research.prepare(self.archive, work, per_repo=1)
        db = sqlite3.connect(self.db)
        db.execute("insert into session values ('extra','New','/old/canix')")
        db.commit()
        db.close()
        with self.assertRaisesRegex(ValueError, "pin"):
            research.export(work)

    def test_no_eligible_text_cannot_be_reported_as_analyzed(self):
        db = sqlite3.connect(self.db)
        db.execute("update part set data=? where session_id='ses_b'",
                   (json.dumps({"type": "tool", "state": {"status": "completed"}}),))
        db.commit()
        db.close()
        self._repin_fixture()
        work = self.root / "empty-session"
        with self.assertRaisesRegex(ValueError, "not an eligible canonical root"):
            research.prepare(self.archive, work, session_ids=["ses_b"])

    def test_null_part_metadata_does_not_discard_verbatim_user_text(self):
        db = sqlite3.connect(self.db)
        db.execute("update part set data=? where id='prt_0'",
                   (json.dumps({"type": "text", "metadata": None,
                                "text": "Please add an export command"}),))
        db.commit()
        db.close()
        self._repin_fixture()
        work = self.root / "null-metadata"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        bundles = research.export(work)
        self.assertIn("Please add", bundles[0]["chunks"][0]["records"][0]["text"])

    def test_export_carries_source_role_and_chronology_and_verifies_them(self):
        work = self.root / "chronology"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        bundle = research.export(work)[0]
        record = bundle["chunks"][0]["records"][0]
        self.assertEqual((record["role"], record["timeCreated"], record["partTimeCreated"]),
                         ("user", 0, 0))
        self.assertEqual(research.verify_sources(work), 1)
        bundles = research.load(work / "bundles.json")
        bundles[0]["chunks"][0]["records"][0]["role"] = "assistant"
        (work / "bundles.json").write_text(json.dumps(bundles))
        with self.assertRaisesRegex(ValueError, "source part"):
            research.verify_sources(work)

    def test_long_root_and_forked_duplicate_are_one_sampled_lineage(self):
        db = sqlite3.connect(self.db)
        db.execute("insert into session values ('ses_long','Investigate archive throughput','/old/SynDB')")
        db.execute("insert into session values ('ses_fork','Investigate archive throughput (fork #1)','/old/SynDB')")
        for i, sid in enumerate(('ses_long', 'ses_fork')):
            db.execute("insert into message values (?,?,?,?)",
                       (f'msg_long{i}', sid, 100 + i, json.dumps({'role': 'user'})))
            db.execute("insert into part values (?,?,?,?,?)",
                       (f'prt_long{i}', f'msg_long{i}', sid, 100 + i,
                        json.dumps({'type': 'text', 'text': 'Why is archive throughput slow?'})))
        db.commit()
        db.close()
        self._repin_fixture()
        recon = json.loads((self.archive / 'reconciliation.json').read_text())
        recon['sessions'].extend([
            {'id': 'ses_long', 'source': 'primary', 'directory': '/old/SynDB',
             'parentID': None, 'messages': 245, 'timeUpdated': 500, 'variants': []},
            {'id': 'ses_fork', 'source': 'primary', 'directory': '/old/SynDB',
             'parentID': None, 'messages': 230, 'timeUpdated': 501, 'variants': []},
        ])
        (self.archive / 'reconciliation.json').write_text(json.dumps(recon))
        plan = research.prepare(self.archive, self.root / 'long', session_ids=['ses_long'])
        self.assertEqual(plan['threads'][0]['messageCount'], 245)
        self.assertEqual(plan['threads'][0]['lineageCount'], 2)
        research.export(self.root / 'long')
        self.assertEqual(research.metrics(self.root / 'long')['replayedRoots'], 1)
        with self.assertRaisesRegex(ValueError, 'same lineage'):
            research.prepare(self.archive, self.root / 'fork',
                             session_ids=['ses_long', 'ses_fork'])

    def test_synthetic_user_part_does_not_identify_unrelated_fork_lineages(self):
        db = sqlite3.connect(self.db)
        for i, text in enumerate(('Implement archive export', 'Implement archive import')):
            sid = f'ses_variant{i}'
            db.execute("insert into session values (?,?,?)",
                       (sid, 'Implement archive sync' + (' (fork #1)' if i else ''), '/old/SynDB'))
            db.execute("insert into message values (?,?,?,?)",
                       (f'msg_variant{i}', sid, 100 + i, json.dumps({'role': 'user'})))
            db.execute("insert into part values (?,?,?,?,?)",
                       (f'prt_synthetic{i}', f'msg_variant{i}', sid, 100 + i,
                        json.dumps({'type': 'text', 'synthetic': True,
                                    'text': 'Repeated synthetic tool transcript'})))
            db.execute("insert into part values (?,?,?,?,?)",
                       (f'prt_real{i}', f'msg_variant{i}', sid, 101 + i,
                        json.dumps({'type': 'text', 'text': text})))
        db.commit()
        db.close()
        self._repin_fixture()
        recon = json.loads((self.archive / 'reconciliation.json').read_text())
        recon['sessions'].extend([
            {'id': f'ses_variant{i}', 'source': 'primary', 'directory': '/old/SynDB',
             'parentID': None, 'messages': 2, 'timeUpdated': 100 + i, 'variants': []}
            for i in range(2)])
        (self.archive / 'reconciliation.json').write_text(json.dumps(recon))
        plan = research.prepare(self.archive, self.root / 'variants',
                                session_ids=['ses_variant0', 'ses_variant1'])
        self.assertEqual([entry['lineageCount'] for entry in plan['threads']], [1, 1])

    def _repin_fixture(self):
        manifest = json.loads((self.archive / "manifest.json").read_text())
        manifest["sources"][0]["bytes"] = self.db.stat().st_size
        manifest["sources"][0]["sha256"] = hashlib.sha256(self.db.read_bytes()).hexdigest()
        (self.archive / "manifest.json").write_text(json.dumps(manifest))

    def test_operator_can_curate_only_canonical_root_sessions(self):
        work = self.root / "curated"
        plan = research.prepare(self.archive, work, per_repo=1, session_ids=["ses_c", "ses_a"])
        self.assertEqual([row["session"] for row in plan["threads"]], ["ses_c", "ses_a"])
        with self.assertRaisesRegex(ValueError, "not an eligible canonical root"):
            research.prepare(self.archive, self.root / "invalid", session_ids=["unknown"])

    def test_sampler_treats_build_errors_as_friction_and_skips_generated_titles(self):
        self.assertEqual(research.stratum("Nix build missing plugins/zenodo/Cargo.toml"), "friction")
        self.assertFalse(research.informative_title('{"verdict":"fail"}'))
        self.assertFalse(research.informative_title("New session - 2026-09-10"))
        self.assertTrue(research.informative_title("Add inner padding to search cards"))

    def test_invented_or_unquoted_citations_are_refused(self):
        work = self.root / "work"
        research.prepare(self.archive, work, per_repo=1)
        bundles = research.export(work)
        a = next(x for x in bundles if x["session"] == "ses_a")
        chunk = a["chunks"][0]
        good = {"brief": "Export was requested", "signals": [{
            "kind": "feature", "claim": "User requested an export command",
            "citations": [{"ref": chunk["records"][0]["ref"], "quote": "add an export"}]}]}
        self.assertEqual(research.validate_signals(good, chunk)["signals"][0]["kind"], "feature")
        bad = json.loads(json.dumps(good))
        bad["signals"][0]["citations"][0]["ref"] = "prt_invented"
        with self.assertRaisesRegex(ValueError, "citation"):
            research.validate_signals(bad, chunk)

    def test_verbatim_short_user_approval_is_citable_but_fragment_is_not(self):
        record = {"ref": "user-short", "role": "user", "text": "commit"}
        chunk = {"records": [record]}
        good = {"brief": "Approval", "signals": [{"kind": "decision", "claim": "User approved",
            "citations": [{"ref": "user-short", "quote": "commit"}]}]}
        self.assertEqual(research.validate_signals(good, chunk), good)
        chunk["records"][0]["text"] = "Please commit this change"
        with self.assertRaisesRegex(ValueError, "citation"):
            research.validate_signals(good, chunk)
        bad = json.loads(json.dumps(good))
        bad["signals"][0]["citations"][0]["quote"] = "a fictional quote"
        with self.assertRaisesRegex(ValueError, "citation"):
            research.validate_signals(bad, chunk)

    def test_report_requires_complete_extract_and_refuses_invented_source(self):
        work = self.root / "work"
        research.prepare(self.archive, work, per_repo=1)
        research.export(work)
        with self.assertRaisesRegex(ValueError, "missing"):
            research.collect(work)
        with self.assertRaisesRegex(ValueError, "missing"):
            research.build_briefs(work)

    def test_session_brief_retains_ordered_spark_chunk_briefs(self):
        work = self.root / "work"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        bundles = research.export(work, max_chars=15, chunk_chars=16)
        signal_dir = work / "signals"
        signal_dir.mkdir()
        for i, chunk in enumerate(bundles[0]["chunks"]):
            research.write_new(signal_dir / f"{chunk['id']}.json",
                               {"brief": f"Part {i + 1}", "signals": []})
        briefs = research.build_briefs(work)
        self.assertEqual(briefs[0]["session"], "ses_a")
        self.assertEqual(briefs[0]["briefs"], ["Part 1", "Part 2"])
        self.assertEqual(briefs[0]["chunkCount"], 2)

    def test_signal_timeline_uses_original_role_and_time_not_model_prose(self):
        work = self.root / "timeline"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        bundle = research.export(work)[0]
        chunk = bundle["chunks"][0]
        (work / "signals").mkdir()
        research.write_new(work / "signals" / f"{chunk['id']}.json", {
            "brief": "Claim by user", "signals": [{"kind": "feature", "claim": "Add export",
                "citations": [{"ref": chunk["records"][0]["ref"], "quote": "add an export"}]}]})
        signal = research.collect(work)["pink-raven"][0]
        self.assertEqual(signal["evidenceRoles"], ["user"])
        self.assertEqual(signal["timeCreated"], 0)
        self.assertEqual(research.build_briefs(work)[0]["timeline"][0]["citations"],
                         signal["citations"])

    def test_opportunity_audit_flags_assistant_only_and_repeated_lineage(self):
        opportunity = {"title": "Maybe add export", "currentStatus": "unverified",
                       "sessionEvidence": [{"ref": "r1", "quote": "assistant plan"},
                                           {"ref": "r2", "quote": "same plan again"}]}
        meta = {"r1": {"role": "assistant", "session": "ses_one", "lineage": "family"},
                "r2": {"role": "assistant", "session": "ses_fork", "lineage": "family"}}
        audit = research.audit_opportunity(opportunity, meta)
        self.assertEqual(audit["independentLineages"], 1)
        self.assertTrue(audit["assistantOnly"])
        self.assertTrue(audit["repeatLineage"])
        meta["r2"]["role"] = "user"
        self.assertFalse(research.audit_opportunity(opportunity, meta)["assistantOnly"])

    def test_pasted_tool_transcript_in_user_role_is_flagged_as_ambiguous(self):
        bundles = [{"repoCwdHint": "canix", "session": "ses_a", "lineage": "family",
                    "chunks": [{"records": [
                        {"ref": "p/s/m/part@0:28", "role": "user", "timeCreated": 12,
                         "text": "Called the Read tool with the following input:"},
                        {"ref": "p/s/m/part@28:56", "role": "user", "timeCreated": 12,
                         "text": "<path>/repo/flake.nix</path><content>..."},
                        {"ref": "p/s/m/normal@0:22", "role": "user", "timeCreated": 20,
                         "text": "Please check the flake"}]}]}]
        metadata = research.source_meta(bundles)
        self.assertTrue(metadata["p/s/m/part@28:56"]["embeddedToolText"])
        self.assertFalse(metadata["p/s/m/normal@0:22"]["embeddedToolText"])
        result = research.audit_opportunity({"title": "Candidate", "sessionEvidence": [
            {"ref": "p/s/m/part@28:56", "quote": "<path>/repo"}]}, metadata)
        self.assertTrue(result["embeddedToolText"])

    def test_pasted_skill_template_is_not_inferred_as_independent_user_request(self):
        bundles = [{"repoCwdHint": "SynDB", "session": "ses_a", "lineage": "one",
                    "chunks": [{"records": [
                        {"ref": "p/s/m/template@0:2500", "role": "user", "timeCreated": 12,
                         "text": "# Scientific Brainstorming\n## Purpose and boundaries\nUse this skill..."},
                        {"ref": "p/s/m/template@2500:3000", "role": "user", "timeCreated": 12,
                         "text": "Further instructions..."},
                        {"ref": "p/s/m/request@0:30", "role": "user", "timeCreated": 20,
                         "text": "Please investigate my manuscript"},
                        {"ref": "p/s/m/skill@0:85", "role": "user", "timeCreated": 21,
                         "text": "# Grouped Git Commits\nThis skill is the canonical reference..."}]}]}]
        metadata = research.source_meta(bundles)
        self.assertTrue(metadata["p/s/m/template@2500:3000"]["pastedTemplateText"])
        self.assertTrue(metadata["p/s/m/skill@0:85"]["pastedTemplateText"])
        self.assertFalse(metadata["p/s/m/request@0:30"]["pastedTemplateText"])
        audit = research.audit_opportunity({"title": "Maybe", "sessionEvidence": [
            {"ref": "p/s/m/template@0:2500", "quote": "Purpose and boundaries"}]}, metadata)
        self.assertTrue(audit["pastedTemplateText"])

    def test_report_citations_retain_role_and_time_including_portfolio(self):
        citation = {"ref": "native@0:13", "quote": "exact excerpt"}
        meta = {"role": "user", "timeCreated": 1781023885618,
                "embeddedToolText": False, "pastedTemplateText": False}
        line = research.format_historical_citation(citation, meta)
        self.assertIn("user at 2026-06-09T", line)
        self.assertIn("`native@0:13`", line)

    def test_source_windows_keep_cited_context_without_repeating_entire_part(self):
        text = "A" * 1100 + "THE CITED NEED" + "B" * 1100
        findings = [{"citations": [{"ref": "original-part", "quote": "THE CITED NEED"}]}]
        result = research.source_windows(findings, {"original-part": text}, margin=80)
        self.assertEqual(len(result["original-part"]), 1)
        self.assertIn("THE CITED NEED", result["original-part"][0])
        self.assertIn(result["original-part"][0], text)
        self.assertLess(len(result["original-part"][0]), 200)

    def test_report_selection_keeps_all_direct_user_decisions_and_bounded_assistant_context(self):
        findings = [{"session": "ses_long", "kind": "feature", "claim": f"Goal {i}",
                     "timeCreated": i, "citations": [{"ref": f"r{i}", "quote": f"Goal {i}"}]}
                    for i in range(20)]
        findings += [{"session": "ses_long", "kind": "outcome", "claim": "Reported completion",
                      "timeCreated": 21, "citations": [{"ref": "assistant", "quote": "reported done"}]}]
        findings += [{"session": "ses_long", "kind": "decision", "claim": "Do not build this",
                      "timeCreated": 22, "citations": [{"ref": "decision", "quote": "Do not build this"}]}]
        meta = {f"r{i}": {"role": "user", "pastedTemplateText": False,
                           "embeddedToolText": False} for i in range(20)}
        meta["assistant"] = {"role": "assistant", "pastedTemplateText": False,
                              "embeddedToolText": False}
        meta["decision"] = {"role": "user", "pastedTemplateText": False,
                             "embeddedToolText": False}
        chosen = research.select_report_findings(findings, meta, ["ses_long"], per_session=8)
        self.assertEqual(len(chosen), 22)
        self.assertEqual({x["claim"] for x in chosen}, {x["claim"] for x in findings})
        self.assertEqual(chosen[-1]["claim"], "Do not build this")

    def test_report_rechecks_original_quotes_and_current_status(self):
        work = self.root / "work"
        research.prepare(self.archive, work, per_repo=1)
        bundles = research.export(work)
        a = next(x for x in bundles if x["session"] == "ses_a")
        citation = {"ref": a["chunks"][0]["records"][0]["ref"],
                    "quote": "add an export"}
        report = {"summary": "Pilot", "opportunities": [{
            "title": "Export", "kind": "feature", "problem": "Need exports",
            "proposal": "Add export", "firstSlice": "Export one kind",
            "validation": "A saved file", "priority": "medium",
            "currentStatus": "unverified", "repoEvidence": [],
            "sessionEvidence": [citation]}], "limitations": ["Older snapshot"]}
        available = {citation["ref"]: a["chunks"][0]["records"][0]["text"]}
        self.assertEqual(research.validate_report(report, available, {}), report)
        report["opportunities"][0]["currentStatus"] = "present"
        with self.assertRaisesRegex(ValueError, "repository evidence"):
            research.validate_report(report, available, {})
        report["opportunities"][0]["currentStatus"] = "confirmed-gap"
        with self.assertRaisesRegex(ValueError, "repository evidence"):
            research.validate_report(report, available, {})
        report["opportunities"][0]["repoEvidence"] = [
            {"ref": "current:file#L1", "quote": "confirmed broken behavior"}]
        self.assertEqual(research.validate_report(report, available,
                         {"current:file#L1": "confirmed broken behavior"}), report)
        report["opportunities"][0]["currentStatus"] = "unverified"
        report["opportunities"][0]["sessionEvidence"][0]["quote"] = "not in source"
        with self.assertRaisesRegex(ValueError, "source evidence"):
            research.validate_report(report, available, {})

    def test_model_event_reader_never_uses_tool_text_as_an_answer(self):
        event = lambda kind, text: json.dumps({"type": kind, "part": {"type": kind, "text": text}})
        raw = event("text", '{"brief":"Done","signals":[]}')
        self.assertEqual(research.parse_events(raw), {"brief": "Done", "signals": []})
        with self.assertRaisesRegex(ValueError, "tool event"):
            research.parse_events(event("tool", '{"wrong":true}') + '\n' + raw)
        with self.assertRaisesRegex(ValueError, "tool event"):
            research.parse_events(event("tool", '{"brief":"false"}'))

    def test_repo_context_uses_pinned_head_and_line_ranges(self):
        repo = self.root / "repo"
        repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / "notes.md").write_text("first\nshipped: added support\nthird\n")
        subprocess.run(["git", "-C", str(repo), "add", "notes.md"], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false",
                        "commit", "-qm", "fixture"], check=True)
        (repo / "notes.md").write_text("changed after commit")
        context = research.repo_context(repo, ["notes.md#L2-L2"])
        self.assertEqual(list(context.values()), ["shipped: added support"])
        self.assertIn(":notes.md#L2", next(iter(context)))
        with self.assertRaisesRegex(ValueError, "relative tracked"):
            research.repo_context(repo, ["../other.md#L1-L2"])

    def test_repo_context_refuses_oversized_tracked_file(self):
        repo = self.root / "large-repo"
        repo.mkdir()
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / "huge.md").write_text("x" * (300 * 1024))
        subprocess.run(["git", "-C", str(repo), "add", "huge.md"], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false",
                        "commit", "-qm", "fixture"], check=True)
        with self.assertRaisesRegex(ValueError, "too large"):
            research.repo_context(repo, ["huge.md#L1-L1"])

    def test_model_runner_refuses_launchers_that_ignore_disposable_db(self):
        work = self.root / "model"
        work.mkdir(mode=0o700)
        research.write_new(work / "plan.json", {"version": 1})
        wrapper = work / "opencode"
        wrapper.write_text("#!/bin/sh\necho /home/user/live-opencode.db\n")
        wrapper.chmod(0o700)
        previous = os.environ.get("SESSION_RESEARCH_OPENCODE_BIN")
        os.environ["SESSION_RESEARCH_OPENCODE_BIN"] = str(wrapper)
        try:
            with self.assertRaisesRegex(ValueError, "isolation"):
                research.model_call(work, "mock-model", "Analyze", {"text": "fixture"}, "try")
        finally:
            if previous is None:
                os.environ.pop("SESSION_RESEARCH_OPENCODE_BIN", None)
            else:
                os.environ["SESSION_RESEARCH_OPENCODE_BIN"] = previous
        self.assertFalse((work / "prompts" / "try.events.jsonl").exists())

    def test_model_runner_refuses_unbounded_prompt_before_inference(self):
        work = self.root / "large-prompt"
        work.mkdir(mode=0o700)
        research.write_new(work / "plan.json", {"version": 1})
        with self.assertRaisesRegex(ValueError, "prompt exceeds"):
            research.model_call(work, "mock-model", "Analyze", {"text": "x" * 210000}, "large")
        self.assertFalse((work / "prompts" / "large.json").exists())

    def test_model_runner_requires_prepared_private_workdir(self):
        work = self.root / "public-workdir"
        work.mkdir(mode=0o755)
        research.write_new(work / "plan.json", {"version": 1})
        with self.assertRaisesRegex(ValueError, "private work"):
            research.model_call(work, "mock-model", "Analyze", {"text": "fixture"}, "public")
        self.assertFalse((work / "prompts").exists())

    def test_model_config_denies_all_tools_even_mcp_and_preserves_prior_config(self):
        work = self.root / "config"
        work.mkdir(mode=0o700)
        old = research.legacy_opencode_config()
        research.write_new(work / "opencode.json", old)
        new = research.ensure_model_config(work)
        self.assertEqual(new["agents"]["session-research"]["permissions"],
                         [{"action": "*", "resource": "*", "effect": "deny"}])
        self.assertEqual(research.load(work / "opencode.previous.json"), old)
        self.assertEqual(research.ensure_model_config(work), new)
        (work / "opencode.json").write_text('{"unknown":"user work"}')
        with self.assertRaisesRegex(ValueError, "permissions changed"):
            research.ensure_model_config(work)

    def test_model_config_pins_host_provider_config_in_private_merge(self):
        work = self.root / "config-merge"
        work.mkdir(mode=0o700)
        host = self.root / "host.json"
        host.write_text(json.dumps({"providers": {"muse-code": {"enabled": True}},
                                    "permissions": [{"action": "*", "resource": "*",
                                                     "effect": "allow"}]}))
        research.ensure_model_config(work)
        env = research.model_configuration(work, {"OPENCODE_CONFIG": str(host)})
        merged = research.load(env["OPENCODE_CONFIG"])
        self.assertEqual(merged["providers"]["muse-code"]["enabled"], True)
        self.assertEqual(merged["agents"]["session-research"]["permissions"][-1]["effect"],
                         "deny")
        self.assertEqual(research.model_configuration(work, {"OPENCODE_CONFIG": str(host)}), env)
        self.assertEqual(json.loads(host.read_text())["providers"]["muse-code"]["enabled"], True)

    def test_auth_seed_copies_only_needed_credentials_and_catalog(self):
        source, target = self.root / "source.db", self.root / "target.db"
        for file in (source, target):
            db = sqlite3.connect(file)
            db.executescript("""
                create table credential(id text primary key, integration_id text, label text,
                  value text, connector_id text, method_id text, active integer,
                  time_created integer, time_updated integer);
                create table kv(key text primary key, value text, time_created integer,
                  time_updated integer);
            """)
            db.close()
        db = sqlite3.connect(source)
        for provider in ("muse-code", "openai", "opencode-omniroute"):
            db.execute("insert into credential values (?, ?, '', 'fixture secret', null, null, 1, 1, 1)",
                       (provider, provider))
        db.execute("insert into kv values ('models-dev:catalog','fixture catalog',1,1)")
        db.execute("insert into kv values ('unrelated','ignore this',1,1)")
        db.commit()
        db.close()
        research.seed_auth(source, target)
        db = sqlite3.connect(target)
        self.assertEqual([x[0] for x in db.execute('select integration_id from credential order by id')],
                         ['muse-code', 'openai'])
        self.assertEqual(db.execute('select key from kv').fetchone()[0], 'models-dev:catalog')
        db.close()
        with self.assertRaisesRegex(ValueError, "already seeded"):
            research.seed_auth(source, target)

    def test_final_proof_checks_citations_against_original_part_not_only_bundle(self):
        work = self.root / "work"
        research.prepare(self.archive, work, per_repo=1)
        research.export(work)
        self.assertEqual(research.verify_sources(work), 3)
        bundles = research.load(work / "bundles.json")
        bundles[0]["chunks"][0]["records"][0]["text"] = "forged source"
        (work / "bundles.json").write_text(json.dumps(bundles))
        with self.assertRaisesRegex(ValueError, "source part"):
            research.verify_sources(work)
        bundles[0]["chunks"][0]["records"][0]["ref"] = (
            "primary/ses_a/msg_summary/prt_summary@0:21")
        bundles[0]["chunks"][0]["records"][0]["text"] = "fake repeated request"
        (work / "bundles.json").write_text(json.dumps(bundles))
        with self.assertRaisesRegex(ValueError, "source part"):
            research.verify_sources(work)

    def test_tampered_export_is_refused_before_sending_to_models(self):
        work = self.root / "before-model"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        research.export(work)
        bundles = research.load(work / "bundles.json")
        bundles[0]["chunks"][0]["records"][0]["text"] = "forged user request"
        (work / "bundles.json").write_text(json.dumps(bundles))
        with self.assertRaisesRegex(ValueError, "source part"):
            research.run_extract(work, privacy_reviewed=True)
        with self.assertRaisesRegex(ValueError, "source part"):
            research.synthesize(work, privacy_reviewed=True)
        self.assertFalse((work / "prompts").exists())

    def test_portfolio_needs_history_from_two_different_repositories(self):
        shared = {"summary": "shared", "shared": [{"title": "Retries", "owner": "canix",
            "problem": "Repeated errors", "firstSlice": "Add one retry",
            "sessionEvidence": [{"ref": "ref_a", "quote": "failed again"}]}]}
        texts = {"ref_a": "failed again", "ref_b": "failed again elsewhere"}
        repos = {"ref_a": "canix", "ref_b": "SynDB"}
        with self.assertRaisesRegex(ValueError, "two repositories"):
            research.validate_portfolio(shared, texts, repos)
        shared["shared"][0]["sessionEvidence"].append({"ref": "ref_b", "quote": "failed again elsewhere"})
        self.assertEqual(research.validate_portfolio(shared, texts, repos), shared)

    def test_metrics_do_not_label_partial_extraction_as_complete(self):
        work = self.root / "metrics"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        bundles = research.export(work, max_chars=15, chunk_chars=16)
        status = research.metrics(work)
        self.assertEqual(status["selectedSessions"], 1)
        self.assertEqual(status["sourceSlices"], 2)
        self.assertEqual(status["extractedChunks"], 0)
        self.assertEqual(status["missingChunks"], 2)

    def test_inspect_retrieves_verified_source_neighborhood(self):
        work = self.root / "inspect"
        research.prepare(self.archive, work, session_ids=["ses_a"])
        bundle = research.export(work, max_chars=15, chunk_chars=16)[0]
        ref = bundle["chunks"][0]["records"][0]["ref"]
        view = research.inspect(work, ref, radius=1)
        self.assertEqual(view["session"], "ses_a")
        self.assertEqual(len(view["records"]), 2)
        self.assertEqual("".join(r["text"] for r in view["records"]),
                         "Please add an export command")
        with self.assertRaisesRegex(ValueError, "not exported"):
            research.inspect(work, "invented", radius=1)


if __name__ == "__main__":
    unittest.main()
