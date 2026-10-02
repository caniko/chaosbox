#!/usr/bin/env python3
"""Qualify reciprocal HM-generated endpoints inside the disposable NixOS guest."""
import argparse
import copy
import json
import os
from pathlib import Path
import pwd
import selectors
import shutil
import subprocess

PROJECT = "git:github.com/caniko/chaosbox"
OWNERS = ("can", "dejana")


def run(owner, *args, **kwargs):
    return subprocess.run(
        ["runuser", "-u", owner, "--", *map(str, args)],
        text=True, capture_output=True, timeout=20, **kwargs,
    )


def cli(root, owner, *args):
    return run(
        owner, f"/etc/profiles/per-user/{owner}/bin/chaosbox-federation",
        "federation", "--config", root / owner / ".config/chaosbox/federation.json",
        *args,
    )


def context(root, owner):
    result = cli(root, owner, "context", "--repo", PROJECT, "--", "context citations")
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def evidence(root, owner, handle):
    return cli(root, owner, "evidence", "--repo", PROJECT, "--handle", json.dumps(handle))


def ssh(root, owner, command="chaosbox federation serve", extra=()):
    peer = "dejana" if owner == "can" else "can"
    return [
        "ssh", "-T", "-F", str(root / owner / ".config/chaosbox/federation-ssh.conf"),
        "-o", "ConnectTimeout=5", *extra, f"chaosbox-{peer}-fixture", command,
    ]


def wire(root, owner, request, extra=()):
    result = run(owner, *ssh(root, owner, extra=extra), input=json.dumps(request) + "\n")
    assert result.returncode == 0, result.stderr
    assert len(result.stdout.splitlines()) == 1, result.stdout
    return json.loads(result.stdout)["result"]


def replace_json(path, value, owner):
    # Replace the managed symlink, never write into the Nix store. The operator
    # authority here is root inside the guest; provider files retain owner uid.
    staged = path.with_suffix(".test-new")
    staged.write_text(json.dumps(value))
    account = pwd.getpwnam(owner)
    os.chown(staged, account.pw_uid, account.pw_gid)
    staged.chmod(0o600)
    staged.replace(path)


def install(root, fixtures):
    for owner in OWNERS:
        home = root / owner
        shutil.copyfile(fixtures / f"{owner}.json", home / "current.json")
        shutil.copytree(fixtures / f"{owner}-history", home / "history")
        account = pwd.getpwnam(owner)
        for path in [home / "current.json", home / "history", *(home / "history").iterdir()]:
            os.chown(path, account.pw_uid, account.pw_gid)
            path.chmod(0o700 if path.is_dir() else 0o600)


def one_shot(root):
    frame = {"version": 1, "request": {
        "operation": "context", "project": PROJECT, "query": "context citations",
        "limit": 1, "max_chars": 2000,
    }}
    # Leave stdin open. One newline must produce one flushed reply and exit;
    # the second frame must never turn this into a persistent shell/session.
    process = subprocess.Popen(
        ["runuser", "-u", "can", "--", *ssh(root, "can")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True,
    )
    try:
        process.stdin.write((json.dumps(frame) + "\n") * 2)
        process.stdin.flush()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            assert selector.select(timeout=15), "one-shot reply was not flushed"
            response = json.loads(process.stdout.readline())
        assert response["result"]["status"] == "ok", response
        assert process.wait(timeout=15) == 0, "one-shot endpoint did not exit successfully"
        assert process.stdout.read() == "", "endpoint served more than one frame"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        process.stdin.close()
        process.stdout.close()


def verify_connected(root, fixtures):
    install(root, fixtures)
    originals = {owner: (root / owner / "current.json").read_bytes() for owner in OWNERS}
    packets = {}
    for owner in OWNERS:
        peer = "dejana" if owner == "can" else "can"
        packet = context(root, owner)
        assert packet["degraded"] is False, packet
        assert [s["identity"]["owner"] for s in packet["sources"]] == [owner, peer], packet
        assert all(s["error"] is None for s in packet["sources"]), packet
        assert len(packet["records"]) == 2, packet
        assert len({r["statement"] for r in packet["records"]}) == 2, packet
        assert "PRIVATE-OTHER-PROJECT" not in json.dumps(packet), packet
        for record in packet["records"]:
            origin = record["handle"]["owner"]
            assert record["handle"]["scope"] == f"private:{origin}", record
            assert record["handle"]["provider"] == f"{origin}-fixture", record
            assert record["handle"]["snapshot"] == next((root / origin / "history").iterdir()).stem
            result = evidence(root, owner, record["handle"])
            assert result.returncode == 0, result.stderr
            proof = json.loads(result.stdout)
            assert proof["record"]["handle"] == record["handle"], proof
            assert proof["evidence"] and proof["receipts"], proof
            assert all(r["state_omitted"] for r in proof["receipts"]), proof
            assert "PRIVATE-OTHER-PROJECT" not in result.stdout, proof
        packets[owner] = packet
    assert all((root / owner / "current.json").read_bytes() == originals[owner] for owner in OWNERS)
    one_shot(root)

    # The generated query key cannot authenticate as the other recipient.
    request = {"version": 1, "request": {
        "operation": "context", "project": PROJECT, "query": "context citations",
        "limit": 5, "max_chars": 12000,
    }}
    failed = run("can", *ssh(root, "can", extra=("-o", "User=can")), input=json.dumps(request) + "\n")
    assert failed.returncode != 0 and failed.stdout == "", failed
    spoofed = copy.deepcopy(request)
    spoofed["caller"] = "dejana"
    assert wire(root, "can", spoofed) == {"status": "error", "code": "invalid_request"}
    wrong_project = copy.deepcopy(request)
    wrong_project["request"]["project"] = "foreign-project"
    assert wire(root, "can", wrong_project) == {"status": "error", "code": "denied"}
    shell = run("can", *ssh(root, "can", command="id"), input=json.dumps(request) + "\n")
    assert shell.returncode != 0 and shell.stdout == "", shell

    for owner in OWNERS:
        peer = "dejana" if owner == "can" else "can"
        handle = next(r["handle"] for r in packets[owner]["records"] if r["handle"]["owner"] == peer)
        provider = root / peer / ".config/chaosbox/provider.json"
        original_policy = json.loads(provider.read_text())
        current = root / peer / "current.json"
        # Historical evidence must use the old generation after publication.
        current.write_bytes((fixtures / f"{peer}-next.json").read_bytes())
        next_bytes = current.read_bytes()
        assert context(root, owner)["sources"][1]["snapshot"] != handle["snapshot"]
        result = evidence(root, owner, handle)
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout)["record"]["handle"] == handle
        assert current.read_bytes() == next_bytes, "history read changed the current source"
        unknown = dict(handle, snapshot="0" * 64)
        result = evidence(root, owner, unknown)
        assert result.returncode != 0 and "snapshot_unavailable" in result.stderr, result

        revoked = copy.deepcopy(original_policy)
        revoked["policy"]["grants"] = []
        replace_json(provider, revoked, peer)
        packet = context(root, owner)
        assert packet["sources"][1]["error"] == "denied", packet
        assert all(r["handle"]["owner"] == owner for r in packet["records"]), packet
        result = evidence(root, owner, handle)
        assert result.returncode != 0 and "denied" in result.stderr, result
        replace_json(provider, original_policy, peer)

        current.write_bytes((fixtures / f"{peer}-withheld.json").read_bytes())
        result = evidence(root, owner, handle)
        assert result.returncode != 0 and "denied" in result.stderr, result
        current.write_bytes(originals[peer])
        history = next((root / peer / "history").iterdir())
        assert history.read_bytes() == originals[peer], "immutable history was modified"
    print("PASS: reciprocal generated SSH endpoints, attribution, history, denial, revocation and one-shot exit")


def verify_outage(root):
    for owner in OWNERS:
        packet = context(root, owner)
        assert packet["degraded"] is True, packet
        assert packet["sources"][0]["error"] is None, packet
        assert packet["sources"][1]["error"] == "unavailable", packet
        assert packet["records"] and all(r["handle"]["owner"] == owner for r in packet["records"]), packet
    print("PASS: both local providers survive a stopped SSH peer")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--fixtures", required=True, type=Path)
    parser.add_argument("--phase", required=True, choices=("connected", "outage"))
    args = parser.parse_args()
    # Fail before any writes on a host with real can/dejana accounts.
    if (os.geteuid() != 0 or args.root != Path("/var/lib/chaosbox-federation-test")
            or not all(Path(pwd.getpwnam(owner).pw_dir) == args.root / owner for owner in OWNERS)):
        raise SystemExit("run inside the disposable federation test guest")
    if args.phase == "connected":
        verify_connected(args.root, args.fixtures)
    else:
        verify_outage(args.root)
