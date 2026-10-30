"""Actual shell immutable admission controls in a disposable Linux directory."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


class PrepublicationTests(unittest.TestCase):
    def test_change_after_snapshot_prevents_publisher_invocation(self):
        if os.name != "posix": self.skipTest("Linux shell fixture")
        # No image parsing occurs in this isolated transport-stage control.
        class Refused(ValueError): pass
        with patch.dict("sys.modules", {"open_document_host": SimpleNamespace(Refused=Refused, same_pixels=None)}):
            spec = importlib.util.spec_from_file_location("transport_fixture", Path(__file__).with_name("open_document_transport.py"))
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            receipt = {"remote_root": str(root), "files": {}}
            for name, count in (("runtime.files", 10), ("document-immutable.files", 14)):
                lines = []
                for i in range(count):
                    path = root / f"{name}-{i}"
                    path.write_bytes(b"original snapshot bytes")
                    lines.append(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + str(path) + "\n")
                manifest = root / name
                manifest.write_text("".join(lines))
                receipt["files"][name] = hashlib.sha256(manifest.read_bytes()).hexdigest()
            # Simulate drift after the previously admitted pre-arm observation.
            (root / "document-immutable.files-0").write_bytes(b"changed ink")
            publisher = root / "publish_creation_arm"
            publisher.write_text('#!/bin/sh\ntouch "' + str(root / "publisher-invoked") + '"\n')
            publisher.chmod(0o700)
            # This control targets immutable admission, not alarm ownership.
            # Keep the current --check call shape executable in this fixture.
            alarm = root / 'owned_frame_alarm'
            alarm.write_text('#!/bin/sh\ntest "$1" = --check || exit 90\nshift\nexec "$@"\n')
            alarm.chmod(0o700)
            def transport(arguments, script, timeout):
                result = subprocess.run(["/bin/sh", "-s"], input=script, capture_output=True, text=True, timeout=timeout)
                return {"timeout": False, "exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
            adapter = module.SSHAdapter([], receipt, {"document_id": "fixture"}, root, transport, b"")
            adapter.tap_released = True
            adapter.tap_release_sha256 = "a" * 64
            adapter.tap_supervisor_sha256 = "a" * 64
            adapter.prelude = lambda *args: 'set -eu\nroot=' + module.q(root) + '\ncutoff=100\nnow_upper() { printf "0\\n"; }\nprivate() { test -f "$1"; }\n'
            timing = SimpleNamespace(stage_end=lambda stage: 100, armed_at=1, deadline=361, candidate_at=20)
            waiting = SimpleNamespace(nonce="1" * 32, pid=2, start_ticks=3, expiry_ms=90000)
            ready = {ext + "_raw_sha256": "a" * 64 for ext in ("content", "metadata")}
            with self.assertRaises(Refused): adapter.arm_once(waiting, timing, ready)
            self.assertFalse((root / "publisher-invoked").exists())
            self.assertFalse((root / "creation-arm").exists())
            self.assertIn("FAILED", (root / "host-prepublication-document-immutable.files.check").read_text())
            with self.assertRaises(Refused): adapter.arm_once(waiting, timing, ready)
            self.assertEqual(len(list(root.glob("transport-*.json"))), 1)


if __name__ == "__main__": unittest.main()
