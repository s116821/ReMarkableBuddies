"""Offline transport refusal and cleanup scope checks; no device access."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from open_document_host import Refused
from open_document_transport import SSHAdapter


class AdapterTests(unittest.TestCase):
    def adapter(self, root, result):
        calls = []
        def transport(arguments, script, timeout):
            calls.append(script)
            return result
        adapter = SSHAdapter([], {"remote_root": "/run/rmb-qt-probe-" + "1" * 32},
                             {"nonce": "1" * 32, "document_id": "fixture",
                              "boot_id": "boot", "tools": {}}, root, transport, b"")
        adapter.root_inode = "1 2"
        return adapter, calls

    def test_uncertain_open_is_never_replayed(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": True, "exit": None})
            with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
            with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
            self.assertEqual(len(calls), 1)
            self.assertIn('kill -KILL "$child"', calls[0])
            self.assertIn('tap 110 280', calls[0])
            self.assertTrue((Path(root) / "transport-001.json").exists())

    def test_cleanup_requires_owned_root_and_fixed_path(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": False, "exit": 0, "stdout": ""})
            adapter.restore_probe()
            self.assertFalse(calls)
            adapter.tap_attempted = True
            adapter.restore_probe()
            self.assertIn("'1 2'", calls[0])
            self.assertIn("'600 0 1'", calls[0])
            self.assertIn('test ! -L /tmp/reader-buddy-probe.png', calls[0])
            self.assertNotIn('rm -rf', calls[0])

    def test_stop_is_one_attempt_even_on_uncertainty(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": True, "exit": None})
            with self.assertRaises(Refused): adapter.stop_once(("boot", 2, 3), 100)
            adapter.stop_once(("boot", 2, 3), 100)
            self.assertEqual(len(calls), 1)

    def test_partial_diagnostic_read_waits_without_replaying_action(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            observations = iter(['{"creation_trial":', '{"creation_trial":{"returned":true}}'])
            adapter.call = lambda script, timeout: next(observations)
            captures = []
            adapter.raw_snapshot = lambda stage, cutoff, idle: captures.append((stage, cutoff, idle)) or {"captured": True}
            with patch("open_document_transport.time.sleep"):
                result = adapter.result_snapshot(("boot", 2, 3), 100)
            self.assertEqual(result, {"captured": True})
            self.assertEqual(captures, [("result", 100, False)])
            self.assertFalse(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)

    def test_persistent_partial_diagnostics_have_fixed_poll_limit(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            observations = []
            adapter.call = lambda script, timeout: observations.append(script) or '{"partial":'
            with patch("open_document_transport.time.sleep"), self.assertRaises(Refused):
                adapter.result_snapshot(("boot", 2, 3), 100)
            self.assertEqual(len(observations), 30)
            self.assertFalse(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)


if __name__ == "__main__":
    unittest.main()
