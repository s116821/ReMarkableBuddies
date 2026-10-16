"""Material exact-source/asset refusal controls; no production API writes."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from artifacts import Release, git, sha256
from admission import validate_range, validate
from verify_release import observe, observe_assets, retained
from build import NAMES

class AdmissionAndAssets(unittest.TestCase):
    def test_squash_merge_identity_and_direct_push_refusal(self):
        sha = "a" * 40
        event = {"ref": "refs/heads/main", "after": sha, "forced": False, "created": False,
                 "deleted": False, "repository": {"full_name": "s116821/ReMarkableBuddies"}}
        observation = {"repository": {"object": {"oid": sha, "associatedPullRequests": {"nodes": [
            {"merged": True, "baseRefName": "main", "mergeCommit": {"oid": sha}}]}}}}
        validate(event, "push", sha, "s116821/ReMarkableBuddies", observation)
        observation["repository"]["object"]["associatedPullRequests"]["nodes"] = []
        with self.assertRaises(ValueError):
            validate(event, "push", sha, "s116821/ReMarkableBuddies", observation)

    def test_batch_edit_revert_and_zero_baseline_refuse_before_build(self):
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as temp:
            repo = Path(temp)
            git(repo, "init", "-b", "main")
            git(repo, "config", "user.name", "Owned Fixture")
            git(repo, "config", "user.email", "fixture@example.invalid")
            file = repo / "app.txt"
            file.write_text("base"); git(repo, "add", "."); git(repo, "commit", "-m", "fix(REM-56): base")
            before = git(repo, "rev-parse", "HEAD")
            file.write_text("edit"); git(repo, "add", "."); git(repo, "commit", "-m", "fix(REM-56): edit")
            edit = git(repo, "rev-parse", "HEAD")
            validate_range(repo, before, edit)
            file.write_text("base"); git(repo, "add", "."); git(repo, "commit", "-m", "revert(REM-56): restore")
            after = git(repo, "rev-parse", "HEAD")
            self.assertEqual(git(repo, "diff", "--name-only", before, after), "")
            for base in (before, "0" * 40):
                with self.assertRaises(ValueError):
                    validate_range(repo, base, after)

    def test_malformed_exact_release_observations_refuse(self):
        valid = {"id": 55, "tag_name": "v0.4.1", "draft": True, "assets": []}
        self.assertEqual(observe_assets(valid, "v0.4.1")["databaseId"], 55)
        for field, bad in (("id", True), ("id", "55"), ("draft", "true"), ("assets", None), ("tag_name", "v0.4.2")):
            with self.subTest(field=field, bad=bad), self.assertRaises((ValueError, TypeError)):
                observe_assets(dict(valid, **{field: bad}), "v0.4.1")
        for data in ({}, {"repository": {}}, {"repository": {"release": False}}):
            with self.assertRaises(ValueError):
                observe(data, "v0.4.1")

    def test_partial_bytes_checked_and_only_missing_files_selected(self):
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as temp:
            root = Path(temp)
            packages = root / "packages"; packages.mkdir()
            candidate = Release("v0.4.1", "a" * 40)
            for name in NAMES:
                (packages / name).write_bytes(("owned " + name).encode())
            manifest = dict(tag=candidate.tag, sha=candidate.sha, version="0.4.1",
                            packages={name: sha256(packages / name) for name in NAMES})
            (packages / "provenance.json").write_text(json.dumps(manifest))
            # This control targets retained-byte selection. Actual ZIP/ELF consumers
            # run separately against real built artifacts in the integration job.
            observation = {"id": 55, "tag_name": candidate.tag, "draft": True, "assets": [
                {"id": 90, "name": NAMES[0], "size": (packages / NAMES[0]).stat().st_size}]}
            calls = []
            def download(repo, asset, size, digest, token):
                calls.append(asset)
                self.assertEqual(digest, sha256(packages / NAMES[0]))
            with patch("verify_release.verify_packages", return_value=manifest), patch("verify_release.Path") as path_class:
                # Keep the optional notes file in the owned temporary directory.
                path_class.return_value = root / "notes"
                with contextlib.redirect_stdout(io.StringIO()):
                    retained(packages, candidate, observation, "fixture/owned", "not-a-credential",
                             root / "missing", downloader=download)
            self.assertEqual(calls, [90])
            self.assertEqual({p.name for p in (root / "missing").iterdir()},
                             set(NAMES[1:]) | {"provenance.json"})
            self.assertEqual((packages / NAMES[0]).read_bytes(), ("owned " + NAMES[0]).encode())
            def corrupt(*_args):
                raise ValueError("retained bytes differ")
            with patch("verify_release.verify_packages", return_value=manifest), self.assertRaises(ValueError):
                retained(packages, candidate, observation, "fixture/owned", "not-a-credential",
                         root / "not-created", downloader=corrupt)
            self.assertFalse((root / "not-created").exists())

if __name__ == "__main__":
    unittest.main()
