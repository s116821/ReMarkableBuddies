"""Owned real-process tests; every case has an independent 30s outer watchdog."""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid
from processes import start_identity, stop


class Session:
    def __init__(self, previous=None):
        self.token = previous.token if previous else uuid.uuid4().hex
        self.root = Path(tempfile.gettempdir()) / ("buddy-e0-" + self.token)
        if previous is None:
            self.root.mkdir()
            (self.root / "owner").write_text(self.token)
            (self.root / "config").write_bytes(b"stock\n")
            (self.root / "stock-baseline").write_bytes(b"stock\n")
            (self.root / "user-config").write_bytes(b"do-not-touch\n")
            (self.root / "payload").write_bytes(b"inert persistent fixture\n")
        self.deadline = time.monotonic() + 30
        self.events = []
        self.identities = []
        self.lines = queue.Queue()
        self.process = subprocess.Popen(
            [sys.executable, str(Path(__file__).with_name("processes.py")),
             "guard", str(self.root), self.token + "-" + uuid.uuid4().hex], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def read():
            for line in self.process.stdout:
                self.lines.put(json.loads(line))
            self.lines.put(None)
        threading.Thread(target=read, daemon=True).start()
        self.watchdog = threading.Timer(30, self.process.kill)
        self.watchdog.daemon = True
        self.watchdog.start()
        hello = self.next()
        if hello["pid"] != self.process.pid or hello["role"] != "guard":
            raise AssertionError("guard identity mismatch")
        self.next("boot")

    def next(self, event=None):
        value = self.lines.get(timeout=max(0.001, self.deadline - time.monotonic()))
        if value is None:
            raise AssertionError("guard exited before observed completion")
        self.events.append(value)
        self.identities.extend(x for x in value.get("identities", []) if x not in self.identities)
        if event is not None and value.get("event") != event:
            raise AssertionError((event, value))
        return value

    def send(self, op, event=None, **fields):
        self.process.stdin.write(json.dumps({"op": op, **fields}) + "\n")
        self.process.stdin.flush()
        return self.next(event or op)

    def activate(self, ready=True):
        self.send("preflight")
        self.send("arm", "armed")
        result = self.send("activate", "activated")
        if ready:
            assert self.send("ready", scope=result["scope"])["accepted"]
        return result["scope"]

    def restored(self, receipt):
        assert receipt["state"] == "DisabledForSession", receipt
        assert receipt["config_hash"] == receipt["baseline_hash"], receipt
        assert receipt["stock_alive"] and not receipt["injected_alive"], receipt
        assert receipt["unrelated_preserved"] and not receipt["owned_remaining"], receipt
        assert (self.root / "config").read_bytes() == b"stock\n"
        assert (self.root / "user-config").read_bytes() == b"do-not-touch\n"
        assert (self.root / "payload").exists()
        stock_identity = receipt["identities"][-1]
        assert stock_identity["role"] == "runtime"
        assert start_identity(stock_identity["pid"]) == stock_identity["os_start"]
        assert self.send("activate", "refused")["attempts"] <= 1

    def close(self, remove=True):
        self.watchdog.cancel()
        if self.process.poll() is None:
            self.process.stdin.write('{"op":"quit"}\n')
            self.process.stdin.flush()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                stop(self.process)
        else:
            self.process.wait()
        # Runner recovery is distinct from guard success and never counted as such.
        # Only the token-owned directory stop signal is used for orphan cleanup.
        (self.root / "stop-owned-children").write_text(self.token)
        end = time.monotonic() + 3
        while any(start_identity(x["pid"]) == x["os_start"] for x in self.identities):
            if time.monotonic() >= end:
                raise AssertionError("owned child survived cleanup")
            threading.Event().wait(0.01)
        # Explicit flat allowlist; unknown files preserve the directory for inspection.
        expected = {"owner", "config", "stock-baseline", "user-config", "payload", "session", "session.partial", "config.partial", "stop-owned-children"}
        actual = {p.name for p in self.root.iterdir()}
        if not actual <= expected:
            raise AssertionError("unexpected files; preserving evidence")
        if remove:
            for name in actual:
                (self.root / name).unlink()
            self.root.rmdir()
        for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
            pipe.close()


class RealProcessTests(unittest.TestCase):
    def setUp(self):
        self.session = Session()

    def tearDown(self):
        self.session.close()

    def test_mismatch_zero_replacement(self):
        r = self.session.send("preflight", compatible=False)
        self.assertEqual(r["attempts"], 0)
        self.assertTrue(r["stock_alive"])
        self.assertEqual(r["config_hash"], r["baseline_hash"])
        self.session.send("activate", "refused")

    def test_healthy_duplicate_nonce_and_disable(self):
        scope = self.session.activate()
        stale = dict(scope, nonce="old")
        self.assertFalse(self.session.send("heartbeat", scope=stale)["accepted"])
        self.assertTrue(self.session.send("heartbeat", scope=scope)["accepted"])
        self.assertEqual(self.session.send("activate", "refused")["attempts"], 1)
        self.session.restored(self.session.send("disable", "restored"))

    def test_missing_readiness_autonomous_restore(self):
        self.session.activate(False)
        self.session.restored(self.session.next("restored"))

    def test_lost_heartbeat_autonomous_restore(self):
        self.session.activate()
        self.session.restored(self.session.next("restored"))

    def test_constructor_exit_does_not_restart_injected(self):
        self.session.activate(False)
        self.session.send("kill-runtime", "runtime-stopped")
        self.session.restored(self.session.next("restored"))

    def test_supervisor_death_at_all_barriers(self):
        # Each fresh directory is a new simulated boot, never a retry in one session.
        self.session.close()
        for barrier in ("stock", "preflight", "armed", "partial", "activating", "ready", "rollback"):
            with self.subTest(barrier=barrier):
                self.session = Session()
                if barrier != "stock":
                    self.session.send("preflight")
                if barrier not in ("stock", "preflight"):
                    self.session.send("arm", "armed")
                if barrier == "partial":
                    self.session.send("partial", "partial-applied")
                if barrier in ("activating", "ready", "rollback"):
                    result = self.session.send("activate", "activated")
                    if barrier != "activating":
                        self.session.send("ready", scope=result["scope"])
                if barrier == "rollback":
                    self.session.send("begin-rollback", "rollback-boundary")
                self.session.process.stdin.write('{"op":"kill-supervisor"}\n')
                self.session.process.stdin.flush()
                self.session.restored(self.session.next("restored"))
                self.session.close()
        self.session = Session()

    def test_interrupted_rollback_is_observed_failure(self):
        self.session.activate()
        self.session.send("block-rollback", "rollback-blocked")
        r = self.session.send("disable", "recovery-failed")
        self.assertEqual(r["state"], "RecoveryFailed")
        self.assertNotEqual(r["config_hash"], r["baseline_hash"])
        self.assertTrue(r["injected_alive"])
        self.session.send("activate", "refused")

    def test_guard_death_discloses_no_protected_success(self):
        self.session.activate()
        self.session.process.kill()
        self.session.process.wait(timeout=3)
        supervisor = next(x for x in self.session.identities if x["role"] == "supervisor")
        self.assertEqual(start_identity(supervisor["pid"]), supervisor["os_start"])
        # Baseline is not restored merely by guard death; report limitation honestly.
        self.assertEqual((self.session.root / "config").read_bytes(), b"injected\n")
        with self.assertRaises(AssertionError):
            self.session.next("restored")

    def test_simultaneous_loss_cold_boot_ignores_leftover_payload(self):
        old_scope = self.session.activate()
        self.session.process.kill()
        self.session.process.wait(timeout=3)
        old = self.session
        old.close(remove=False)
        self.assertEqual((old.root / "config").read_bytes(), b"injected\n")
        self.assertTrue((old.root / "session").exists())
        self.session = Session(previous=old)
        boot = self.session.events[-1]
        self.assertEqual(boot["state"], "Stock")
        self.assertEqual(boot["attempts"], 0)
        self.assertFalse(boot["injected_alive"])
        self.assertEqual(boot["config_hash"], boot["baseline_hash"])
        self.assertFalse(self.session.send("ready", scope=old_scope)["accepted"])
        self.assertTrue((self.session.root / "payload").exists())

    def test_partial_update_uninstall_preserves_baseline(self):
        self.session.close()
        for operation in ("update", "uninstall"):
            self.session = Session()
            self.session.send("preflight")
            self.session.send("arm", "armed")
            self.session.send("partial", "partial-applied")
            self.session.restored(self.session.send(operation, "restored"))
            self.session.close()
        self.session = Session()


if __name__ == "__main__":
    unittest.main()
