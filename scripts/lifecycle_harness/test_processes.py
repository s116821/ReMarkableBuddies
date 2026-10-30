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
from unittest.mock import patch
from processes import start_identity, stop, child


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
        try:
            hello = self.next()
            if hello["pid"] != self.process.pid or hello["role"] != "guard":
                raise AssertionError("guard identity mismatch")
            self.next("boot")
        except BaseException:
            self.close()
            raise

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
            try:
                self.process.stdin.write('{"op":"quit"}\n')
                self.process.stdin.flush()
            except (BrokenPipeError, OSError):
                pass
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
        expected = {"owner", "config", "stock-baseline", "user-config", "payload", "session", "session.partial", "config.partial", "stop-owned-children", "supervisor-recovery"}
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

    def test_constructor_refusal_cleans_owned_process_and_directory(self):
        captured = []
        def refuse(session, event=None):
            captured.append(session)
            raise AssertionError("injected startup refusal")
        with patch.object(Session, "next", refuse):
            with self.assertRaisesRegex(AssertionError, "injected startup refusal"):
                Session()
        self.assertEqual(len(captured), 1)
        self.assertIsNotNone(captured[0].process.poll())
        self.assertFalse(captured[0].root.exists())
        self.assertTrue(captured[0].watchdog.finished.is_set())

    def test_dead_child_identity_is_not_live_with_retained_handle(self):
        p, identity = child(self.session.root, self.session.token + "-retained", "runtime")
        try:
            self.assertEqual(start_identity(p.pid), identity["os_start"])
            stop(p)
            self.assertIsNone(start_identity(p.pid))
        finally:
            stop(p)
            for pipe in (p.stdin, p.stdout, p.stderr):
                pipe.close()

    def test_child_malformed_absent_and_missing_identity_cleanup(self):
        real_popen = subprocess.Popen
        for failure in ("malformed", "absent", "identity"):
            with self.subTest(failure=failure):
                children = []
                def create(*args, **kwargs):
                    p = real_popen(*args, **kwargs)
                    children.append(p)
                    return p
                with patch("processes.subprocess.Popen", create):
                    if failure == "identity":
                        with patch("processes.start_identity", return_value=None):
                            with self.assertRaises(RuntimeError):
                                child(self.session.root, self.session.token + "-fixture", "runtime")
                    else:
                        effect = queue.Empty if failure == "absent" else None
                        with patch("processes.read_line", return_value="{}", side_effect=effect):
                            with self.assertRaises((RuntimeError, queue.Empty)):
                                child(self.session.root, self.session.token + "-fixture", "runtime")
                self.assertEqual(len(children), 1)
                self.assertIsNotNone(children[0].poll())
                self.assertTrue(all(p.closed for p in (children[0].stdin, children[0].stdout, children[0].stderr)))

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
        started = time.monotonic()
        self.session.send("disable", "rollback-boundary")
        r = self.session.next("recovery-failed")
        self.assertGreaterEqual(time.monotonic() - started, 5)
        self.assertLess(time.monotonic() - started, 8)
        self.assertEqual(r["state"], "RecoveryFailed")
        self.assertNotEqual(r["config_hash"], r["baseline_hash"])
        self.assertTrue(r["injected_alive"])
        self.session.send("activate", "refused")

    def test_guard_death_surviving_supervisor_restores_stock(self):
        self.session.activate()
        self.session.process.kill()
        self.session.process.wait(timeout=3)
        supervisor = next(x for x in self.session.identities if x["role"] == "supervisor")
        self.assertEqual(start_identity(supervisor["pid"]), supervisor["os_start"])
        deadline = time.monotonic() + 6
        receipt = self.session.root / "supervisor-recovery"
        while not receipt.exists():
            self.assertLess(time.monotonic(), deadline, "no Supervisor recovery completion")
            threading.Event().wait(0.01)
        r = json.loads(receipt.read_bytes())
        self.assertEqual(r["state"], "DisabledForSession")
        self.assertEqual(r["config_hash"], r["baseline_hash"])
        self.assertTrue(r["unrelated_preserved"] and r["injected_gone"])
        self.assertEqual((self.session.root / "config").read_bytes(), b"stock\n")
        self.assertEqual((self.session.root / "user-config").read_bytes(), b"do-not-touch\n")
        self.assertTrue((self.session.root / "payload").exists())
        self.assertEqual(start_identity(r["stock"]["pid"]), r["stock"]["os_start"])
        self.session.identities.append(r["stock"])
        self.assertFalse(any((self.session.root / x).exists() for x in ("session", "config.partial", "session.partial")))

    def test_guard_death_at_every_activation_barrier(self):
        self.session.close()
        for barrier in ("armed", "partial", "prepared", "applied", "stock-stopped"):
            with self.subTest(barrier=barrier):
                self.session = Session()
                self.session.send("preflight")
                self.session.send("arm", "armed")
                if barrier == "partial":
                    self.session.send("partial", "partial-applied")
                if barrier in ("prepared", "applied", "stock-stopped"):
                    self.session.send("prepare", "prepared")
                if barrier in ("applied", "stock-stopped"):
                    self.session.send("apply", "applied")
                if barrier == "stock-stopped":
                    self.session.send("stop-stock", "activated")
                self.session.process.kill()
                self.session.process.wait(timeout=3)
                deadline = time.monotonic() + 6
                receipt = self.session.root / "supervisor-recovery"
                while not receipt.exists():
                    self.assertLess(time.monotonic(), deadline)
                    threading.Event().wait(0.01)
                result = json.loads(receipt.read_bytes())
                self.assertEqual(result["state"], "DisabledForSession")
                self.assertTrue(result["injected_gone"] and result["unrelated_preserved"])
                self.assertEqual(result["config_hash"], result["baseline_hash"])
                self.assertEqual((self.session.root / "config").read_bytes(), b"stock\n")
                self.assertEqual(start_identity(result["stock"]["pid"]), result["stock"]["os_start"])
                self.session.identities.append(result["stock"])
                self.assertFalse(any((self.session.root / x).exists() for x in ("session", "config.partial", "session.partial")))
                self.session.close()
        self.session = Session()

    def test_lost_prepare_and_apply_ack_restore_before_cleanup(self):
        self.session.close()
        for operation in ("lose-prepare-ack", "lose-apply-ack", "lose-restore-ack"):
            with self.subTest(operation=operation):
                self.session = Session()
                self.session.send("preflight")
                self.session.send("arm", "armed")
                if operation == "lose-apply-ack":
                    self.session.send("prepare", "prepared")
                if operation == "lose-restore-ack":
                    result = self.session.send("activate", "activated")
                    self.session.send("ready", scope=result["scope"])
                self.session.send(operation, "ack-pipe-lost")
                deadline = time.monotonic() + 6
                receipt = self.session.root / "supervisor-recovery"
                while not receipt.exists():
                    self.assertLess(time.monotonic(), deadline)
                    threading.Event().wait(0.01)
                result = json.loads(receipt.read_bytes())
                self.assertEqual(result["state"], "DisabledForSession")
                self.assertEqual(result["restore_runs"], 1)
                self.assertTrue(result["injected_gone"] and result["unrelated_preserved"])
                identity = result["injected_identity"]
                self.assertIsNotNone(identity)
                self.assertNotEqual(start_identity(identity["pid"]), identity["os_start"])
                self.assertEqual((self.session.root / "config").read_bytes(), b"stock\n")
                self.assertEqual(result["config_hash"], result["baseline_hash"])
                self.assertEqual(start_identity(result["stock"]["pid"]), result["stock"]["os_start"])
                self.session.identities.extend([identity, result["stock"]])
                self.session.close()
        self.session = Session()

    def test_shared_restore_idempotent_and_late_writers_fenced(self):
        scope = self.session.activate()
        for field in ("nonce", "process"):
            refused = self.session.send("foreign-restore", "restore-refused", field=field)
            self.assertTrue(refused["refused"])
            self.assertEqual((self.session.root / "config").read_bytes(), b"injected\n")
            self.assertTrue(refused["injected_alive"])
        restored = self.session.send("restore-race", "restored")
        first, second = restored["receipts"]
        self.assertEqual(first, second)
        self.assertEqual(first["scope"], scope)
        self.assertEqual(first["restore_runs"], 1)
        self.assertTrue(first["injected_gone"])
        self.session.restored(restored)
        late = self.session.send("late-apply", "late-refused")
        self.assertTrue(late["refused"] and late["partial_refused"])
        self.assertEqual((self.session.root / "config").read_bytes(), b"stock\n")
        self.assertFalse(any((self.session.root / x).exists() for x in ("session", "config.partial", "session.partial")))

    def test_supervisor_exit_during_restore_rpc_uses_independent_fallback(self):
        self.session.activate()
        self.session.restored(self.session.send("fault-restore-exit", "restored"))
        self.assertIsNone(start_identity(next(x for x in self.session.identities
                                             if x["role"] == "supervisor")["pid"]))

    def test_publication_failure_is_uncertain_without_restore_replay(self):
        self.session.activate()
        receipt = self.session.root / "supervisor-recovery"
        receipt.mkdir()  # Real filesystem replacement failure, not a mocked receipt.
        try:
            result = self.session.send("disable", "recovery-failed")
            self.assertEqual(result["state"], "RecoveryFailed")
            self.assertEqual((self.session.root / "config").read_bytes(), b"stock\n")
            self.assertFalse(result["injected_alive"])
            self.assertTrue(receipt.is_dir())
            self.assertFalse(any(p.name.startswith("supervisor-recovery.partial.")
                                 for p in self.session.root.iterdir()))
            first = self.session.send("restore-status")["receipt"]
            second = self.session.send("restore-status")["receipt"]
            self.assertEqual(first, second)
            self.assertEqual(first["state"], "RecoveryFailed")
            self.assertEqual(first["reason"], "receipt-publication-failed")
            self.assertEqual(first["restore_runs"], 1)
            self.assertEqual(start_identity(first["stock"]["pid"]), first["stock"]["os_start"])
            self.session.send("activate", "refused")
        finally:
            receipt.rmdir()

    def test_lost_rpc_with_live_supervisor_refuses_takeover(self):
        self.session.activate()
        result = self.session.send("fault-restore-live", "recovery-failed")
        self.assertTrue(result["supervisor_alive"] and result["injected_alive"])
        self.assertFalse(result["stock_alive"])
        self.assertEqual(result["state"], "RecoveryFailed")
        self.assertEqual((self.session.root / "config").read_bytes(), b"injected\n")
        self.assertFalse((self.session.root / "supervisor-recovery").exists())
        self.session.send("activate", "refused")

    def test_cached_completion_refuses_dead_stock_without_second_restore(self):
        self.session.activate()
        restored = self.session.send("restore-race", "restored")
        first = restored["receipts"][0]
        result = self.session.send("cached-stock-loss", "cached-restoration")
        receipt = result["receipt"]
        self.assertEqual(receipt["state"], "RecoveryFailed")
        self.assertEqual(receipt["reason"], "cached-postcondition-lost")
        self.assertEqual(receipt["stock"], first["stock"])
        self.assertEqual(receipt["restore_runs"], 1)
        self.assertIsNone(start_identity(receipt["stock"]["pid"]))
        self.assertFalse(result["stock_alive"])
        self.assertEqual((self.session.root / "config").read_bytes(), b"stock\n")
        self.session.send("activate", "refused")

    def test_simultaneous_loss_cold_boot_ignores_leftover_payload(self):
        old_scope = self.session.activate()
        self.session.process.stdin.write('{"op":"kill-both"}\n')
        self.session.process.stdin.flush()
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
