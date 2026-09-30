#!/usr/bin/env python3
"""Read-only, cited product-research pilot over pinned OpenCode v1 snapshots.

Only generated artifacts are written, into a new private work directory. The
source snapshot and its reconciliation are never modified or installed.
"""

import argparse
from contextlib import closing
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import subprocess

import session_research_jev as jev

REPOS = ("canix", "SynDB", "pink-raven")
MAX_SESSION_MESSAGES = 500
MAX_SESSION_TEXT_CHARS = 120000
STRATA = {
    "friction": ("fix ", "fail", "error", "broken", "missing", "debug", "troubleshoot"),
    "unfinished": ("unfinished", "remaining", "todo", "gap", "resume", "continue"),
    "feature": ("feature", "add ", "implement", "support", "build ", "create "),
}


def digest(path):
    hash_ = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(4 * 1024 * 1024), b""):
            hash_.update(block)
    return hash_.hexdigest()


def write_new(path, value):
    path = Path(path)
    data = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "wb") as file:
            file.write(data)
            file.flush()
            os.fsync(file.fileno())
        parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def load(path):
    return json.loads(Path(path).read_text())


def pinned_snapshot(archive, source):
    entry = next((s for s in load(archive / "manifest.json")["sources"]
                  if s["name"] == source), None)
    if entry is None:
        raise ValueError(f"source {source} absent from archive manifest")
    path = (archive / "snapshots" / f"{source}.db").resolve(strict=True)
    if Path(entry["destination"]).resolve(strict=True) != path:
        raise ValueError("snapshot destination is not the frozen archive path")
    if Path(str(path) + "-wal").exists() and Path(str(path) + "-wal").stat().st_size:
        raise ValueError("nonempty snapshot WAL: cannot use immutable read-only mode")
    if path.stat().st_size != entry["bytes"] or digest(path) != entry["sha256"]:
        raise ValueError("snapshot pin mismatch")
    return path, entry["sha256"]


def open_snapshot(path):
    db = sqlite3.connect(Path(path).as_uri() + "?mode=ro&immutable=1", uri=True)
    db.row_factory = sqlite3.Row
    db.execute("PRAGMA query_only = ON")
    return db


def directory_repo(directory):
    # A cwd is a sampling hint, not proof that every finding belongs to it.
    return next((repo for repo in REPOS if Path(directory).name.lower() == repo.lower()), None)


def stratum(title):
    title = title.lower()
    return next((name for name, patterns in STRATA.items()
                 if any(pattern in title for pattern in patterns)), "control")


def informative_title(title):
    title = title.strip()
    return (0 < len(title) <= 110 and not title.lower().startswith(
        ("new session", "i’ll ", "i'll ", "i will ", "you are ", "{", '"')))


def lineage_key(title, first_user_text):
    """Conservatively collapse forks replaying the same request, not related topics."""
    name = re.sub(r"\s*\(fork\s*#\d+\)\s*$", "", title, flags=re.I).strip().casefold()
    request = " ".join(first_user_text.split()).casefold()
    return hashlib.sha256(json.dumps([name, request[:10000]]).encode()).hexdigest()[:24]


def prepare(archive, work, per_repo=12, session_ids=None):
    archive, work = Path(archive).resolve(), Path(work).resolve()
    if work.exists():
        raise FileExistsError(work)
    snapshot, sha = pinned_snapshot(archive, "primary")
    reconciliation = archive / "reconciliation.json"
    if per_repo < 1:
        raise ValueError("per_repo must be positive")
    canonical = load(reconciliation)["sessions"]
    with closing(open_snapshot(snapshot)) as db:
        titles = {r["id"]: r["title"] or "" for r in db.execute("select id,title from session")}
        eligible_rows = []
        for item in canonical:
            if not re.fullmatch(r"ses_[A-Za-z0-9_]+", item.get("id", "")):
                raise ValueError("invalid native session id")
            repo = directory_repo(item.get("directory") or "")
            if (repo is None or item["source"] != "primary" or item.get("parentID") is not None
                    or not 2 <= item.get("messages", 0) <= MAX_SESSION_MESSAGES
                    or item["id"] not in titles or not informative_title(titles[item["id"]])):
                continue
            first = db.execute(
                "select json_extract(p.data,'$.text') from message m join part p on p.message_id=m.id "
                "where m.session_id=? and json_extract(m.data,'$.role')='user' "
                "and json_extract(p.data,'$.type')='text' "
                "and coalesce(json_extract(p.data,'$.synthetic'),0)=0 "
                "and coalesce(json_extract(p.data,'$.metadata.chaosboxMigrationDraft'),0)=0 "
                "order by m.time_created,m.id,p.time_created,p.id limit 1", (item["id"],)
            ).fetchone()
            if first is None or not first[0]:
                continue
            eligible_rows.append((item, repo, lineage_key(titles[item["id"]], first[0])))
    counts = {}
    for _, _, family in eligible_rows:
        counts[family] = counts.get(family, 0) + 1
    pool = {repo: {key: [] for key in (*STRATA, "control")} for repo in REPOS}
    eligible = {repo: 0 for repo in REPOS}
    for item, repo, family in eligible_rows:
        eligible[repo] += 1
        entry = {"repo": repo, "session": item["id"], "source": "primary",
                 "title": titles[item["id"]], "directory": item["directory"],
                 "timeUpdated": item.get("timeUpdated"), "messageCount": item["messages"],
                 "variants": item.get("variants", []), "stratum": stratum(titles[item["id"]]),
                 "lineage": family, "lineageCount": counts[family]}
        pool[repo][entry["stratum"]].append(entry)
    selected = []
    if session_ids is not None:
        lookup = {row["session"]: row for repo in REPOS for group in pool[repo].values()
                  for row in group}
        if len(set(session_ids)) != len(session_ids):
            raise ValueError("duplicate curated session id")
        for session in session_ids:
            if session not in lookup:
                raise ValueError(f"not an eligible canonical root: {session}")
            selected.append(lookup[session])
        if len({row["lineage"] for row in selected}) != len(selected):
            raise ValueError("selected sessions include the same lineage")
    else:
        seen = set()
        for repo in REPOS:
            for group in pool[repo].values():
                group.sort(key=lambda e: hashlib.sha256(e["session"].encode()).hexdigest())
            while len([e for e in selected if e["repo"] == repo]) < per_repo:
                remaining = per_repo - len([e for e in selected if e["repo"] == repo])
                chosen = False
                for category in ("feature", "unfinished", "friction", "control"):
                    if remaining == 0:
                        break
                    while pool[repo][category] and pool[repo][category][0]["lineage"] in seen:
                        pool[repo][category].pop(0)
                    if pool[repo][category]:
                        entry = pool[repo][category].pop(0)
                        selected.append(entry)
                        seen.add(entry["lineage"])
                        remaining -= 1
                        chosen = True
                if not chosen:
                    break
    if not selected:
        raise ValueError("no eligible canonical root sessions selected")
    work.mkdir(mode=0o700, parents=True)
    plan = {"version": 2, "archive": str(archive), "source": str(snapshot),
            "sourceSha256": sha, "reconciliationSha256": digest(reconciliation),
            "eligibleRootsByCwd": eligible, "threads": selected,
            "coverage": ("Canonical primary roots only; cwd is a sampling hint; "
                         "children and later deltas are excluded.")}
    write_new(work / "plan.json", plan)
    return plan


def split_records(source, session, message, part, text, max_chars,
                  role=None, time_created=None, part_time_created=None):
    return [{"ref": f"{source}/{session}/{message}/{part}@{start}:{min(start + max_chars, len(text))}",
             "text": text[start:start + max_chars], "role": role,
             "timeCreated": time_created, "partTimeCreated": part_time_created}
            for start in range(0, len(text), max_chars)]


def chunk_records(records, chunk_chars):
    chunks, batch, size = [], [], 0
    for record in records:
        if batch and size + len(record["text"]) > chunk_chars:
            chunks.append(batch)
            batch, size = [], 0
        batch.append(record)
        size += len(record["text"])
    if batch:
        chunks.append(batch)
    return chunks


def export(work, max_chars=2500, chunk_chars=12000):
    work = Path(work)
    plan = load(work / "plan.json")
    archive = Path(plan["archive"])
    path, sha = pinned_snapshot(archive, "primary")
    if str(path) != plan["source"] or sha != plan["sourceSha256"]:
        raise ValueError("source pin changed since prepare")
    if digest(archive / "reconciliation.json") != plan["reconciliationSha256"]:
        raise ValueError("reconciliation pin changed since prepare")
    if not 0 < max_chars <= chunk_chars <= 50000:
        raise ValueError("invalid chunk sizes")
    bundles = []
    with closing(open_snapshot(path)) as db:
        for thread in plan["threads"]:
            session = thread["session"]
            row = db.execute("select 1 from session where id=?", (session,)).fetchone()
            if row is None:
                raise ValueError(f"missing pinned session {session}")
            records, derived, tools, eligible_parts = [], 0, 0, 0
            for msg in db.execute(
                "select id,data,time_created from message where session_id=? order by time_created,id", (session,)
            ):
                metadata = json.loads(msg["data"])
                role = metadata.get("role")
                for part in db.execute(
                    "select id,data,time_created from part where message_id=? order by time_created,id",
                    (msg["id"],)
                ):
                    value = json.loads(part["data"])
                    if value.get("type") == "tool":
                        tools += 1
                        continue
                    if (role not in ("user", "assistant") or metadata.get("summary") is True
                            or metadata.get("agent") == "compaction" or value.get("synthetic")
                            or (value.get("metadata") or {}).get("chaosboxMigrationDraft")):
                        derived += 1
                        continue
                    text = value.get("text")
                    if value.get("type") != "text" or not isinstance(text, str) or not text.strip():
                        continue
                    eligible_parts += 1
                    records.extend(split_records("primary", session, msg["id"], part["id"],
                                                 text, max_chars, role, msg["time_created"],
                                                 part["time_created"]))
            if not records:
                raise ValueError(f"no eligible text in pinned session {session}")
            if sum(len(record["text"]) for record in records) > MAX_SESSION_TEXT_CHARS:
                raise ValueError(f"session {session} exceeds bounded source-text budget")
            chunks = [{"id": f"{session}-{i:04d}", "repoCwdHint": thread["repo"],
                       "session": session, "sourceSha256": sha, "records": group}
                      for i, group in enumerate(chunk_records(records, chunk_chars))]
            bundles.append({"repoCwdHint": thread["repo"], "session": session,
                            "lineage": thread.get("lineage"), "lineageCount": thread.get("lineageCount", 1),
                            "title": thread["title"], "stratum": thread["stratum"],
                            "eligibleTextParts": eligible_parts, "excludedDerived": derived,
                            "excludedTool": tools, "chunks": chunks})
    write_new(work / "bundles.json", bundles)
    return bundles


KINDS = {"feature", "unfinished", "friction", "decision", "outcome", "constraint"}


def validate_signals(answer, chunk):
    if not isinstance(answer, dict) or not isinstance(answer.get("brief"), str):
        raise ValueError("invalid brief")
    signals = answer.get("signals")
    if not isinstance(signals, list) or len(signals) > 12:
        raise ValueError("invalid signals")
    records = {r["ref"]: r for r in chunk["records"]}
    for signal in signals:
        if (not isinstance(signal, dict) or signal.get("kind") not in KINDS
                or not isinstance(signal.get("claim"), str) or not signal["claim"].strip()
                or not isinstance(signal.get("citations"), list) or not signal["citations"]):
            raise ValueError("invalid signal")
        for citation in signal["citations"]:
            if (not isinstance(citation, dict) or citation.get("ref") not in records
                    or not isinstance(citation.get("quote"), str)
                    or citation["quote"] not in records[citation["ref"]]["text"]
                    or (len(citation["quote"]) < 8 and not (
                        records[citation["ref"]].get("role") == "user"
                        and citation["quote"] == records[citation["ref"]]["text"]))):
                raise ValueError("invalid citation (ref or exact quote absent from source)")
    return answer


def collect(work):
    work = Path(work)
    bundles = load(work / "bundles.json")
    signals = {repo: [] for repo in REPOS}
    missing = []
    for bundle in bundles:
        for chunk in bundle["chunks"]:
            file = work / "signals" / f"{chunk['id']}.json"
            if not file.exists():
                missing.append(chunk["id"])
                continue
            answer = validate_signals(load(file), chunk)
            records = {record["ref"]: record for record in chunk["records"]}
            for signal in answer["signals"]:
                signals[bundle["repoCwdHint"]].append({**signal,
                    "session": bundle["session"], "chunk": chunk["id"],
                    "lineage": bundle.get("lineage"),
                    "evidenceRoles": sorted({records[c["ref"]].get("role", "unknown")
                                             for c in signal["citations"]}),
                    "timeCreated": min(records[c["ref"]].get("timeCreated") or 0
                                       for c in signal["citations"])})
    if missing:
        raise ValueError(f"missing {len(missing)} chunk assessments (first: {missing[0]})")
    return signals


def build_briefs(work):
    """Preserve ordered, source-scoped views without new corroboration."""
    work = Path(work)
    by_repo = collect(work)  # Fail closed if even one chunk is absent or invalid.
    briefs = []
    for bundle in load(work / "bundles.json"):
        summaries = [load(work / "signals" / f"{chunk['id']}.json")["brief"]
                     for chunk in bundle["chunks"]]
        briefs.append({"repoCwdHint": bundle["repoCwdHint"], "session": bundle["session"],
                       "title": bundle["title"], "chunkCount": len(summaries),
                       "briefs": summaries, "excludedDerived": bundle["excludedDerived"],
                       "excludedTool": bundle["excludedTool"],
                       "lineage": bundle.get("lineage"),
                       "timeline": [{"timeCreated": s["timeCreated"], "kind": s["kind"],
                                     "evidenceRoles": s["evidenceRoles"], "claim": s["claim"],
                                     "citations": s["citations"]}
                                    for s in sorted((s for s in by_repo[bundle["repoCwdHint"]]
                                                     if s["session"] == bundle["session"]),
                                                    key=lambda s: s["timeCreated"])],
                       "note": "Generated summaries of original slices; not independent evidence."})
    write_new(work / "briefs.json", briefs)
    return briefs


def metrics(work):
    work = Path(work)
    bundles = load(work / "bundles.json")
    chunks = [c for bundle in bundles for c in bundle["chunks"]]
    completed = []
    signals_by_cwd = {repo: 0 for repo in REPOS}
    for bundle in bundles:
        for chunk in bundle["chunks"]:
            file = work / "signals" / f"{chunk['id']}.json"
            if file.exists():
                answer = validate_signals(load(file), chunk)
                completed.append(chunk["id"])
                signals_by_cwd[bundle["repoCwdHint"]] += len(answer["signals"])
    prompts = work / "prompts"
    result = {
        "sourceSha256": load(work / "plan.json")["sourceSha256"],
        "selectedSessions": len(bundles), "totalChunks": len(chunks),
        "longRoots": sum(t["messageCount"] > 120 for t in load(work / "plan.json")["threads"]),
        "distinctLineages": len({b.get("lineage") or b["session"] for b in bundles}),
        "replayedRoots": sum(t.get("lineageCount", 1) - 1
                             for t in load(work / "plan.json")["threads"]),
        "reconciliationVariants": sum(len(t.get("variants", []))
                                      for t in load(work / "plan.json")["threads"]),
        "extractedChunks": len(completed), "missingChunks": len(chunks) - len(completed),
        "sourceSlices": sum(len(c["records"]) for c in chunks),
        "sourceCharacters": sum(len(r["text"]) for c in chunks for r in c["records"]),
        "excludedDerived": sum(b["excludedDerived"] for b in bundles),
        "excludedTool": sum(b["excludedTool"] for b in bundles),
        "modelErrors": len(list(prompts.glob("*.error.json"))) if prompts.exists() else 0,
        "modelTimeouts": len(list(prompts.glob("*.timeout.json"))) if prompts.exists() else 0,
        "signalsByCwd": signals_by_cwd,
        "jevUsage": jev_usage(work),
    }
    model_db = work / "model.sqlite"
    if model_db.exists():
        with closing(sqlite3.connect(model_db.as_uri() + "?mode=ro", uri=True)) as db:
            row = db.execute("select count(*), coalesce(sum(tokens_input),0), "
                             "coalesce(sum(tokens_output),0), coalesce(sum(cost),0) "
                             "from session_v2").fetchone()
            result["isolatedModelUsage"] = {"sessions": row[0], "inputTokens": row[1],
                                            "outputTokens": row[2], "reportedCost": row[3]}
    return result


def private_work(work):
    work = Path(work).resolve(strict=True)
    if (not work.is_dir() or stat.S_IMODE(work.stat().st_mode) & 0o077
            or not (work / "plan.json").is_file()):
        raise ValueError("model execution requires a prepared private work directory (mode 0700)")
    return work


SENSITIVE = re.compile(r"(?i)(-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----|"
                       r"(?:ghp_|github_pat_|sk-[A-Za-z0-9_-]{16}|AKIA[0-9A-Z]{16})|"
                       r"(?:authorization\s*[:=]\s*bearer\s+\S+)|"
                       r"(?:password|api[_-]?key|secret)\s*[:=]\s*['\"]?[A-Za-z0-9+/=_-]{16,})")


class JevRunner:
    """One dispatched attempt per call, with durable work-wide budget reservations."""

    def __init__(self, work, retry=False, max_requests=100, max_input_tokens=1_000_000):
        self.work = private_work(work)
        self.retry = retry
        if max_requests < 1 or max_input_tokens < 1:
            raise ValueError("Jev budgets must be positive")
        self.max_requests, self.max_input_tokens = max_requests, max_input_tokens
        self.binary = None

    def _preflight(self):
        if self.binary is not None:
            return self.binary
        binary = os.environ.get("SESSION_RESEARCH_CHAOSBOX_BIN", "chaosbox")
        expected = {"version": 1, "model": jev.MODEL,
                    "endpoint": "https://api.typesafe.ai/v1/systemone", "receipt_version": 1,
                    "strict_model_identity": True, "redirects": False}
        try:
            proc = subprocess.run([binary, "jev", "capabilities"], cwd=self.work,
                                  capture_output=True, text=True, timeout=10)
            capabilities = json.loads(proc.stdout)
            if (proc.returncode or not isinstance(capabilities, dict)
                    or any(type(capabilities.get(key)) is not type(value)
                           or capabilities[key] != value for key, value in expected.items())):
                raise ValueError("incompatible policy")
        except (OSError, subprocess.TimeoutExpired, ValueError):
            raise ValueError("Jev capabilities preflight failed; build the current Chaosbox CLI and "
                             "set SESSION_RESEARCH_CHAOSBOX_BIN to its absolute path") from None
        self.binary = binary
        return binary

    def __call__(self, state, questions):
        request = {"model": jev.MODEL, "state": {**state, "rubric": jev.RUBRIC},
                   "questions": questions}
        encoded = json.dumps(request, ensure_ascii=False, sort_keys=True)
        if len(encoded.encode("utf-8")) > 180000:
            raise ValueError("Jev prompt exceeds bounded input; narrow the reviewed cohort")
        if SENSITIVE.search(encoded):
            raise ValueError("Jev context contains potential credentials")
        identity = {"request": request, "planSha256": digest(self.work / "plan.json"),
                    "probability": jev.MIN_PROBABILITY, "confidence": jev.MIN_CONFIDENCE}
        key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
        directory = self.work / "jev"
        directory.mkdir(mode=0o700, exist_ok=True)
        if directory.is_symlink() or stat.S_IMODE(directory.stat().st_mode) & 0o077:
            raise ValueError("Jev receipts require a private directory")
        fd = os.open(directory / ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return self._evaluate(directory, key, request, identity)

    def _evaluate(self, directory, key, request, identity):
        pin = directory / f"{key}.input.json"
        if pin.exists():
            if load(pin) != identity:
                raise ValueError("Jev cached input differs")
        else:
            write_new(pin, identity)
        attempts = sorted(directory.glob(f"{key}.*.intent.json"))
        for attempt in attempts:
            result = attempt.with_name(attempt.name.replace(".intent.json", ".result.json"))
            if result.exists() and (receipt := load(result)).get("response") is not None:
                return jev.validate_response(receipt["response"], request["questions"])
        if attempts and not self.retry:
            raise ValueError("prior Jev attempt failed or was interrupted; explicit --retry required")
        # Charge crashes/timeouts too. Reservations deliberately exceed the token estimate;
        # actual provider usage is also retained and can increase the accounting.
        reserved = len(json.dumps(request).encode()) + 4096 + 256 * len(request["questions"])
        usage = jev_usage(self.work)
        if (usage["reservedRequests"] >= self.max_requests
                or usage["chargedInputTokens"] + reserved > self.max_input_tokens):
            raise ValueError("work-wide Jev budget exceeded")
        binary = self._preflight()
        stem = f"{key}.{len(attempts) + 1:04d}"
        wire = directory / f"{key}.request.json"
        if not wire.exists():
            write_new(wire, request)
        elif load(wire) != request:
            raise ValueError("Jev wire request differs from pinned input")
        write_new(directory / f"{stem}.intent.json", {"reservedInputTokens": reserved})
        reported_tokens = 0
        try:
            proc = subprocess.run([binary, "jev", "evaluate", "--privacy-reviewed", "--input", str(wire)],
                                  cwd=self.work, capture_output=True, text=True, timeout=90)
            receipt = json.loads(proc.stdout)
            if (not isinstance(receipt, dict) or type(receipt.get("input_tokens")) is not int
                    or receipt["input_tokens"] < 0):
                raise ValueError("invalid Jev receipt")
            reported_tokens = receipt["input_tokens"]
            if proc.returncode or receipt.get("error") or receipt.get("version") != 1:
                code = receipt.get("error")
                code = code if code in {"invalid_request", "auth", "budget", "context", "transport",
                                        "schema", "protocol", "transient", "cancelled", "model_mismatch"} else "evaluation_failed"
                receipt = {"error": code, "response": None,
                           "input_tokens": receipt.get("input_tokens", 0)}
            else:
                jev.validate_response(receipt["response"], request["questions"])
                receipt["input_tokens"] = receipt["response"]["usage"]["input_tokens"]
        except (OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError):
            receipt = {"error": "transport_or_protocol", "response": None, "input_tokens": reported_tokens}
        if usage["chargedInputTokens"] + max(reserved, receipt.get("input_tokens", 0)) > self.max_input_tokens:
            receipt.update(error="input_budget", response=None)
        write_new(directory / f"{stem}.result.json", receipt)
        if receipt.get("error"):
            raise ValueError(f"Jev evaluation failed; inspect private jev/{stem}.result.json")
        return receipt["response"]


def jev_usage(work):
    usage = {"reservedRequests": 0, "chargedInputTokens": 0, "reportedInputTokens": 0,
             "reportedOutputTokens": 0, "failedOrInterrupted": 0}
    for intent in (Path(work) / "jev").glob("*.intent.json"):
        result = intent.with_name(intent.name.replace(".intent.json", ".result.json"))
        receipt = load(result) if result.exists() else {}
        tokens = receipt.get("input_tokens", 0)
        usage["reservedRequests"] += 1
        usage["chargedInputTokens"] += max(load(intent)["reservedInputTokens"], tokens)
        usage["reportedInputTokens"] += tokens
        if receipt.get("response"):
            usage["reportedOutputTokens"] += receipt["response"]["usage"]["output_tokens"]
        else:
            usage["failedOrInterrupted"] += 1
    return usage


def run_extract(work, limit=None, privacy_reviewed=False, retry=False,
                max_requests=100, max_input_tokens=1_000_000):
    if not privacy_reviewed:
        raise ValueError("inspect exported text locally before --privacy-reviewed")
    work = Path(work)
    verify_sources(work)
    bundles = load(work / "bundles.json")
    evaluate = JevRunner(work, retry, max_requests, max_input_tokens)
    metadata = source_meta(bundles)
    directory = work / "signals"
    directory.mkdir(mode=0o700, exist_ok=True)
    done = 0
    for bundle in bundles:
        for chunk in bundle["chunks"]:
            target = directory / f"{chunk['id']}.json"
            if target.exists():
                if load(target).get("rubric") != jev.RUBRIC:
                    raise ValueError("legacy extraction; use a new work directory for Jev")
                pin_artifact(target, jev.extract(chunk, {r["ref"]: metadata[r["ref"]]
                                                       for r in chunk["records"]}, evaluate))
                continue
            if limit is not None and done >= limit:
                return done
            if any(SENSITIVE.search(record["text"]) for record in chunk["records"]):
                raise ValueError(f"potential credential in {chunk['id']}; remove from pilot")
            answer = jev.extract(chunk, {r["ref"]: metadata[r["ref"]] for r in chunk["records"]}, evaluate)
            write_new(target, validate_signals(answer, chunk))
            done += 1
    return done


def source_texts(bundles, repo=None):
    return {record["ref"]: record["text"] for bundle in bundles
            if repo is None or bundle["repoCwdHint"] == repo
            for chunk in bundle["chunks"] for record in chunk["records"]}


def source_windows(findings, original, margin=96):
    """Original substrings around cited quotes; each window stays independent."""
    ranges = {}
    for finding in findings:
        for citation in finding["citations"]:
            ref, quote = citation["ref"], citation["quote"]
            start = original[ref].find(quote)
            if start < 0:
                raise ValueError("cited quote absent from original source")
            ranges.setdefault(ref, []).append((max(0, start - margin),
                                                 min(len(original[ref]), start + len(quote) + margin)))
    windows = {}
    for ref, locations in ranges.items():
        merged = []
        for begin, end in sorted(locations):
            if merged and begin <= merged[-1][1]:
                merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
            else:
                merged.append((begin, end))
        windows[ref] = [original[ref][begin:end] for begin, end in merged]
    return windows


def evenly_spaced(items, count):
    if count <= 0:
        return []
    if len(items) <= count:
        return items
    if count == 1:
        return [items[-1]]
    return [items[round(i * (len(items) - 1) / (count - 1))] for i in range(count)]


def select_report_findings(findings, metadata, sessions, per_session=8):
    """Retain every direct user signal, sampling assistant context per root."""
    chosen = []
    for session in sessions:
        items = [item for item in findings if item["session"] == session]
        direct = [item for item in items if any(
            metadata[c["ref"]]["role"] == "user"
            and not metadata[c["ref"]].get("pastedTemplateText", False)
            and not metadata[c["ref"]].get("embeddedToolText", False)
            for c in item["citations"])]
        needs = [item for item in direct if item["kind"] in
                 ("feature", "unfinished", "friction", "constraint")]
        assistant_outcomes = [item for item in items if item["kind"] == "outcome"
                              and item not in direct]
        reserve = 1 if assistant_outcomes else 0
        primary = evenly_spaced(needs, per_session - reserve)
        remaining = per_session - reserve - len(primary)
        extras = [item for item in direct if item not in primary]
        primary += evenly_spaced(extras, remaining)
        if assistant_outcomes:
            primary.append(assistant_outcomes[-1])
        # A later user decision can reverse a proposal. Dropping it for a
        # per-root prompt quota changes the requested scope, not just coverage.
        primary += [item for item in direct if item not in primary]
        remaining = per_session - len(primary)
        other = [item for item in items if item not in primary and item not in direct
                 and not any(metadata[c["ref"]].get("pastedTemplateText", False)
                             or metadata[c["ref"]].get("embeddedToolText", False)
                             for c in item["citations"])]
        primary += evenly_spaced(other, remaining)
        chosen.extend(sorted(primary, key=lambda item: item["timeCreated"]))
    return chosen


def source_meta(bundles, repo=None):
    metadata = {}
    for bundle in bundles:
        if repo is not None and bundle["repoCwdHint"] != repo:
            continue
        records = [r for chunk in bundle["chunks"] for r in chunk["records"]]
        embedded_parts = {r["ref"].split("@", 1)[0] for r in records
                          if r.get("role") == "user" and re.match(
                              r"(?is)^\s*(?:called the (?:read|grep|glob|bash|shell|edit|write) "
                              r"tool with the following input:|<path>.*?<content>)", r["text"])}
        template_parts = {r["ref"].split("@", 1)[0] for r in records
                          if r.get("role") == "user" and re.match(
                              r"(?i)^\s*(?:# [^\n]{1,90}\n\s*(?:## Purpose and boundaries|This skill\b)"
                              r"|# Skill: |<skill_content\b)", r["text"])}
        for record in records:
            metadata[record["ref"]] = {
                "role": record.get("role", "unknown"),
                "timeCreated": record.get("timeCreated"),
                "session": bundle["session"], "lineage": bundle.get("lineage"),
                "embeddedToolText": record["ref"].split("@", 1)[0] in embedded_parts,
                "pastedTemplateText": record["ref"].split("@", 1)[0] in template_parts}
    return metadata


def verify_sources(work):
    """Recheck every exported slice against the pinned original SQLite part."""
    work = Path(work)
    plan, bundles = load(work / "plan.json"), load(work / "bundles.json")
    archive = Path(plan["archive"])
    path, sha = pinned_snapshot(archive, "primary")
    if sha != plan["sourceSha256"] or digest(archive / "reconciliation.json") != plan["reconciliationSha256"]:
        raise ValueError("source pin changed since export")
    total = 0
    with closing(open_snapshot(path)) as db:
        for bundle in bundles:
            for chunk in bundle["chunks"]:
                for record in chunk["records"]:
                    match = re.fullmatch(
                        r"primary/(ses_[A-Za-z0-9_]+)/(msg_[A-Za-z0-9_]+)/(prt_[A-Za-z0-9_]+)@(\d+):(\d+)",
                        record["ref"])
                    if not match or match[1] != bundle["session"] or chunk["session"] != bundle["session"]:
                        raise ValueError("invalid exported source part reference")
                    session, message, part, begin, end = match.groups()
                    row = db.execute("select p.data, m.data, m.time_created, p.time_created "
                                     "from part p join message m on m.id=p.message_id "
                                     "where p.id=? and p.message_id=? and p.session_id=? and m.session_id=?",
                                     (part, message, session, session)).fetchone()
                    if row is None:
                        raise ValueError("missing original source part")
                    body, meta = json.loads(row[0]), json.loads(row[1])
                    text = body.get("text")
                    if (meta.get("role") not in ("user", "assistant") or meta.get("summary") is True
                            or meta.get("agent") == "compaction" or body.get("synthetic")
                            or (body.get("metadata") or {}).get("chaosboxMigrationDraft")
                            or body.get("type") != "text"
                            or not isinstance(text, str) or not 0 <= int(begin) < int(end) <= len(text)
                            or record["text"] != text[int(begin):int(end)]
                            or (plan["version"] >= 2 and
                                (record.get("role") != meta["role"]
                                 or record.get("timeCreated") != row[2]
                                 or record.get("partTimeCreated") != row[3]))):
                        raise ValueError("export disagrees with original source part")
                    total += 1
    return total


def inspect(work, ref, radius=2):
    """Return bounded, pinned source context for an exported citation."""
    if not 0 <= radius <= 3:
        raise ValueError("source context radius must be 0–3")
    verify_sources(work)
    for bundle in load(Path(work) / "bundles.json"):
        records = [record for chunk in bundle["chunks"] for record in chunk["records"]]
        for i, record in enumerate(records):
            if record["ref"] == ref:
                return {"session": bundle["session"], "title": bundle["title"],
                        "records": records[max(0, i-radius):i+radius+1]}
    raise ValueError("citation not exported in this pinned pilot")


def check_citations(citations, texts, label):
    if not isinstance(citations, list) or not citations:
        raise ValueError(f"missing {label}")
    for citation in citations:
        if (not isinstance(citation, dict) or citation.get("ref") not in texts
                or not isinstance(citation.get("quote"), str)
                or citation["quote"] not in texts[citation["ref"]]
                or (len(citation["quote"]) < 8 and citation["quote"] != texts[citation["ref"]])):
            raise ValueError(f"invalid {label}")


def validate_report(report, source, repo_context, reproductions=None):
    if reproductions is not None and not isinstance(reproductions, dict):
        raise ValueError("reproductions must be an object")
    if (not isinstance(report, dict) or not isinstance(report.get("summary"), str)
            or not isinstance(report.get("limitations"), list)):
        raise ValueError("invalid report")
    opportunities = report.get("opportunities")
    if not isinstance(opportunities, list) or len(opportunities) > 8:
        raise ValueError("invalid opportunities")
    for entry in opportunities:
        for key in ("title", "problem", "proposal", "firstSlice", "validation"):
            if not isinstance(entry.get(key), str) or not entry[key].strip():
                raise ValueError(f"invalid report {key}")
        if entry.get("kind") not in ("feature", "unfinished", "friction"):
            raise ValueError("invalid report kind")
        if entry.get("priority") not in ("high", "medium", "low", "unverified"):
            raise ValueError("invalid report priority")
        if entry.get("currentStatus") not in ("unverified", "present", "partial", "rejected",
                                               "confirmed-gap"):
            raise ValueError("invalid current status")
        check_citations(entry.get("sessionEvidence"), source, "source evidence")
        evidence = entry.get("repoEvidence", [])
        if entry["currentStatus"] != "unverified":
            check_citations(evidence, repo_context, "repository evidence")
        elif evidence:
            check_citations(evidence, repo_context, "repository evidence")
        if entry["currentStatus"] == "confirmed-gap":
            ref = entry.get("reproductionRef")
            receipt = (reproductions or {}).get(ref) if isinstance(ref, str) else None
            revisions = {c["ref"].split(":", 1)[0] for c in evidence}
            if not jev.reproduction_matches(receipt, revisions):
                raise ValueError("confirmed-gap requires a pinned reproduction receipt")
    return report


def repo_context(directory, files):
    """Pinned HEAD excerpts, not a claim of exhaustive present-state coverage."""
    def git(*args):
        return subprocess.run(["git", "-C", str(directory), *args], check=True,
                              capture_output=True, text=True, timeout=30).stdout
    commit = git("rev-parse", "HEAD").strip()
    snippets = {}
    for spec in files:
        match = re.fullmatch(r"([^#]+)(?:#L([1-9][0-9]*)-L([1-9][0-9]*))?", spec)
        if not match:
            raise ValueError("repository file range must be path#Lstart-Lend")
        name, first, last = match.groups()
        file = Path(name)
        if file.is_absolute() or ".." in file.parts:
            raise ValueError("repository files must be relative tracked paths")
        if int(git("cat-file", "-s", f"{commit}:{name}").strip()) > 200 * 1024:
            raise ValueError("tracked repository file too large for bounded context")
        lines = git("show", f"{commit}:{name}").splitlines()
        begin, end = int(first or 1), int(last or min(len(lines), 120))
        if end < begin or end - begin >= 120 or begin > len(lines):
            raise ValueError("repository context line range invalid or too large")
        for i in range(begin, min(end, len(lines)) + 1):
            line = lines[i-1]
            if line.strip():
                snippets[f"{commit}:{name}#L{i}"] = line
    if sum(len(s) for s in snippets.values()) > 45000:
        raise ValueError("repository context exceeds review budget")
    return snippets


def validate_portfolio(report, texts, repos_by_ref=None):
    if not isinstance(report.get("summary"), str) or not isinstance(report.get("shared"), list):
        raise ValueError("invalid portfolio")
    if len(report["shared"]) > 8:
        raise ValueError("too many shared opportunities")
    for entry in report["shared"]:
        if entry.get("owner") not in REPOS:
            raise ValueError("invalid owner")
        for name in ("title", "problem", "firstSlice"):
            if not isinstance(entry.get(name), str) or not entry[name].strip():
                raise ValueError("incomplete shared opportunity")
        check_citations(entry.get("sessionEvidence"), texts, "cross-repo source evidence")
        if repos_by_ref is not None:
            owners = set()
            for citation in entry["sessionEvidence"]:
                value = repos_by_ref.get(citation["ref"], [])
                owners.update([value] if isinstance(value, str) else value)
            if len(owners) < 2:
                raise ValueError("shared opportunity needs evidence from two repositories")
    return report


def audit_opportunity(opportunity, meta):
    cited = [meta[c["ref"]] for c in opportunity["sessionEvidence"]]
    roles = {record["role"] for record in cited}
    lineages = {record["lineage"] or record["session"] for record in cited}
    sessions = {record["session"] for record in cited}
    return {"title": opportunity["title"], "currentStatus": opportunity.get("currentStatus"),
            "sourceRoles": sorted(roles), "citedSessions": len(sessions),
            "independentLineages": len(lineages),
            "assistantOnly": roles == {"assistant"},
            "embeddedToolText": any(record.get("embeddedToolText", False) for record in cited),
            "pastedTemplateText": any(record.get("pastedTemplateText", False) for record in cited),
            "repeatLineage": len(sessions) > len(lineages)}


def audit_reports(work):
    """Quantify evidence provenance and highlight items needing human review."""
    work = Path(work)
    verify_sources(work)
    collect(work)
    bundles = load(work / "bundles.json")
    meta, texts = source_meta(bundles), source_texts(bundles)
    reports = {}
    for repo in REPOS:
        report = load(work / "reports" / f"{repo}.json")
        prompt = load(work / "prompts" / f"report-{repo}.json")
        validate_saved_report(report, prompt, bundles, repo)
        reports[repo] = [audit_opportunity(entry, meta) for entry in report["opportunities"]]
    portfolio = load(work / "reports" / "portfolio.json")
    validate_portfolio(portfolio, texts, report_owners(work, bundles))
    return {"sourceSha256": load(work / "plan.json")["sourceSha256"],
            "selectedSessions": len(bundles),
            "lineages": len({b.get("lineage") or b["session"] for b in bundles}),
            "repos": reports, "portfolio": [audit_opportunity(entry, meta)
                                             for entry in portfolio["shared"]],
            "note": "Evidence roles and counts are structural; a human must review whether quotes support claims."}


def pin_artifact(path, value):
    if path.exists():
        if load(path) != value:
            raise ValueError("existing artifact differs; use a new work directory")
    else:
        write_new(path, value)


def synthesize(work, repo_files=None, privacy_reviewed=False, retry=False, decisions=None,
               max_requests=100, max_input_tokens=1_000_000):
    from session_research_ledger import build_ledger, review_context, validate_candidate_view
    if not privacy_reviewed:
        raise ValueError("inspect private findings before --privacy-reviewed")
    work = Path(work)
    verify_sources(work)
    bundles = load(work / "bundles.json")
    findings = collect(work)  # Refuses partial extraction.
    if decisions is None:
        raise ValueError("synthesis requires reviewed candidate decisions; run ledger first")
    ledger = build_ledger(findings, bundles, decisions)
    ledger_path = work / "candidate-ledger.json"
    if ledger_path.exists():
        if load(ledger_path) != ledger:
            raise ValueError("candidate review changed; use a new synthesis work directory")
    else:
        write_new(ledger_path, ledger)
    reproductions = load(work / "reproductions.json") if (work / "reproductions.json").exists() else {}
    if not isinstance(reproductions, dict):
        raise ValueError("reproductions must be an object")
    repo_files = repo_files or {}
    reports = {}
    directory = work / "reports"
    directory.mkdir(mode=0o700, exist_ok=True)
    prompts = work / "prompts"
    prompts.mkdir(mode=0o700, exist_ok=True)
    evaluate = JevRunner(work, retry, max_requests, max_input_tokens)
    for repo in REPOS:
        contexts = {}
        if repo in repo_files:
            root, files = repo_files[repo]
            contexts = repo_context(root, files)
        if SENSITIVE.search(json.dumps(contexts)):
            raise ValueError("repo context contains potential credentials")
        review = review_context(ledger, repo)
        original = {r["ref"]: r["text"] for conversation in review["conversations"]
                    for r in conversation["records"]}
        payload = {"targetRepository": repo, "reviewVersion": 1, "rubric": jev.RUBRIC, **review,
                   "sourceMeta": {ref: meta for ref, meta in source_meta(bundles).items()
                                  if ref in original},
                   "repoContext": contexts, "reproductions": reproductions,
                   "note": "Ownership and status are operator review annotations, not current-code proof. "
                           "Full exported conversation context is retained; earlier export limits still apply."}
        if SENSITIVE.search(json.dumps(payload)):
            raise ValueError("review context contains potential credentials")
        if len(json.dumps(payload, ensure_ascii=False).encode("utf-8")) > 200000:
            raise ValueError(f"{repo} synthesis exceeds prompt budget; split into smaller cited batches")
        target = directory / f"{repo}.json"
        pin_artifact(prompts / f"report-{repo}.json", payload)
        report = jev.report(payload, evaluate)
        reports[repo] = validate_report(report, original, contexts, reproductions)
        validate_candidate_view(report, review["candidates"])
        pin_artifact(target, report)
    target = directory / "portfolio.json"
    portfolio = jev.portfolio(reports, source_meta(bundles), evaluate)
    validate_portfolio(portfolio, source_texts(bundles), report_owners(work, bundles))
    pin_artifact(target, portfolio)
    return reports, portfolio


def report_owners(work, bundles):
    path = Path(work) / "candidate-ledger.json"
    if not path.exists():
        return {r["ref"]: b["repoCwdHint"] for b in bundles for c in b["chunks"] for r in c["records"]}
    owners = {}
    for row in load(path)["candidates"]:
        if row["disposition"] == "selected":
            for citation in row["finding"]["citations"] + row["ownershipEvidence"]:
                owners.setdefault(citation["ref"], set()).add(row["targetRepository"])
    return {ref: sorted(values) for ref, values in owners.items()}


def validate_saved_report(report, prompt, bundles, repo):
    from session_research_ledger import validate_candidate_view
    if prompt.get("reviewVersion") == 1:
        refs = {r["ref"] for conversation in prompt["conversations"] for r in conversation["records"]}
        source = {ref: text for ref, text in source_texts(bundles).items() if ref in refs}
        validate_candidate_view(report, prompt["candidates"])
    else:
        source = source_texts(bundles, repo)
    return validate_report(report, source, prompt["repoContext"], prompt.get("reproductions", {}))


def write_markdown(path, content):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as output:
        output.write(content)


def format_historical_citation(citation, record):
    created = record["timeCreated"]
    at = (datetime.fromtimestamp(created / 1000, timezone.utc).isoformat(timespec="seconds")
          if isinstance(created, int) and created > 0 else "time unknown")
    label = record["role"]
    label += " (embedded tool text)" if record["embeddedToolText"] else ""
    label += " (pasted template)" if record["pastedTemplateText"] else ""
    return f"{label} at {at}, `{citation['ref']}`: {json.dumps(citation['quote'], ensure_ascii=False)}"


def render(work):
    work = Path(work)
    bundles = load(work / "bundles.json")
    plan = load(work / "plan.json")
    collect(work)
    verify_sources(work)
    report_dir = work / "reports"
    if not all((report_dir / f"{repo}.json").is_file() for repo in REPOS):
        raise ValueError("missing repository reports")
    if not (report_dir / "portfolio.json").is_file():
        raise ValueError("missing portfolio report")
    source = source_texts(bundles)
    meta = source_meta(bundles)
    documents = {}
    for repo in REPOS:
        report = load(report_dir / f"{repo}.json")
        prompt = load(work / "prompts" / f"report-{repo}.json")
        validate_saved_report(report, prompt, bundles, repo)
        sampled = [row for row in plan["threads"] if row["repo"] == repo]
        updates = [row["timeUpdated"] for row in sampled if row.get("timeUpdated") is not None]
        period = (f"{datetime.fromtimestamp(min(updates) / 1000, timezone.utc).date()} to "
                  f"{datetime.fromtimestamp(max(updates) / 1000, timezone.utc).date()}"
                  if updates else "unknown")
        lines = [f"# {repo}: session-derived opportunities", "",
                 f"Frozen primary snapshot: `{plan['sourceSha256']}`", "",
                 f"Sample: {sum(b['repoCwdHint'] == repo for b in bundles)} root sessions; "
                 f"last updated {period}; selected by historical cwd, not a complete repo audit.", "",
                  report["summary"], ""]
        if prompt.get("reviewVersion") == 1:
            lines.extend(["Ownership: reviewed per candidate; full inventory: `../candidate-ledger.json`.", ""])
        for i, item in enumerate(report["opportunities"], 1):
            lines.extend([f"## {i}. {item['title']} ({item['kind']}; {item['priority']})", "",
                          f"- Problem: {item['problem']}", f"- Proposal: {item['proposal']}",
                          f"- First slice: {item['firstSlice']}", f"- Validation: {item['validation']}",
                          f"- Current status: {item['currentStatus']}",
                          "- Historical evidence:"])
            for citation in item["sessionEvidence"]:
                check_citations([citation], source, "source evidence")
                lines.append("  - " + format_historical_citation(citation, meta[citation["ref"]]))
            for citation in item.get("repoEvidence", []):
                lines.append(f"- Pinned repository excerpt `{citation['ref']}`: "
                             f"{json.dumps(citation['quote'], ensure_ascii=False)}")
            if prompt.get("reviewVersion") == 1:
                lines.extend([f"- Candidate IDs: {', '.join(item['candidateIds'])}",
                              f"- Reviewed action: {item['action']}; implementation: {item['implementation']}; "
                              f"deployment: {item['deployment']}"])
            lines.append("")
        lines.extend(["## Limitations", "", *[f"- {note}" for note in report["limitations"]], ""])
        documents[repo] = "\n".join(lines)
    portfolio = validate_portfolio(load(report_dir / "portfolio.json"), source, report_owners(work, bundles))
    lines = ["# Cross-repository opportunities", "", portfolio["summary"], ""]
    for i, item in enumerate(portfolio["shared"], 1):
        lines.extend([f"## {i}. {item['title']} (owner: {item['owner']})", "",
                      item["problem"], "", f"First slice: {item['firstSlice']}", ""])
        for citation in item["sessionEvidence"]:
            lines.append("- " + format_historical_citation(citation, meta[citation["ref"]]))
        lines.append("")
    lines.extend(["## Limitations", "", *[f"- {note}" for note in portfolio.get("limitations", [])], ""])
    documents["portfolio"] = "\n".join(lines)
    for name, text in documents.items():
        write_markdown(report_dir / f"{name}.md", text)
    return documents


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prep = commands.add_parser("prepare")
    prep.add_argument("--archive", type=Path, required=True)
    prep.add_argument("--work", type=Path, required=True)
    prep.add_argument("--per-repo", type=int, default=12)
    prep.add_argument("--session", action="append", dest="sessions",
                      help="Curated canonical root; repeat to choose a reviewed sample")
    exp = commands.add_parser("export")
    exp.add_argument("--work", type=Path, required=True)
    exp.add_argument("--max-chars", type=int, default=2500)
    exp.add_argument("--chunk-chars", type=int, default=12000)
    col = commands.add_parser("collect")
    col.add_argument("--work", type=Path, required=True)
    ledger = commands.add_parser("ledger")
    ledger.add_argument("--work", type=Path, required=True)
    ledger.add_argument("--decisions", type=Path)
    ledger.add_argument("--out", type=Path, required=True)
    run = commands.add_parser("extract")
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--limit", type=int)
    run.add_argument("--privacy-reviewed", action="store_true")
    run.add_argument("--retry", action="store_true")
    run.add_argument("--max-requests", type=int, default=100)
    run.add_argument("--max-input-tokens", type=int, default=1_000_000)
    synth = commands.add_parser("synthesize")
    synth.add_argument("--work", type=Path, required=True)
    synth.add_argument("--privacy-reviewed", action="store_true")
    synth.add_argument("--retry", action="store_true")
    synth.add_argument("--max-requests", type=int, default=100)
    synth.add_argument("--max-input-tokens", type=int, default=1_000_000)
    synth.add_argument("--decisions", type=Path, required=True)
    synth.add_argument("--repo-root", action="append", default=[], metavar="REPO=DIR")
    synth.add_argument("--repo-file", action="append", default=[], metavar="REPO=FILE")
    rend = commands.add_parser("render")
    rend.add_argument("--work", type=Path, required=True)
    briefs = commands.add_parser("briefs")
    briefs.add_argument("--work", type=Path, required=True)
    stat = commands.add_parser("metrics")
    stat.add_argument("--work", type=Path, required=True)
    audit = commands.add_parser("audit")
    audit.add_argument("--work", type=Path, required=True)
    source = commands.add_parser("inspect")
    source.add_argument("--work", type=Path, required=True)
    source.add_argument("--ref", required=True)
    source.add_argument("--radius", type=int, default=2)
    args = parser.parse_args()
    if args.command == "prepare":
        plan = prepare(args.archive, args.work, args.per_repo, args.sessions)
        print(json.dumps({"threads": len(plan["threads"]), "byCwd": plan["eligibleRootsByCwd"]}))
    elif args.command == "export":
        bundles = export(args.work, args.max_chars, args.chunk_chars)
        print(json.dumps({"threads": len(bundles), "chunks": sum(len(b["chunks"]) for b in bundles)}))
    elif args.command == "collect":
        print(json.dumps({key: len(value) for key, value in collect(args.work).items()}))
    elif args.command == "ledger":
        from session_research_ledger import build_ledger
        verify_sources(args.work)
        result = build_ledger(collect(args.work), load(args.work / "bundles.json"),
                              load(args.decisions) if args.decisions else None)
        write_new(args.out, result)
        print(json.dumps({"candidates": len(result["candidates"]), "out": str(args.out)}))
    elif args.command == "extract":
        print(json.dumps({"processedChunks": run_extract(args.work, args.limit,
                                   args.privacy_reviewed, args.retry,
                                   args.max_requests, args.max_input_tokens)}))
    elif args.command == "synthesize":
        roots, files = {}, {repo: [] for repo in REPOS}
        for item in args.repo_root:
            name, separator, path = item.partition("=")
            if not separator or name not in REPOS:
                parser.error(f"invalid --repo-root {item}")
            roots[name] = Path(path).resolve(strict=True)
        for item in args.repo_file:
            name, separator, path = item.partition("=")
            if not separator or name not in REPOS or not path:
                parser.error(f"invalid --repo-file {item}")
            files[name].append(path)
        if any(files[name] and name not in roots for name in REPOS):
            parser.error("each --repo-file requires its --repo-root")
        context = {name: (root, files[name]) for name, root in roots.items()}
        reports, portfolio = synthesize(args.work, context, args.privacy_reviewed, args.retry,
                                        load(args.decisions), args.max_requests, args.max_input_tokens)
        print(json.dumps({"repoOpportunities": {name: len(r["opportunities"]) for name, r in reports.items()},
                          "shared": len(portfolio["shared"])}))
    elif args.command == "render":
        print(json.dumps({"reports": list(render(args.work))}))
    elif args.command == "briefs":
        print(json.dumps({"sessionBriefs": len(build_briefs(args.work))}))
    elif args.command == "metrics":
        print(json.dumps(metrics(args.work), indent=2))
    elif args.command == "audit":
        result = audit_reports(args.work)
        write_new(args.work / "reports" / "audit.json", result)
        print(json.dumps({"auditedSessions": result["selectedSessions"],
                          "lineages": result["lineages"],
                          "assistantOnly": sum(item["assistantOnly"] for rows in result["repos"].values()
                                               for item in rows)}))
    elif args.command == "inspect":
        print(json.dumps(inspect(args.work, args.ref, args.radius), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
