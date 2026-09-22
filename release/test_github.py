"""Production gh adapter contract tests with an isolated in-memory API boundary."""

import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from coordinator import GitHub, Release, TARGETS, sha256


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name) / "assets"
        self.directory.mkdir()
        self.release = Release("v9.8.7", "a" * 40)
        self.adapter = GitHub("fixture/repo")
        packages = {}
        for target in TARGETS:
            path = self.directory / f"reader-buddy-{target}.tar.gz"
            path.write_bytes(target.encode())
            packages[path.name] = sha256(path)
        manifest = dict(tag=self.release.tag, sha=self.release.sha, version="9.8.7", packages=packages)
        (self.directory / "provenance.json").write_text(json.dumps(manifest))
        self.state = None
        self.fail_upload = False
        self.corrupt_download = False
        self.calls = []

    def fake_run(self, *args):
        self.calls.append(args)
        operation = args[1]
        if operation == "create":
            self.state = {"draft": True, "body": "", "assets": []}
        elif operation == "upload":
            if self.fail_upload:
                raise RuntimeError("upload interrupted")
            self.state["assets"] = [{"name": path.name, "digest": f"sha256:{sha256(path)}"}
                                    for path in self.directory.iterdir()]
        elif operation == "download":
            target = Path(args[args.index("--dir") + 1])
            for path in self.directory.iterdir():
                shutil.copyfile(path, target / path.name)
            if self.corrupt_download:
                (target / "provenance.json").unlink()
        elif operation == "edit":
            self.state["draft"] = False
            self.state["body"] = args[args.index("--notes") + 1]
        return ""

    def test_view_distinguishes_missing_from_auth_and_network_failure(self):
        for message, missing in [("gh: Not Found (HTTP 404)", True),
                                 ("gh: Bad credentials (HTTP 401)", False),
                                 ("network timeout", False)]:
            result = subprocess.CompletedProcess([], 1, "", message)
            with self.subTest(message=message), patch("coordinator.subprocess.run", return_value=result):
                if missing:
                    self.assertIsNone(self.adapter.view(self.release))
                else:
                    with self.assertRaises(RuntimeError):
                        self.adapter.view(self.release)

    def test_partial_draft_retry_and_published_immutability(self):
        with patch.object(self.adapter, "view", side_effect=lambda _: self.state), patch.object(self.adapter, "run", side_effect=self.fake_run):
            self.fail_upload = True
            with self.assertRaisesRegex(RuntimeError, "upload interrupted"):
                self.adapter.publish(self.release, self.directory)
            self.assertTrue(self.state["draft"])
            self.assertFalse(self.adapter.complete(self.release))
            self.fail_upload = False
            self.adapter.publish(self.release, self.directory)
            self.assertFalse(self.state["draft"])
            calls = len(self.calls)
            self.assertTrue(self.adapter.complete(self.release))
            self.assertEqual(len(self.calls), calls)  # no historical binary download
            with self.assertRaisesRegex(ValueError, "overwrite"):
                self.adapter.publish(self.release, self.directory)
            self.state["assets"][0]["digest"] = "sha256:wrong"
            with self.assertRaisesRegex(ValueError, "digests"):
                self.adapter.complete(self.release)

    def test_missing_uploaded_provenance_stays_draft(self):
        self.corrupt_download = True
        with patch.object(self.adapter, "view", side_effect=lambda _: self.state), patch.object(self.adapter, "run", side_effect=self.fake_run):
            with self.assertRaises(FileNotFoundError):
                self.adapter.publish(self.release, self.directory)
        self.assertTrue(self.state["draft"])
        self.assertFalse(any(args[1] == "edit" for args in self.calls))

    def test_published_missing_or_wrong_source_provenance_fails_closed(self):
        self.state = {"draft": False, "body": "", "assets": []}
        with patch.object(self.adapter, "view", side_effect=lambda _: self.state):
            with self.assertRaisesRegex(ValueError, "completion provenance"):
                self.adapter.complete(self.release)
            record = json.loads((self.directory / "provenance.json").read_text())
            record["sha"] = "b" * 40
            self.state["body"] = f"<!-- reader-buddy-complete:{json.dumps(record)} -->"
            with self.assertRaisesRegex(ValueError, "provenance"):
                self.adapter.complete(self.release)


if __name__ == "__main__":
    unittest.main()
