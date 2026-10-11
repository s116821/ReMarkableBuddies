"""Offline exact-source fake-evdev faults. Disposable Linux container only.

No tablet/native kernel input is opened. These controls establish source failure
ordering, not actual kernel echo, scheduler/crash safety or exclusive ownership.
"""
from pathlib import Path
import os
import json
import shutil
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent


class GuardianFaultTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if os.name != "posix" or os.getuid() != 0:
            raise unittest.SkipTest("Disposable root Linux fixture container required")
        cls.work = tempfile.TemporaryDirectory(prefix="guardian-build-")
        cls.fixture = Path(cls.work.name) / "fixture"
        cls.guardian = Path(cls.work.name) / "guardian"
        for name, output in (("owned_tap_guardian.c", cls.guardian),
                             ("owned_tap_guardian_fixture.c", cls.fixture)):
            subprocess.run(["gcc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            str(HERE / name), "-o", str(output)], check=True, capture_output=True)

    @classmethod
    def tearDownClass(cls):
        cls.work.cleanup()

    def test_exact_source_fault_controls(self):
        modes = ("success", "partial-down-exact", "partial-down-no-point", "partial-up",
                 "dropped", "other-contact", "replaced", "point-drift", "expired",
                 "clock-failure", "source-loss", "pen-contact", "empty", "live-writer",
                 "malformed", "closing", "lease-replacement", "takeover",
                 "dead-writer", "persistent-lock", "expired-empty", "expired-owned",
                 "intent-expired", "intent-closing", "intent-replaced", "up-no-syn", "truncated-echo",
                 "delayed-go", "supervisor-normal", "supervisor-stopped",
                 "supervisor-after-release-crash", "supervisor-expired", "pidfd-fence",
                 "close-grammar", "foreign-point", "mixed-point", "close-foreign-echo",
                 "close-foreign-contact", "supervisor-close", "close-takeover", "supervisor-close-stopped",
                 "pidfd-open-unavailable", "pidfd-zero-denied",
                 "actual-os-release", "canonical-os-release-symlink",
                 "check-input-released", "check-input-x-unsupported", "check-input-contact")
        for mode in modes:
            with self.subTest(mode=mode):
                result = subprocess.run([str(self.fixture), mode], text=True, capture_output=True, timeout=3)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(mode + " PASS", result.stdout)
                if mode.startswith("check-input-"):
                    diagnostic = json.loads(result.stdout.splitlines()[0])
                    self.assertEqual(diagnostic["input_events"], 0)
                    self.assertEqual(diagnostic["event_reads"], 0)
                    self.assertFalse(diagnostic["tap_ready"])
                    checks = {item["name"]: item for item in diagnostic["checks"]}
                    if mode == "check-input-x-unsupported":
                        self.assertFalse(checks["x-slots"]["ok"])
                        self.assertEqual(checks["x-slots"]["errno"], 22)
                        self.assertTrue(checks["tracking-slots"]["ok"])
                    elif mode == "check-input-contact":
                        self.assertFalse(checks["touch-released"]["ok"])
                        self.assertEqual(diagnostic["tracking_ids"][1], 23)
                    else:
                        self.assertTrue(all(item["ok"] for item in diagnostic["checks"]))
                if mode.startswith("pidfd-") and mode != "pidfd-fence":
                    receipt = json.loads(result.stdout.splitlines()[0])
                    self.assertEqual(receipt["status"], "pidfd-self-check-unavailable")
                    self.assertFalse(receipt["pidfd_supported"])
                    self.assertEqual(receipt["input_events"], 0)
                    self.assertEqual(receipt["signal"], 0)
                    self.assertGreater(receipt["errno"], 0)
                directory = Path(result.stdout.strip().split("temporary=", 1)[1]).resolve()
                self.assertEqual(directory.parent, Path("/tmp"))
                self.assertTrue(directory.name.startswith("guardian-fixture-"))
                shutil.rmtree(directory)

    def test_read_only_pidfd_self_check(self):
        result = subprocess.run([str(self.guardian), "--check-pidfd"], text=True,
                                capture_output=True, timeout=3)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout), {
            "status": "pidfd-self-check-supported", "pidfd_supported": True,
            "errno": 0, "input_events": 0, "signal": 0})
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
