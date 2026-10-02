#!/usr/bin/env python3
"""Typecheck and load the scratch adapter through an explicit OpenCode V2 host."""
import argparse
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--opencode", required=True, type=Path)
parser.add_argument("--chaosbox-bin", required=True, type=Path)
parser.add_argument("--bun", default="bun")
args = parser.parse_args()
repo = Path(__file__).resolve().parent.parent
core = args.opencode.resolve() / "packages/core"
env = os.environ.copy()
env["CHAOSBOX_TEST_BIN"] = str(args.chaosbox_bin.resolve(strict=True))
with tempfile.TemporaryDirectory(prefix="chaosbox-scratch-verify-", dir=core / "test") as fixture:
    target = Path(fixture)
    plugin = target / "chaosbox-scratch"
    plugin.mkdir()
    for name in ["index.ts", "runtime.mjs", "runtime.d.mts", "package.json"]:
        shutil.copyfile(repo / "plugins/chaosbox-scratch" / name, plugin / name)
    shutil.copyfile(repo / "plugins/chaosbox-scratch/test/opencode.test.ts", plugin / "opencode.test.ts")
    helper = target / "chaosbox-compaction"
    helper.mkdir()
    shutil.copyfile(repo / "plugins/chaosbox-compaction/runtime.mjs", helper / "runtime.mjs")
    with tempfile.TemporaryDirectory(prefix="scratch-ledger-test-", dir="/data/scratch/tmp/opencode") as state:
        base = Path(state)
        root = base / "scratch"
        root.mkdir()
        env["CHAOSBOX_TEST_ROOT"] = str(root)
        env["CHAOSBOX_TEST_WORK"] = str(base / "custody")
        subprocess.run([args.bun, "run", "typecheck"], cwd=core, env=env, check=True)
        subprocess.run([args.bun, "test", str(plugin / "opencode.test.ts")], cwd=core, env=env, check=True)
