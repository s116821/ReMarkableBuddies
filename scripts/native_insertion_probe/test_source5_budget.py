"""Local packet-budget admission checks; synthetic pins and geometry, no transport.
Runtime control enforcement and full cryptographic packet admission are separate gates.
"""
import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import coordinator as c
import test_host_profile as profile_fixtures
from test_source5_selection import source5
from open_document_timing import V2_PUBLISHER_SHA256, SOURCE5_CONTROL_POLICY, SOURCE5_CONTROL_SHELL_SHA256

class Source5BudgetReview(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = root = Path(temporary.name)
        receipt, profile = profile_fixtures.ProfileTests().fixture(root)
        selected = source5()
        profile.update(selected)
        profile.pop("before_page_ids")
        profile.update(page_ids=selected["before_page_ids"], target_id=selected["target_page_id"],
                       host_sequence_contract="bounded-host-v6-production-v2", library_tap_variant="primary-1641396")
        receipt.update(selected)
        profile.update(control_output_policy=SOURCE5_CONTROL_POLICY, control_shell_sha256=SOURCE5_CONTROL_SHELL_SHA256)
        receipt.update(control_output_policy=SOURCE5_CONTROL_POLICY, control_shell_sha256=SOURCE5_CONTROL_SHELL_SHA256)
        receipt["library_tap_variant"] = "primary-1641396"
        receipt["files"]["publish_creation_arm"] = profile["tools"]["publish_creation_arm"] = V2_PUBLISHER_SHA256
        entries = {f"/fixture/immutable/{i}": c.sha(str(i).encode()) for i in range(16)}
        profile["immutable_hashes"] = {
            k: v for k, v in profile["immutable_hashes"].items() if "document-immutable.files" not in k
        } | entries
        (root / "document-immutable.files").write_text("".join(f"{v}  {k}\n" for k, v in entries.items()))
        # Synthetic geometry for packet admission; actual CLI geometry is reviewed separately.
        paths = {}
        for direction, start_x, sign in (("previous", 100, 1), ("next", 700, -1)):
            virtual = [[start_x + sign * (590 * step // 15), 512] for step in range(16)]
            paths[direction] = {"geometry_variant": "fixed-gap10-v1",
                "navigation_policy": "stop-at-declared-neighbor-v1", "direction": direction,
                "virtual_path": virtual,
                "native_path": [[x * 1404 // 768, y * 1872 // 1024] for x, y in virtual]}
        profile["tools"]["owned_relative_gesture"] = receipt["files"]["owned_relative_gesture"] = "b" * 64
        profile["gesture_paths"] = paths
        profile["gesture_path_hashes"] = {}
        for direction in paths:
            name = "gesture-paths-" + direction + ".json"
            data = json.dumps(paths[direction]).encode()
            (root / name).write_bytes(data)
            profile["gesture_path_hashes"][name] = receipt["files"][name] = c.sha(data)
        for name in c.packet_files(receipt) - {"host-profile.json"}:
            if not (root / name).exists():
                (root / name).write_bytes(b"x")
        profile["packet_file_sizes"] = {
            name: (root / name).stat().st_size for name in c.packet_files(receipt) - {"host-profile.json"}
        }
        profile["source5_control_bytes_reserve"] = 17 * 1024 * 1024
        receipt["packet_file_sizes"] = copy.deepcopy(profile["packet_file_sizes"])
        receipt["source5_control_bytes_reserve"] = 17 * 1024 * 1024
        (root / "preparation.json").write_bytes(b"x")
        (root / "packet.files").write_bytes(b"x")
        self.profile, self.receipt = profile, receipt
        self.enterContext(patch("coordinator.SOURCE5_LIBRARY_SHA256", profile["references"]["library-reference.png"]))
        self.enterContext(patch("coordinator.SOURCE5_GESTURE_SHA256", "b" * 64))
        self.transport = self.enterContext(patch.object(c, "transport", side_effect=AssertionError("No transport permitted")))

    def admit(self, profile=None, receipt=None):
        profile = self.profile if profile is None else profile
        receipt = self.receipt if receipt is None else receipt
        (self.root / "host-profile.json").write_text(json.dumps(profile))
        return c.load_host_profile(self.root, receipt)

    def tearDown(self):
        self.transport.assert_not_called()

    def test_positive_complete_profile(self):
        self.assertEqual(self.admit(), self.profile)

    def test_witness_receipt_profile_mismatch_refuses_before_transport(self):
        changed=self.profile|{'source_witness_variant':'source5-stable-body-idle-v1'}
        with self.assertRaises(ValueError):self.admit(changed)
        missing=copy.deepcopy(self.profile);del missing['source_witness_variant']
        with self.assertRaises(ValueError):self.admit(missing)

    def test_size_schema_and_receipt_disagreement(self):
        variants = []
        for value in (True, -1, 2):
            changed = copy.deepcopy(self.profile)
            changed["packet_file_sizes"]["payload.so"] = value
            variants.append(changed)
        missing = copy.deepcopy(self.profile)
        del missing["packet_file_sizes"]["payload.so"]
        variants.append(missing)
        extra = copy.deepcopy(self.profile)
        extra["packet_file_sizes"]["extra"] = 1
        variants.append(extra)
        variants.append(self.profile | {"source5_control_bytes_reserve": 18 * 1024 * 1024})
        for changed in variants:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                self.admit(changed)

    def test_matching_declared_sizes_cannot_hide_actual_file_drift(self):
        self.profile["packet_file_sizes"]["payload.so"] = 2
        self.receipt["packet_file_sizes"]["payload.so"] = 2
        self.assertEqual((self.root / "payload.so").stat().st_size, 1)
        with self.assertRaisesRegex(ValueError, "Open-document execution refused"):
            self.admit()

    def test_profile_cap(self):
        with self.assertRaises(ValueError):
            self.admit(self.profile | {"extra_padding": "x" * 65536})

    def test_receipt_manifest_combined_boundary(self):
        (self.root / "preparation.json").write_bytes(b"x" * 65535)
        self.assertEqual(self.admit(), self.profile)
        (self.root / "preparation.json").write_bytes(b"x" * 65536)
        with self.assertRaises(ValueError):
            self.admit()

    def test_aggregate_exact_limit_and_one_over(self):
        limit = 17 * 1024 * 1024 - 2 * 65536 - 1024 * 1024
        others = sum(v for k, v in self.profile["packet_file_sizes"].items() if k != "payload.so")
        size = limit - others
        for delta in (0, 1):
            (self.root / "payload.so").write_bytes(b"x" * (size + delta))
            self.profile["packet_file_sizes"]["payload.so"] = size + delta
            self.receipt["packet_file_sizes"]["payload.so"] = size + delta
            if delta == 0:
                self.assertEqual(self.admit(), self.profile)
            else:
                with self.assertRaises(ValueError):
                    self.admit()

if __name__ == "__main__":
    unittest.main(verbosity=2)
