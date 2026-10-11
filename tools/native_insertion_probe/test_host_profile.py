"""Real packet/profile drift must refuse before SSH pinning or transport."""
import io
import json
from pathlib import Path
import tempfile
import unittest
from PIL import Image
import coordinator
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, TOOLS as CAPTURE_TOOLS
from prepare_packet import ENCODER_HASHES


class ProfileTests(unittest.TestCase):
    def fixture(self, root):
        sources = ("open_document_host.py", "open_document_transport.py", "open_document_protocol.py",
                   "open_document_timing.py", "publish_creation_arm.c", "coordinator.py", "prepare_packet.py", "actor.sh.in", "arm.sh.in", "owned_tap_guardian.c", "owned_frame_capture.py")
        receipt = {"nonce": "1" * 32, "document_id": "e7f661f1-db6f-4dfc-854a-b38aff7f75de",
                   "target_page_id": "f" * 36, "before_page_ids": list(range(7)), "files": {}}
        profile = {"host_sequence_contract": "bounded-host-v5", "references": {}, "encoder_hashes": ENCODER_HASHES, "source_sha256": {name: coordinator.sha((coordinator.HERE / name).read_bytes()) for name in sources},
                   "nonce": receipt["nonce"], "document_id": receipt["document_id"], "target_id": receipt["target_page_id"], "page_ids": receipt["before_page_ids"],
                   "boot_id": "02b2347b-03aa-4a85-8dd7-d731aabf6f3e", "immutable_hashes": {}, "tools": {},
                   "sdk_creation_deadline_sha256": "c3d8e7124eff6de5868c74f321189ce1f55f8b4111e8983ecdc7eb7c86b3a109",
                   "publisher_source_sha256": coordinator.sha((coordinator.HERE / "publish_creation_arm.c").read_bytes()),
                   "capture_contract": CAPTURE_CONTRACT}
        for name, count in (("runtime.files", 10), ("document-immutable.files", 14)):
            entries = {f"/fixture/{name}/{i}": coordinator.sha(str(i).encode()) for i in range(count)}
            profile["immutable_hashes"].update(entries)
            (root / name).write_bytes("".join(f"{digest}  {path}\n" for path, digest in entries.items()).encode())
        for name in (("owned_tap_guardian", "observe_input", "publish_creation_arm") + tuple(CAPTURE_TOOLS)):
            profile["tools"][name] = receipt["files"][name] = CAPTURE_TOOLS.get(name, coordinator.sha(name.encode()))
        png = io.BytesIO(); Image.new("RGB", (768, 1024)).save(png, format="PNG")
        for name in ("library-reference.png", "source-body-reference.png", "source-footer-reference.png"):
            (root / name).write_bytes(png.getvalue())
            profile["references"][name] = receipt["files"][name] = coordinator.sha(png.getvalue())
        (root / "host-profile.json").write_text(json.dumps(profile))
        return receipt, profile

    def test_tool_partition_clock_source_and_host_source_drift_refuse(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); receipt, profile = self.fixture(root)
            self.assertEqual(coordinator.load_host_profile(root, receipt), profile)
            variants = [profile | {"tools": {}}, profile | {"immutable_hashes": {}}, profile | {"encoder_hashes": {}},
                        profile | {"host_sequence_contract": "bounded-host-v3"}, profile | {"references": {}}, profile | {"capture_contract": {}},
                        profile | {"sdk_creation_deadline_sha256": "0" * 64},
                        profile | {"source_sha256": profile["source_sha256"] | {"open_document_transport.py": "0" * 64}}]
            for changed in variants:
                (root / "host-profile.json").write_text(json.dumps(changed))
                with self.assertRaisesRegex(ValueError, "execution refused"):
                    coordinator.load_host_profile(root, receipt)

    def test_each_separate_reference_is_required_and_bound(self):
        for name in ("source-body-reference.png", "source-footer-reference.png"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); receipt, _ = self.fixture(root)
                path = root / name
                original = path.read_bytes()
                path.unlink()
                with self.assertRaisesRegex(ValueError, "execution refused"):
                    coordinator.load_host_profile(root, receipt)
                path.write_bytes(original + b"changed")
                with self.assertRaisesRegex(ValueError, "execution refused"):
                    coordinator.load_host_profile(root, receipt)


if __name__ == "__main__": unittest.main()
