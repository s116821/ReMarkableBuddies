"""Offline transport refusal and cleanup scope checks; no device access."""
from pathlib import Path
import base64
import json
import tempfile
import unittest
from unittest.mock import patch
from open_document_host import Refused
from open_document_transport import SSHAdapter


class AdapterTests(unittest.TestCase):
    def guardian(self, adapter, status):
        return {"nonce": adapter.profile["nonce"], "boot_id": adapter.profile["boot_id"], "pid": 24,
                "start_ticks": 33, "slot": 0, "tracking_id": 123, "native_point": [201, 1360],
                "lease_end_ms": 99000, "status": status, "exclusive_kernel_ownership": False}

    def test_guardian_ready_release_is_bound_and_go_is_single_shot(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            ready = (json.dumps(self.guardian(adapter, "guardian-ready-no-down")) + "\n").encode()
            release = (json.dumps(self.guardian(adapter, "guardian-released")) + "\n").encode()
            supervisor = (json.dumps(self.guardian(adapter, "normal-child-exit")) + "\n").encode()
            encode = lambda data: base64.b64encode(data).decode()
            output = iter([json.dumps({"ready": encode(ready), "setup": encode(b"setup bytes"), "selected_cutoff_ms": 100000, "uptime": encode(b"98.00 0.00\n")}),
                           json.dumps({"released": encode(release), "supervisor": encode(supervisor), "lease": encode(b"lease bytes")})])
            def transport(arguments, script, timeout):
                calls.append(script)
                return {"timeout": False, "exit": 0, "stdout": next(output), "stderr": ""}
            adapter.transport = transport
            adapter.open_once(("boot", 2, 3), 100)
            self.assertTrue(adapter.tap_released)
            self.assertEqual(len(calls), 2)
            self.assertIn('nohup "$root/owned_tap_guardian"', calls[0])
            self.assertNotIn('> "$root/tap.go"', calls[0])
            self.assertEqual(calls[1].count('> "$root/tap.go"'), 1)
            self.assertNotIn('/tmp/reader-buddy-probe.png', "".join(calls))
            with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
            self.assertEqual(len(calls), 2)

    def test_takeover_receipt_or_identity_change_never_admits_arm(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            for changed in (self.guardian(adapter, "conditional-takeover-released"),
                            self.guardian(adapter, "guardian-released") | {"nonce": "2" * 32},
                            self.guardian(adapter, "guardian-released") | {"lease_end_ms": 100001}):
                with self.assertRaises(Refused):
                    adapter.tap_receipt((json.dumps(changed) + "\n").encode(), "guardian-released", 100)
            with self.assertRaises(Refused): adapter.arm_once(None, None, {})
            self.assertFalse(calls)

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
            self.assertIn('nohup "$root/owned_tap_guardian" --tap', calls[0])
            self.assertNotIn('bounded "$root/owned_tap_guardian"', calls[0])
            self.assertTrue((Path(root) / "transport-001.json").exists())

    def test_guardian_sequence_never_mutates_shared_debug_image(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": False, "exit": 0, "stdout": ""})
            adapter.restore_probe()
            self.assertFalse(calls)
            adapter.tap_attempted = True
            adapter.restore_probe()
            self.assertFalse(calls)

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
