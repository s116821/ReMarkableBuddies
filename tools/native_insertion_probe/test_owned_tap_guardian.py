"""Offline exact-source fake-evdev faults. Disposable Linux container only.

No tablet/native kernel input is opened. These controls establish source failure
ordering, not actual kernel echo, scheduler/crash safety or exclusive ownership.
"""
from pathlib import Path
import os
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
        for name, output in (("owned_tap_guardian.c", Path(cls.work.name) / "guardian"),
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
                 "close-foreign-contact", "supervisor-close", "close-takeover", "supervisor-close-stopped")
        for mode in modes:
            with self.subTest(mode=mode):
                result = subprocess.run([str(self.fixture), mode], text=True, capture_output=True, timeout=3)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(mode + " PASS", result.stdout)
                directory = Path(result.stdout.strip().split("temporary=", 1)[1]).resolve()
                self.assertEqual(directory.parent, Path("/tmp"))
                self.assertTrue(directory.name.startswith("guardian-fixture-"))
                shutil.rmtree(directory)


if __name__ == "__main__":
    unittest.main()
