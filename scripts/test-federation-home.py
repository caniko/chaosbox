#!/usr/bin/env python3
"""Exercise HM-rendered config with real OpenSSH and the federation CLI."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile


def verify(evaluation, chaosbox):
    with tempfile.TemporaryDirectory(prefix="chaosbox-federation-home-", dir=os.environ.get("TMPDIR")) as directory:
        root = Path(directory)
        ssh_config = root / "ssh-config"
        ssh_config.write_text(evaluation["sshConfig"])
        alias = evaluation["client"]["peers"][0]["destination"]
        effective = subprocess.run(
            ["ssh", "-G", "-F", str(ssh_config), alias],
            check=True, text=True, capture_output=True,
        ).stdout.splitlines()
        settings = {}
        for line in effective:
            key, value = line.split(" ", 1)
            settings.setdefault(key, []).append(value)
        assert settings["hostname"] == ["192.0.2.10"]
        assert settings["port"] == ["2222"]
        assert settings["user"] == ["dejana"]
        assert settings["identityfile"] == ["/run/agenix/can-chaosbox-query"]
        assert settings["identityagent"] == ["none"]
        assert settings["controlmaster"] == ["false"]
        # OpenSSH omits this field from -G output when the path is disabled.
        assert settings.get("controlpath", ["none"]) == ["none"]
        assert settings["stricthostkeychecking"] == ["true"]

        # A closed loopback port exercises peer failure without contacting a host.
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        ssh_config.write_text(evaluation["sshConfig"].replace("192.0.2.10", "127.0.0.1"))
        provider = evaluation["provider"]
        bundle = root / "bundle.json"
        bundle.write_text(json.dumps({
            "version": 1, "scope": provider["policy"]["identity"]["scope"],
            "records": [], "assessments": [], "coverage": [],
        }))
        provider["backend"]["path"] = str(bundle)
        provider_file = root / "provider.json"
        provider_file.write_text(json.dumps(provider))
        client = evaluation["client"]
        client["local"] = str(provider_file)
        client["peers"][0].update(ssh_config=str(ssh_config), port=port)
        client_file = root / "client.json"
        client_file.write_text(json.dumps(client))
        project = next(iter(provider["policy"]["projects"]))
        packet = json.loads(subprocess.run(
            [chaosbox, "federation", "--config", str(client_file), "context",
             "--repo", project, "--", "project constraints"],
            check=True, text=True, capture_output=True, timeout=15,
        ).stdout)
        assert packet["project"] == project
        assert packet["sources"][0]["identity"] == provider["policy"]["identity"]
        assert packet["sources"][0]["error"] is None
        assert packet["sources"][1]["error"] == "unavailable"
        assert packet["degraded"] is True
        print("HM JSON accepted by the CLI; dedicated SSH settings and local-first outage handling verified")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", required=True)
    parser.add_argument("--chaosbox", required=True)
    args = parser.parse_args()
    verify(json.loads(Path(args.evaluation).read_text()), args.chaosbox)
