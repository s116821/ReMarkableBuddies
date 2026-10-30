"""Deterministic event tests. Clock deadlines never imply successful recovery."""
import unittest
from dataclasses import replace
from model import Lifecycle


class PolicyTests(unittest.TestCase):
    def active(self, ready=False):
        m = Lifecycle()
        self.assertTrue(m.request())
        self.assertTrue(m.arm())
        self.assertTrue(m.activate())
        if ready:
            self.assertTrue(m.ready(m.scope))
        return m

    def assert_restored(self, m):
        self.assertTrue(m.finish_restore())
        self.assertEqual(m.config, m.baseline)
        self.assertFalse(m.injected)
        self.assertEqual(m.unrelated, b"user-config\n")
        self.assertFalse(m.owned_files)
        self.assertIsNone(m.request())

    def test_mismatch_and_unavailable_guard_have_zero_activation(self):
        for mismatch in (True, False):
            m = Lifecycle()
            if mismatch:
                m.request(compatible=False)
            else:
                m.request()
                self.assertFalse(m.arm(available=False))
            self.assertEqual(m.attempts, 0)
            self.assertEqual(m.config, m.baseline)
            self.assertFalse(m.injected)

    def test_healthy_duplicate_disable(self):
        m = self.active(True)
        scope = m.scope
        self.assertEqual(m.request(), scope)
        self.assertFalse(m.activate())
        self.assertEqual(m.attempts, 1)
        m.fail()
        self.assert_restored(m)

    def test_all_stale_scope_components_rejected(self):
        m = self.active()
        variants = [replace(m.scope, boot="old"), replace(m.scope, generation=0),
                    replace(m.scope, nonce="old"), replace(m.scope, process="old")]
        for stale in variants:
            self.assertFalse(m.ready(stale))
        self.assertEqual(m.state, "Activating")
        self.assertTrue(m.ready(m.scope))
        m.advance(2)
        before = m.last_heartbeat
        for stale in variants:
            self.assertFalse(m.heartbeat(stale))
        self.assertEqual(m.last_heartbeat, before)
        m.advance(1)
        self.assertEqual(m.state, "RestoringStock")
        self.assert_restored(m)
        self.assertFalse(m.ready(m.scope))
        self.assertFalse(m.heartbeat(m.scope))

    def test_constructor_missing_ready_heartbeat_and_crash_loop(self):
        for failure in ("constructor", "missing", "heartbeat", "loop"):
            with self.subTest(failure=failure):
                m = self.active(failure == "heartbeat")
                if failure == "missing":
                    m.advance(5)
                elif failure == "heartbeat":
                    m.advance(3)
                else:
                    m.fail()
                self.assert_restored(m)
                for _ in range(4):
                    self.assertIsNone(m.request())
                    self.assertFalse(m.activate())
                self.assertEqual(m.attempts, 1)

    def test_supervisor_death_at_every_boundary(self):
        for state in ("Stock", "Preflight", "RecoveryArmed", "Activating", "Ready", "RestoringStock"):
            with self.subTest(state=state):
                m = Lifecycle()
                if state != "Stock":
                    m.request()
                if state not in ("Stock", "Preflight"):
                    m.arm()
                if state in ("Activating", "Ready", "RestoringStock"):
                    m.activate()
                if state == "Ready":
                    m.ready(m.scope)
                if state == "RestoringStock":
                    m.fail()
                m.death("supervisor")
                if m.state == "RestoringStock":
                    self.assert_restored(m)
                self.assertEqual(m.config, m.baseline)
                self.assertFalse(m.injected)

    def test_guard_alone_death_requires_recovery(self):
        m = self.active(True)
        m.death("guard")
        self.assertNotEqual(m.state, "Ready")
        self.assert_restored(m)

    def test_simultaneous_loss_disclosed_and_cold_boot_stock(self):
        m = self.active(True)
        m.death("both")
        self.assertEqual(m.state, "FailedUnprotected")
        self.assertTrue(m.injected)
        n = m.cold_boot()
        self.assertEqual(n.state, "Stock")
        self.assertFalse(n.injected)
        self.assertEqual(n.attempts, 0)
        self.assertFalse(n.ready(m.scope))
        self.assertFalse(n.heartbeat(m.scope))

    def test_interrupted_rollback_deadline_is_failure_not_completion(self):
        m = self.active(True)
        m.rollback_available = False
        m.fail()
        self.assertFalse(m.finish_restore())
        m.advance(5)
        self.assertEqual(m.state, "RecoveryFailed")
        self.assertTrue(m.injected)
        self.assertNotEqual(m.config, m.baseline)
        self.assertIsNone(m.request())

    def test_partial_files_management_preserves_unrelated(self):
        for operation in ("update", "uninstall", "disable"):
            with self.subTest(operation=operation):
                m = self.active()
                m.owned_files.add("interrupted-" + operation)
                m.fail()
                self.assert_restored(m)


if __name__ == "__main__":
    unittest.main()
