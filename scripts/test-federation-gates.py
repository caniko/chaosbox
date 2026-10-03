#!/usr/bin/env python3
"""Small subprocess regressions for the federation qualification drivers."""
import argparse
from pathlib import Path
import os
import runpy
import subprocess
import sys
import tempfile
import threading
import time
import unittest

REPLY = '{"result":{"status":"ok"}}\n'


class RustTestCount(unittest.TestCase):
    def invoke(self, summary, exit_code=0):
        with tempfile.TemporaryDirectory(prefix="federation-gate-", dir=os.environ.get("TMPDIR")) as directory:
            root = Path(directory)
            cargo = root / "cargo"
            cargo.write_text(
                '#!/usr/bin/env bash\n'
                'printf "%s\\n" "$TEST_SUMMARY"\n'
                'exit "$TEST_EXIT"\n'
            )
            cargo.chmod(0o700)
            return subprocess.run(
                ["bash", str(TYPEDB_GATE), str(root / "test.log")],
                env=dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                         TEST_SUMMARY=summary, TEST_EXIT=str(exit_code)),
                text=True, capture_output=True, timeout=5,
            )

    def test_exactly_one_executed_test_passes(self):
        result = self.invoke("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_zero_tests_cannot_pass(self):
        result = self.invoke("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;")
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_ignored_test_cannot_pass(self):
        result = self.invoke("test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out;")
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_multiple_tests_cannot_pass(self):
        result = self.invoke("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;")
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_failed_cargo_cannot_pass_despite_summary(self):
        result = self.invoke("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;", 7)
        self.assertNotEqual(result.returncode, 0, result.stdout)


class OneShotDeadline(unittest.TestCase):
    def invoke(self, body, timeout=0.3):
        process = subprocess.Popen(
            [sys.executable, "-u", "-c", f"import sys,time,os\n{body}"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
        # Also bound a future regression to a blocking pipe read.
        watchdog = threading.Timer(2, lambda: process.kill() if process.poll() is None else None)
        watchdog.start()
        started = time.monotonic()
        try:
            return READ_ONE_SHOT(process, timeout=timeout)
        finally:
            watchdog.cancel()
            if process.poll() is None:
                process.kill()
            process.wait(timeout=2)
            process.stdout.close()
            self.assertLess(time.monotonic() - started, 1.5, "read escaped its deadline")

    def test_partial_frame_cannot_escape_the_deadline(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            self.invoke("sys.stdout.write('{'); sys.stdout.flush(); time.sleep(30)")

    def test_fragmented_complete_frame_passes(self):
        self.invoke("sys.stdout.write('{'); sys.stdout.flush(); time.sleep(0.02); "
                    f"sys.stdout.write({REPLY[1:]!r})")

    def test_complete_frame_with_open_pipe_times_out(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            self.invoke(f"sys.stdout.write({REPLY!r}); sys.stdout.flush(); time.sleep(30)")

    def test_response_without_exit_times_out(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            self.invoke(f"sys.stdout.write({REPLY!r}); sys.stdout.flush(); os.close(1); time.sleep(30)")

    def test_read_and_exit_share_one_deadline(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            self.invoke(f"time.sleep(0.18); sys.stdout.write({REPLY!r}); "
                        "sys.stdout.flush(); os.close(1); time.sleep(0.18)")

    def test_second_frame_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.invoke(f"sys.stdout.write({(REPLY * 2)!r})")

    def test_unterminated_frame_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.invoke(f"sys.stdout.write({REPLY[:-1]!r})")

    def test_response_with_failed_exit_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.invoke(f"sys.stdout.write({REPLY!r}); sys.exit(7)")

    def test_oversized_response_is_rejected(self):
        with self.assertRaises(AssertionError):
            self.invoke("sys.stdout.write('x' * (256 * 1024 + 1)); sys.stdout.flush(); time.sleep(30)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssh-gate", type=Path, default=Path(__file__).with_name("test-federation-ssh.py"))
    parser.add_argument("--typedb-gate", type=Path, default=Path(__file__).with_name("test-typedb-federation.sh"))
    args, remaining = parser.parse_known_args()
    TYPEDB_GATE = args.typedb_gate.resolve()
    READ_ONE_SHOT = runpy.run_path(str(args.ssh_gate))["read_one_shot"]
    unittest.main(argv=[sys.argv[0], *remaining])
