#!/usr/bin/env python3
"""Small subprocess regressions for the federation qualification drivers."""
import argparse
import os
import runpy
import socket
import subprocess
import sys
import threading
import time
import unittest
from pathlib import Path

REPLY = '{"result":{"status":"ok"}}\n'


class OneShotDeadline(unittest.TestCase):
    def invoke(self, body, timeout=0.3):
        # Interpreter startup in the VM has its own bound. Start the unchanged
        # protocol deadline only once the child is ready to execute the fixture.
        ready, child_ready = socket.socketpair()
        with ready, child_ready:
            descriptor = child_ready.fileno()
            process = subprocess.Popen(
                [sys.executable, "-u", "-c",
                 "import sys,time,os,socket\n"
                 f"ready = socket.socket(fileno={descriptor})\n"
                 "ready.sendall(b'R')\n"
                 "assert ready.recv(1) == b'G'\n"
                 "ready.close()\n" + body],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
                pass_fds=(descriptor,),
            )
            child_ready.close()
            watchdog = None
            started = None
            try:
                ready.settimeout(5)
                self.assertEqual(ready.recv(1), b"R", "fixture did not become ready")
                # Also bound a future regression to a blocking pipe read.
                watchdog = threading.Timer(2, lambda: process.kill() if process.poll() is None else None)
                watchdog.start()
                started = time.monotonic()
                ready.sendall(b"G")
                return READ_ONE_SHOT(process, timeout=timeout)
            finally:
                if watchdog is not None:
                    watchdog.cancel()
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=2)
                process.stdout.close()
                if started is not None:
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
    args, remaining = parser.parse_known_args()
    READ_ONE_SHOT = runpy.run_path(str(args.ssh_gate))["read_one_shot"]
    unittest.main(argv=[sys.argv[0], *remaining])
