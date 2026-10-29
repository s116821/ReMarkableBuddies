"""Non-runnable local inventory plus compiled invalid role/case rejection."""
from pathlib import Path
import json
import subprocess
import tempfile
import unittest

from prepare_packet import EVENT_ROLES, ROLES, prepare


class PacketTests(unittest.TestCase):
    def test_reachable_event_inventory_matches_compiled_role_refusal(self):
        with tempfile.TemporaryDirectory(prefix="e0t-packet-") as temp:
            parent = Path(temp)
            nonce = "0123456789abcdef0123456789abcdef"
            root = parent / ("buddy-e0t-" + nonce)
            root.mkdir(mode=0o700)
            (root / "owner").write_text(nonce)
            (root / "owner").chmod(0o600)
            binary = parent / "helper"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-DE0T_ACTORS", "-DE0T_MANAGER",
                            '-DE0T_NONCE="' + nonce + '"', '-DE0T_RUNTIME_PARENT="' + str(parent) + '"',
                            str(Path(__file__).with_name("helper.c")), "-o", str(binary)], check=True)
            manifest = prepare(parent / "draft", nonce)
            event_paths = {item["path"] for item in manifest["owned_paths"] if "/events-T" in item["path"]}
            expected = {manifest["root"] + f"/events-T{case}-{role}"
                        for case, roles in EVENT_ROLES.items() for role in roles}
            self.assertEqual(event_paths, expected)
            self.assertEqual(len(event_paths), 28)
            self.assertEqual(len(manifest["owned_paths"]), 197)
            self.assertEqual(len({item["path"] for item in manifest["owned_paths"]}), 197)
            self.assertEqual(manifest["evidence_candidates"]["bounded_runtime_record_bytes"], 68577)
            self.assertFalse(manifest["runnable"])
            self.assertFalse(manifest["evidence_candidates"]["full_evidence_frozen"])
            refused = 0
            for case in range(1, 9):
                for role in ROLES:
                    if role in EVENT_ROLES[case]:
                        continue  # Existing worker/actor fixtures exercise admitted roles.
                    result = subprocess.run([str(binary), role, str(case)], capture_output=True, timeout=2)
                    self.assertEqual(result.returncode, 90, (case, role, result.stderr))
                    self.assertIn(b"role not valid for case", result.stderr)
                    self.assertFalse((root / f"events-T{case}-{role}").exists())
                    refused += 1
            self.assertEqual(refused, 68)
            self.assertEqual(sorted(item.name for item in root.iterdir()), ["owner"])
            self.assertEqual(json.loads((parent / "draft" / "manifest.json").read_text()), manifest)


if __name__ == "__main__":
    unittest.main()
