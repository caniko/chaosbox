"""Reader acceptance within test-typedb.sh's disposable server and source tree."""

import array
import fcntl
import json
import os
import select
import subprocess
import sys
import termios
import time
from pathlib import Path


def main(work: Path) -> None:
    status = json.loads((work / "status.json").read_text())
    now = int(time.time())
    view = {
        "version": 1,
        "id": "disposable-reader-view",
        "policy_revision": "fixture-v1",
        "identity": {"company": "fixture", "project": "syntax", "agent": "test", "task": "gate",
                     "run": "reader-gate", "host": "disposable-local", "server": "chaosbox"},
        "issued_at": now,
        "expires_at": now + 300,
        "repo": "syntax",
        "build_id": status["build_id"],
        "snapshots": status["snapshots"],
        "source_paths": ["app.ts"],
        "excluded_paths": [],
        "operations": ["status", "search", "lookup", "neighbors", "path", "explain", "evidence", "export"],
        "budgets": {"max_nodes": 100, "max_edges": 100, "max_graph_bytes": 1024 * 1024,
                    "max_evidence": 20, "max_page_size": 20, "max_request_bytes": 8192,
                    "max_response_bytes": 65536, "timeout_ms": 10000, "max_calls": 30,
                    "max_hops": 4, "max_visits": 100},
    }
    admission = work / "read-view.json"
    with admission.open("x") as stream:
        os.fchmod(stream.fileno(), 0o600)
        json.dump(view, stream)
    with (work / "reader.log").open("w") as log:
        process = subprocess.Popen(["chaosbox", "reader", "--admission", str(admission)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                   text=True, bufsize=1)
        try:
            def rpc(method: str, params: dict) -> dict:
                assert process.stdin is not None and process.stdout is not None
                process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}) + "\n")
                process.stdin.flush()
                ready, _, _ = select.select([process.stdout], [], [], 15)
                assert ready, "reader response deadline exceeded"
                line = process.stdout.readline()
                assert line, (work / "reader.log").read_text()
                return json.loads(line)

            def call(name: str, **arguments) -> dict:
                response = rpc("tools/call", {"name": name, "arguments": {"repo": "syntax", **arguments}})
                assert "error" not in response and not response["result"].get("isError"), response
                return json.loads(response["result"]["content"][0]["text"])

            assert rpc("initialize", {"protocolVersion": "2025-06-18"})["result"]["serverInfo"]["name"] == "chaosbox-read-view"
            first = call("search", query="after")
            assert first["data"]["items"]
            assert first["read_view"]["build_id"] == status["build_id"]
            assert first["applicability"] == "unknown" and first["workspace_observation"] is None
            edges = call("export", kind="edges")["data"]["items"]
            assert len(edges) == 3
            for edge in edges:
                evidence = call("evidence", rel=edge["rel_id"])["data"]["evidence"]
                assert evidence and all(row["citation"]["file"] == "app.ts" for row in evidence)

            # Advance the live pointer while this connection holds its old data.
            (work / "syntax" / "app.ts").write_text("export function newest() {}\nexport const value = 3;\n")
            with (work / "third.json").open("w") as output, (work / "third.log").open("w") as errors:
                subprocess.run(["chaosbox", "run", str(work / "syntax"), "--repo", "syntax",
                                "--no-decisions", "--max-candidates", "0"], check=True,
                               stdout=output, stderr=errors, timeout=30)
            assert json.loads((work / "third.json").read_text())["build_id"] != status["build_id"]
            assert call("search", query="after") == first
            assert call("search", query="newest")["data"]["items"] == []
            denied = rpc("tools/call", {"name": "status", "arguments": {"repo": "test"}})
            assert denied["result"]["isError"]

            admission.unlink()
            assert process.wait(timeout=3) != 0
            assert '"code":"revoked"' in (work / "reader.log").read_text()
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    print("admitted reader: pinned queries, source evidence, publication isolation and lease revocation PASS")

    # Keep stdin open and idle past expiry: shutting down a Tokio stdio read
    # must not leave a blocking read thread preventing process termination.
    view["issued_at"] = int(time.time())
    view["expires_at"] = view["issued_at"] + 4
    with admission.open("x") as stream:
        os.fchmod(stream.fileno(), 0o600)
        json.dump(view, stream)
    with (work / "reader-expiry.log").open("w") as log:
        process = subprocess.Popen(["chaosbox", "reader", "--admission", str(admission)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
        try:
            assert process.stdin is not None and process.stdout is not None
            process.stdin.write(b'{"jsonrpc":"2.0","id":1,"method":"initialize"}\n')
            process.stdin.flush()
            ready, _, _ = select.select([process.stdout], [], [], 3)
            assert ready, "expiry fixture failed to initialize"
            assert json.loads(process.stdout.readline())["result"]["serverInfo"]["name"] == "chaosbox-read-view"
            assert process.wait(timeout=5) == 0, (work / "reader-expiry.log").read_text()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if process.stdin is not None:
                process.stdin.close()
            if process.stdout is not None:
                process.stdout.close()
    print("admitted reader: real-pipe idle expiry settles process PASS")

    view["issued_at"] = int(time.time())
    view["expires_at"] = view["issued_at"] + 300
    admission.write_text(json.dumps(view))
    with (work / "reader-backpressure.log").open("w") as log:
        process = subprocess.Popen(["chaosbox", "reader", "--admission", str(admission)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log)
        try:
            assert process.stdin is not None and process.stdout is not None
            process.stdin.write(b'{"jsonrpc":"2.0","id":1,"method":"initialize"}\n')
            process.stdin.flush()
            ready, _, _ = select.select([process.stdout], [], [], 3)
            assert ready, "backpressure fixture failed to initialize"
            assert json.loads(process.stdout.readline())["result"]["serverInfo"]["name"] == "chaosbox-read-view"
            # Fill a small real kernel pipe with control replies followed by
            # an export. Keep stdin open and leave stdout completely undrained.
            capacity = fcntl.fcntl(process.stdout.fileno(), fcntl.F_SETPIPE_SZ, 4096)
            ping = b'{"jsonrpc":"2.0","id":1,"method":"ping"}\n'
            process.stdin.write(ping)
            process.stdin.flush()
            ready, _, _ = select.select([process.stdout], [], [], 3)
            assert ready, "backpressure ping failed"
            reply = process.stdout.readline()
            assert json.loads(reply)["id"] == 1
            count = capacity // len(reply)
            payload = ping * count
            payload += b'{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"export","arguments":{"repo":"syntax","kind":"nodes"}}}\n'
            assert len(payload) <= fcntl.fcntl(process.stdin.fileno(), fcntl.F_GETPIPE_SZ)
            assert os.write(process.stdin.fileno(), payload) == len(payload)
            deadline = time.monotonic() + 3
            unread = array.array("i", [0])
            while time.monotonic() < deadline:
                fcntl.ioctl(process.stdout.fileno(), termios.FIONREAD, unread)
                if unread[0] >= count * len(reply):
                    break
                time.sleep(0.01)
            # A <= PIPE_BUF write is atomic: the next response can be blocked
            # with a few bytes still free. All pings fit, while the following
            # export cannot fit in the remaining sub-ping-sized space.
            assert unread[0] >= count * len(reply), "fixture did not establish real output backpressure"
            assert capacity - unread[0] < len(reply)
            assert process.poll() is None
            admission.unlink()
            assert process.wait(timeout=3) != 0
            assert '"code":"revoked"' in (work / "reader-backpressure.log").read_text()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if process.stdin is not None:
                process.stdin.close()
            if process.stdout is not None:
                process.stdout.close()
    print("admitted reader: real-pipe backpressure revocation settles process PASS")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
