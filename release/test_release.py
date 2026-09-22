"""Public, offline regression tests: python -m unittest discover -s release -v.

Requires Git, Python 3.11+ and pinned git-cliff on PATH (or GIT_CLIFF).
All tags/remotes are temporary local fixtures, never the production repository.
"""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import coordinator as release
from policy import classify_event, documentation, git

CLIFF = os.environ.get("GIT_CLIFF", "git-cliff")


class Fixture:
    def __init__(self, root):
        self.repo = root / "repo"
        self.remote = root / "remote.git"
        self.repo.mkdir()
        git(self.repo, "init", "-b", "main")
        git(self.repo, "config", "user.name", "Release Fixture")
        git(self.repo, "config", "user.email", "fixture@example.invalid")
        git(self.repo, "init", "--bare", str(self.remote))
        git(self.repo, "remote", "add", "origin", str(self.remote))
        self.counter = 0
        self.commit("chore(REM-0): baseline", "src/main.rs")
        git(self.repo, "tag", "v0.1.12")
        self.push()

    def commit(self, message, *paths):
        self.counter += 1
        for name in paths:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"fixture {self.counter}\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "--allow-empty", "-m", message)
        return git(self.repo, "rev-parse", "HEAD")

    def push(self):
        git(self.repo, "push", "origin", "main", "--tags")

    def plan(self):
        return release.plan(self.repo, "main", CLIFF)


class Publisher:
    def __init__(self):
        self.published = set()
        self.calls = []
        self.fail_upload = False

    def complete(self, candidate):
        return candidate in self.published

    def publish(self, candidate, directory):
        self.calls.append(candidate)
        if self.fail_upload:
            raise RuntimeError("partial upload")
        self.published.add(candidate)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.addCleanup(self.temp.cleanup)
        self.fixture = Fixture(Path(self.temp.name))

    def test_semantic_bumps_from_real_git_cliff(self):
        for message, expected in [
            ("feat(REM-9): feature", "v0.2.0"),
            ("fix(REM-9): repair", "v0.1.13"),
            ("build(deps): dependency", "v0.1.13"),
            ("chore(REM-30): tooling", "v0.1.13"),
            ("refactor(REM-13): restructure", "v0.1.13"),
            ("feat(REM-9)!: breaking", "v1.0.0"),
            ("fix(REM-9): change\n\nBREAKING CHANGE: incompatible format", "v1.0.0"),
        ]:
            with self.subTest(message=message), tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as temp:
                fixture = Fixture(Path(temp))
                sha = fixture.commit(message, "src/main.rs")
                self.assertEqual(fixture.plan(), release.Release(expected, sha))

    def test_docs_only_no_tag_and_no_build_gate(self):
        before = git(self.fixture.repo, "rev-parse", "HEAD")
        after = self.fixture.commit("docs(REM-30): explain", "README.md", "openspec/specs/test/spec.md", "docs/validation/images/diagram.png")
        self.assertIsNone(self.fixture.plan())
        self.assertFalse(classify_event(self.fixture.repo, {"before": before, "after": after}, "push"))
        self.assertEqual(git(self.fixture.repo, "tag"), "v0.1.12")

    def test_mixed_and_later_docs_tag_actual_app_sha(self):
        app = self.fixture.commit("feat(REM-9): feature", "src/main.rs", "README.md")
        self.fixture.commit("docs(REM-30): explain", "docs/guide.md")
        self.assertEqual(self.fixture.plan(), release.Release("v0.2.0", app))

    def test_unreleased_feature_not_lost_by_later_fix(self):
        first = self.fixture.commit("feat(REM-9): feature", "src/main.rs")
        self.fixture.commit("fix(REM-9): repair", "src/main.rs")
        self.assertEqual(self.fixture.plan(), release.Release("v0.2.0", first))

    def test_misclassified_and_unknown_types_fail_closed(self):
        for message in ["docs(REM-9): disguised application", "unknown(REM-9): change", "not conventional"]:
            with self.subTest(message=message), tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as temp:
                fixture = Fixture(Path(temp))
                fixture.commit(message, "Cargo.lock")
                with self.assertRaises(ValueError):
                    fixture.plan()

    def test_reverted_application_push_still_builds(self):
        before = git(self.fixture.repo, "rev-parse", "HEAD")
        changed = self.fixture.commit("fix(REM-9): temporary", "src/main.rs")
        git(self.fixture.repo, "revert", "--no-commit", changed)
        git(self.fixture.repo, "commit", "-m", "revert(REM-9): undo temporary")
        after = git(self.fixture.repo, "rev-parse", "HEAD")
        self.assertEqual(git(self.fixture.repo, "diff", before, after), "")
        self.assertTrue(classify_event(self.fixture.repo, {"before": before, "after": after}, "push"))
        self.assertEqual(self.fixture.plan().tag, "v0.1.13")

    def test_rename_from_application_to_docs_still_relevant(self):
        git(self.fixture.repo, "mv", "src/main.rs", "README.md")
        git(self.fixture.repo, "commit", "-m", "refactor(REM-9): remove application")
        self.assertEqual(self.fixture.plan().tag, "v0.1.13")

    def test_pr_title_gate_rechecks_current_title(self):
        base = git(self.fixture.repo, "rev-parse", "HEAD")
        head = self.fixture.commit("work in progress", "src/main.rs")
        event = {"pull_request": {"base": {"sha": base}, "head": {"sha": head}, "title": "docs(REM-9): wrong"}}
        with self.assertRaises(ValueError):
            classify_event(self.fixture.repo, event, "pull_request")
        event["pull_request"]["title"] = "feat(REM-9): corrected"
        self.assertTrue(classify_event(self.fixture.repo, event, "pull_request"))

    def test_unknown_paths_and_build_dependencies_are_relevant(self):
        for path in ["build.sh", "build.rs", "Cargo.toml", "Cargo.lock", "Cross.toml", ".github/workflows/ci.yml", "release/policy.py", "new-component/file", "docs/simulator/scenarios/blank-answer.json", "docs/validation/fixtures/generate_strokes.py", ".agents/skills/tool/scripts/check.py"]:
            self.assertFalse(documentation(path), path)

    def test_executable_fixture_under_docs_is_application_relevant(self):
        self.fixture.commit("test(REM-22): refine scenario", "docs/simulator/scenarios/blank-answer.json")
        self.assertEqual(self.fixture.plan().tag, "v0.1.13")

    def test_retry_after_tag_and_partial_upload_preserves_sha(self):
        sha = self.fixture.commit("feat(REM-9): feature", "src/main.rs")
        self.fixture.push()
        publisher = Publisher()
        built = []

        def builder(repo, candidate, directory):
            remote = git(repo, "ls-remote", "origin", f"refs/tags/{candidate.tag}^{{}}")
            self.assertEqual(remote.split()[0], candidate.sha)  # tag precedes build
            built.append(candidate)

        publisher.fail_upload = True
        with self.assertRaisesRegex(RuntimeError, "partial upload"):
            release.publish_all(self.fixture.repo, publisher, builder, CLIFF)
        candidate = release.Release("v0.2.0", sha)
        self.assertEqual(built, [candidate])
        publisher.fail_upload = False
        self.fixture.commit("docs(REM-30): newer docs", "README.md")
        self.fixture.push()
        release.publish_all(self.fixture.repo, publisher, builder, CLIFF)
        self.assertEqual(built, [candidate, candidate])
        release.publish_all(self.fixture.repo, publisher, builder, CLIFF)
        self.assertEqual(len(built), 2)  # completed retry does not compile
        self.assertEqual(git(self.fixture.repo, "tag").splitlines(), ["v0.1.12", "v0.2.0"])

    def test_stale_queued_requests_release_every_merge_in_order(self):
        first = self.fixture.commit("feat(REM-9): first queued", "src/main.rs")
        last = self.fixture.commit("fix(REM-9): second queued", "src/main.rs")
        self.fixture.push()
        publisher = Publisher()
        built = []
        builder = lambda repo, candidate, directory: built.append(candidate)
        release.publish_all(self.fixture.repo, publisher, builder, CLIFF)
        release.publish_all(self.fixture.repo, publisher, builder, CLIFF)
        self.assertEqual(built, [release.Release("v0.2.0", first), release.Release("v0.2.1", last)])
        next_sha = self.fixture.commit("fix(REM-9): next merge", "src/main.rs")
        self.fixture.push()
        release.publish_all(self.fixture.repo, publisher, builder, CLIFF)
        self.assertEqual(built[-1], release.Release("v0.2.2", next_sha))

    def test_conflicting_remote_tag_never_builds(self):
        sha = self.fixture.commit("fix(REM-9): repair", "src/main.rs")
        self.fixture.push()
        baseline = git(self.fixture.repo, "rev-parse", "v0.1.12")
        git(self.fixture.remote, "tag", "v0.1.13", baseline)
        with self.assertRaises(subprocess.CalledProcessError):
            release.create_tag(self.fixture.repo, release.Release("v0.1.13", sha))
        self.assertEqual(git(self.fixture.remote, "rev-parse", "v0.1.13"), baseline)

    def test_failed_build_recovers_before_new_application_release(self):
        first = self.fixture.commit("feat(REM-9): first", "src/main.rs")
        self.fixture.push()
        publisher = Publisher()
        def failure(repo, candidate, directory):
            raise RuntimeError("build failed after tag")
        with self.assertRaisesRegex(RuntimeError, "build failed"):
            release.publish_all(self.fixture.repo, publisher, failure, CLIFF)
        second = self.fixture.commit("fix(REM-9): second", "src/main.rs")
        self.fixture.push()
        built = []
        release.publish_all(self.fixture.repo, publisher,
                            lambda repo, candidate, directory: built.append(candidate), CLIFF)
        self.assertEqual(built, [release.Release("v0.2.0", first), release.Release("v0.2.1", second)])
        self.assertEqual(publisher.calls, built)

    def test_unpushed_local_tag_cannot_authorize_recovery_build(self):
        sha = self.fixture.commit("fix(REM-9): repair", "src/main.rs")
        self.fixture.push()
        git(self.fixture.repo, "tag", "-a", "v0.1.13", "-m", release.MARKER)
        built = []
        with self.assertRaisesRegex(ValueError, "Remote tag identity mismatch"):
            release.publish_all(self.fixture.repo, Publisher(),
                                lambda repo, candidate, directory: built.append(candidate), CLIFF)
        self.assertEqual(built, [])

    def test_checksum_and_source_provenance_reject_partial_or_wrong_assets(self):
        candidate = release.Release("v9.8.7", "a" * 40)
        directory = Path(self.temp.name) / "assets"
        directory.mkdir()
        packages = {}
        for target in release.TARGETS:
            path = directory / f"reader-buddy-{target}.tar.gz"
            path.write_bytes(target.encode())
            packages[path.name] = release.sha256(path)
        manifest = dict(tag=candidate.tag, sha=candidate.sha, version="9.8.7", packages=packages)
        provenance = directory / "provenance.json"
        provenance.write_text(json.dumps(manifest))
        release.verify_packages(directory, candidate)
        manifest["sha"] = "b" * 40
        provenance.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, "provenance"):
            release.verify_packages(directory, candidate)
        manifest["sha"] = candidate.sha
        provenance.write_text(json.dumps(manifest))
        (directory / next(iter(packages))).write_bytes(b"partial")
        with self.assertRaisesRegex(ValueError, "checksum"):
            release.verify_packages(directory, candidate)

    def test_shallow_plan_rejected(self):
        clone = Path(self.temp.name) / "shallow"
        git(self.fixture.repo, "clone", "--depth", "1", self.fixture.repo.as_uri(), str(clone))
        with self.assertRaisesRegex(ValueError, "full history"):
            release.plan(clone, "HEAD", CLIFF)


if __name__ == "__main__":
    unittest.main()
