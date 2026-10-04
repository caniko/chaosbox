#!/usr/bin/env python3
"""Verify adapter typing and native loading against an explicit patched OpenCode checkout."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--opencode", required=True, type=Path)
parser.add_argument("--bun", default="bun")
parser.add_argument("--chaosbox-bin", required=True, type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
core = args.opencode.resolve() / "packages/core"
binary = args.chaosbox_bin.resolve(strict=True)
env = os.environ.copy()
env["PATH"] = str(Path(shutil.which(args.bun) or args.bun).resolve().parent) + os.pathsep + env["PATH"]
env["CHAOSBOX_TEST_BIN"] = str(binary)

# Keep dependencies inside the supplied checkout; remove only this verifier's
# fresh directory. No package installs, credential loading or service activation.
with tempfile.TemporaryDirectory(prefix="chaosbox-verify-", dir=core / "test") as fixture:
    target = Path(fixture)
    plugin = root / "plugins/chaosbox-compaction"
    for name in ["index.ts", "runtime.mjs", "runtime.d.mts", "package.json"]:
        shutil.copyfile(plugin / name, target / name)
    shutil.copyfile(plugin / "test/opencode.test.ts", target / "opencode.test.ts")
    scratch = os.environ.get("TMPDIR", "/data/scratch/tmp/opencode")
    with tempfile.TemporaryDirectory(prefix="chaosbox-custody-", dir=scratch) as work:
        env["CHAOSBOX_TEST_WORK"] = work
        subprocess.run([args.bun, "run", "typecheck"], cwd=core, env=env, check=True)
        subprocess.run([args.bun, "test", str(target / "opencode.test.ts")], cwd=core, env=env, check=True)
