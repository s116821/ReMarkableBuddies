"""Linux-only worker foundation checks; no service manager/device operations."""
import os
import errno
import fcntl
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest
import uuid


class WorkerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e0t-foundation-")
        self.parent = Path(self.temp.name)
        self.nonce = uuid.uuid4().hex
        self.root = self.parent / ("buddy-e0t-" + self.nonce)
        self.root.mkdir(mode=0o700)
        (self.root / "owner").write_text(self.nonce)
        self.binary = self.parent / "helper"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                        '-DE0T_NONCE="' + self.nonce + '"',
                        '-DE0T_RUNTIME_PARENT="' + str(self.parent) + '"',
                        str(Path(__file__).with_name("helper.c")), "-o", str(self.binary)], check=True)

    def tearDown(self):
        self.temp.cleanup()

    def run_helper(self, role, generation, expected=0):
        result = subprocess.run([str(self.binary), role, str(generation)], capture_output=True, timeout=3)
        self.assertEqual(result.returncode, expected, result.stderr)
        return result

    def controlled_exit(self, role, message=b"X"):
        deadline = time.monotonic() + 3
        while True:
            try:
                fd = os.open(self.root / ("control-" + role), os.O_WRONLY | os.O_NONBLOCK)
                break  # An actual reader is present, rather than presumed ready.
            except OSError as error:
                if error.errno != errno.ENXIO or time.monotonic() >= deadline:
                    raise
                time.sleep(0.01)  # Bounded polling pace, not completion evidence.
        try:
            self.assertEqual(os.write(fd, message), 1)
        finally:
            os.close(fd)

    def test_fixed_role_case_and_actor_refusal(self):
        for role, generation in (("other", 1), ("claim", 1), ("separate", 9), ("guard", 6), ("cleanup", 1)):
            self.run_helper(role, generation, 90)
        self.assertEqual(list(self.root.iterdir()), [self.root / "owner"])

    def test_profile_reports_actual_kernel_limits_without_owned_effects(self):
        result = subprocess.run([str(self.binary), "--profile"], capture_output=True, timeout=3, check=True)
        profile = json.loads(result.stdout)
        expected = {"as": 8388608, "stack": 524288, "data": 1048576, "file": 2048, "core": 0, "cpu": 2}
        self.assertEqual(profile["limits"], {key: {"soft": value, "hard": value} for key, value in expected.items()})
        self.assertLessEqual(profile["initial_mapped_bytes"] + profile["headroom_bytes"], expected["as"])
        self.assertFalse(profile["aggregate_kernel_limit"])
        self.assertEqual(list(self.root.iterdir()), [self.root / "owner"])

    def test_manager_parent_has_low_soft_limit_and_explicit_child_hard_ceiling(self):
        result = subprocess.run([str(self.binary), "--profile-manager"], capture_output=True, timeout=3, check=True)
        profile = json.loads(result.stdout)
        self.assertTrue(profile["manager_parent"])
        self.assertEqual(profile["limits"]["as"], {"soft": 8388608, "hard": 20971520})
        self.assertFalse(profile["aggregate_kernel_limit"])
        self.assertEqual(list(self.root.iterdir()), [self.root / "owner"])

    def test_bad_address_limit_refuses_before_owned_effects(self):
        bad = self.parent / "bad-limit-helper"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-DE0T_AS_LIMIT=65536",
                        '-DE0T_NONCE="' + self.nonce + '"',
                        '-DE0T_RUNTIME_PARENT="' + str(self.parent) + '"',
                        str(Path(__file__).with_name("helper.c")), "-o", str(bad)], check=True)
        result = subprocess.run([str(bad), "fail-a", "1"], capture_output=True, timeout=3)
        self.assertEqual(result.returncode, 90)
        self.assertIn(b"insufficient initial mapping/headroom", result.stderr)
        self.assertEqual(list(self.root.iterdir()), [self.root / "owner"])

    def test_small_kernel_allocation_and_file_refusal_fixtures(self):
        fixture = self.parent / "resource-fixture-helper"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-DE0T_RESOURCE_TEST",
                        '-DE0T_NONCE="' + self.nonce + '"',
                        '-DE0T_RUNTIME_PARENT="' + str(self.parent) + '"',
                        str(Path(__file__).with_name("helper.c")), "-o", str(fixture)], check=True)
        for kind in ("allocation", "file"):
            result = subprocess.run([str(fixture), "--resource-fixture", kind], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, (kind + "-refused\n").encode())
        self.assertEqual((self.root / "resource-file-fixture").stat().st_size, 2048)

    def test_markers_identity_and_owned_record_bounds(self):
        self.run_helper("fail-a", 1)
        data = (self.root / "events-T1-fail-a").read_text()
        self.assertIn(self.nonce + " T1 fail-a pid=", data)
        self.assertIn("start=", data)
        self.assertIn(" marker\n", data)
        self.run_helper("norestart", 2, 42)
        self.assertIn("intentional-failure", (self.root / "events-T2-norestart").read_text())
        record = self.root / "events-T1-fail-b"
        record.write_bytes(b"x" * 2048)
        record.chmod(0o600)
        self.run_helper("fail-b", 1, 90)
        self.assertEqual(record.stat().st_size, 2048)

    def test_immutable_command_reads_disjoint_owned_case(self):
        case = self.root / "case"
        case.write_text("1")
        case.chmod(0o600)
        self.run_helper("norestart", "current", 42)
        case.write_text("2")
        self.run_helper("norestart", "current", 42)
        for number in (1, 2):
            self.assertEqual((self.root / f"events-T{number}-norestart").read_text().count("intentional-failure"), 1)
        case.write_text("9")
        self.run_helper("norestart", "current", 90)

    def test_owner_and_record_links_refuse(self):
        self.root.chmod(0o755)
        self.run_helper("fail-a", 1, 90)
        self.root.chmod(0o700)
        outside = self.parent / "outside"
        outside.write_text("untouched")
        record = self.root / "events-T1-fail-a"
        record.symlink_to(outside)
        self.run_helper("fail-a", 1, 90)
        self.assertEqual(outside.read_text(), "untouched")
        record.unlink()
        os.link(outside, record)
        self.run_helper("fail-a", 1, 90)
        self.assertEqual(outside.read_text(), "untouched")

    def test_fifo_substitution_and_record_lock_refuse_without_waiting(self):
        for filename, role, generation in (("owner", "fail-a", 1),
                                            ("case", "norestart", "current"),
                                            ("claim-T2", "claim", 2),
                                            ("events-T1-fail-a", "fail-a", 1)):
            with self.subTest(filename=filename):
                target = self.root / filename
                previous = target.read_bytes() if target.exists() else None
                target.unlink(missing_ok=True)
                os.mkfifo(target, 0o600)
                fd = os.open(target, os.O_RDWR | os.O_NONBLOCK)
                try:
                    self.run_helper(role, generation, 90)
                finally:
                    os.close(fd)
                    target.unlink()
                    if previous is not None:
                        target.write_bytes(previous)
        record = self.root / "events-T1-fail-b"
        record.write_bytes(b"")
        record.chmod(0o600)
        with record.open("rb") as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            self.run_helper("fail-b", 1, 90)
        self.assertEqual(record.read_bytes(), b"")

    def test_spent_claim_never_repeats_and_controlled_hold(self):
        self.run_helper("claim", 2, 42)
        claim = (self.root / "claim-T2").read_bytes()
        os.mkfifo(self.root / "control-claim", 0o600)
        worker = subprocess.Popen([str(self.binary), "claim", "2"], stderr=subprocess.PIPE)
        try:
            self.controlled_exit("claim")
            self.assertEqual(worker.wait(timeout=3), 0)
            self.assertEqual((self.root / "claim-T2").read_bytes(), claim)
            data = (self.root / "events-T2-claim").read_text()
            self.assertEqual(data.count("claim-spent"), 1)
            self.assertEqual(data.count("stock-standin"), 1)
        finally:
            if worker.poll() is None:
                worker.kill()
            worker.wait(timeout=3)
            worker.stderr.close()

    def test_barrier_ready_requires_explicit_control_and_repeated_ready_refuses(self):
        os.mkfifo(self.root / "control-barrier", 0o600)
        receiver = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        endpoint = self.parent / "barrier-socket"
        receiver.bind(str(endpoint))
        receiver.settimeout(0.15)
        worker = subprocess.Popen([str(self.binary), "barrier", "5"], stderr=subprocess.PIPE,
                                  env={**os.environ, "NOTIFY_SOCKET": str(endpoint)})
        try:
            with self.assertRaises(socket.timeout):
                receiver.recv(128)
            self.controlled_exit("barrier", b"R")
            receiver.settimeout(3)
            self.assertEqual(receiver.recv(128), b"READY=1")
            self.controlled_exit("barrier", b"R")
            self.assertEqual(worker.wait(timeout=3), 90)
            self.assertEqual((self.root / "events-T5-barrier").read_text().count("barrier-ready-sent"), 1)
        finally:
            if worker.poll() is None:
                worker.kill()
            worker.wait(timeout=3)
            worker.stderr.close()
            receiver.close()

    def test_manager_supplied_notify_context_and_missing_context_refusal(self):
        os.mkfifo(self.root / "control-notify", 0o600)
        self.run_helper("notify", 4, 90)
        endpoint = self.parent / "notify-socket"
        receiver = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        receiver.bind(str(endpoint))
        receiver.settimeout(3)
        worker = subprocess.Popen([str(self.binary), "notify", "4"], stderr=subprocess.PIPE,
                                  env={**os.environ, "NOTIFY_SOCKET": str(endpoint), "WATCHDOG_USEC": "3000000"})
        try:
            self.assertEqual(receiver.recv(128), b"READY=1\nWATCHDOG=1")
            self.controlled_exit("notify")
            self.assertEqual(worker.wait(timeout=3), 0)
        finally:
            if worker.poll() is None:
                worker.kill()
            worker.wait(timeout=3)
            worker.stderr.close()
            receiver.close()


if __name__ == "__main__":
    unittest.main()
