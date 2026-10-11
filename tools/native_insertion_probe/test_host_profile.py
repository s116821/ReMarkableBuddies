"""Real packet/profile drift must refuse before SSH pinning or transport."""
import io
import json
from pathlib import Path
import tempfile
import unittest
import copy
from unittest.mock import patch
from PIL import Image
import coordinator
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, TOOLS as CAPTURE_TOOLS
from owned_frame_capture import OBSERVATION_KIND
from prepare_packet import ENCODER_HASHES
from open_document_timing import PRODUCTION_V2, V2_CONTRACT, V2_SDK_REVISION, V2_SDK_SOURCES, V2_PUBLISHER_SHA256


class ProfileTests(unittest.TestCase):
    def test_explicit_v2_contract_sdk_config_and_artifact_drift_refuse(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary); receipt,profile=self.fixture(root)
            receipt['experiment_kind']=profile['experiment_kind']=PRODUCTION_V2
            receipt['library_tap_variant']=profile['library_tap_variant']='primary-1641396'
            profile.update(production_contract=V2_CONTRACT,sdk_revision=V2_SDK_REVISION,
                           sdk_source_hashes=V2_SDK_SOURCES,sdk_setup_variant='AfterOpen95s',
                           sdk_configuration_sha256='c'*64,host_sequence_contract='bounded-host-v6-production-v2')
            receipt['files']['publish_creation_arm']=profile['tools']['publish_creation_arm']=V2_PUBLISHER_SHA256
            # This pure profile test uses a synthetic PNG pin; actual approved
            # Library pixels and their fixed production hash are separate gates.
            with patch('coordinator.V2_LIBRARY_SHA256',profile['references']['library-reference.png']):
                (root/'host-profile.json').write_text(json.dumps(profile))
                self.assertEqual(coordinator.load_host_profile(root,receipt),profile)
                alternate=copy.deepcopy(profile); alternate["library_tap_variant"]="alternate-2011360"
                (root/"host-profile.json").write_text(json.dumps(alternate))
                self.assertEqual(coordinator.load_host_profile(root,receipt | {"library_tap_variant":"alternate-2011360"}),alternate)
                for missing in (None, "unknown", 1, [], {}):
                    with self.assertRaises(ValueError): coordinator.load_host_profile(root,receipt | {"library_tap_variant":missing})
                for missing_from in ("profile","receipt"):
                    missing_profile=copy.deepcopy(profile);missing_receipt=copy.deepcopy(receipt)
                    (missing_profile if missing_from=="profile" else missing_receipt).pop("library_tap_variant")
                    (root/"host-profile.json").write_text(json.dumps(missing_profile))
                    with self.assertRaises(ValueError):coordinator.load_host_profile(root,missing_receipt)
                for field,value in (('sdk_revision','485ac1a46367bfdf203abe140a8b764728cdf54d'),
                                    ('sdk_setup_variant','Legacy40s'),('sdk_source_hashes',{}),
                                    ('production_contract',{}),('sdk_configuration_sha256',''),
                                    ('host_sequence_contract','bounded-host-v5'),
                                    ('library_tap_variant',None),('library_tap_variant','unknown'),
                                    ('library_tap_variant',1),('library_tap_variant',[]),
                                    ('library_tap_variant','alternate-2011360')):
                    changed=copy.deepcopy(profile); changed[field]=value
                    (root/'host-profile.json').write_text(json.dumps(changed))
                    with self.subTest(field=field),self.assertRaises(ValueError): coordinator.load_host_profile(root,receipt)
    def test_preparation_tap_enum_has_no_default_or_adaptive_choice(self):
        from prepare_packet import library_tap_selection
        for value, command, point in (
            ("primary-1641396","--tap-library-v2",[164,1396]),
            ("alternate-2011360","--tap-library-v2-alt",[201,1360]),
        ):
            self.assertEqual(library_tap_selection({"library_tap_variant":value}),(command,point))
        for value in (None, "", "unknown", 1, [], {}):
            with self.assertRaises(ValueError): library_tap_selection({"library_tap_variant":value})

    def test_purpose_is_bound_to_profile_and_packet(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);receipt,profile=self.fixture(root)
            receipt['experiment_kind']=OBSERVATION_KIND
            with self.assertRaises(ValueError): coordinator.load_host_profile(root,receipt)
            profile['experiment_kind']=OBSERVATION_KIND
            (root/'host-profile.json').write_text(json.dumps(profile))
            self.assertEqual(coordinator.load_host_profile(root,receipt),profile)
            receipt['experiment_kind']='unknown'
            with self.assertRaises(ValueError): coordinator.load_host_profile(root,receipt)
    def fixture(self, root):
        sources = ("open_document_host.py", "open_document_transport.py", "open_document_protocol.py",
                   "open_document_timing.py", "publish_creation_arm.c", "coordinator.py", "prepare_packet.py", "actor.sh.in", "arm.sh.in", "owned_tap_guardian.c", "owned_frame_capture.py")
        receipt = {"experiment_kind": "open-document-insertion-v1", "nonce": "1" * 32, "document_id": "e7f661f1-db6f-4dfc-854a-b38aff7f75de",
                   "target_page_id": "f" * 36, "before_page_ids": list(range(7)), "files": {}}
        profile = {"experiment_kind": "open-document-insertion-v1", "host_sequence_contract": "bounded-host-v5", "references": {}, "encoder_hashes": ENCODER_HASHES, "source_sha256": {name: coordinator.sha((coordinator.HERE / name).read_bytes()) for name in sources},
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
