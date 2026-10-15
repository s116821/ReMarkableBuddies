"""Actual PSR semantic/path/tag controls in disposable Git repositories only."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from artifacts import Release, remote_identity, source_identity

CONFIG = Path(__file__).with_name("python-semantic-release.toml").resolve()
TOOL = os.environ.get("PYTHON_SEMANTIC_RELEASE", "semantic-release")

class SemanticToolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "source"
        self.env = {k: v for k, v in os.environ.items() if k in
                    ("PATH", "SystemRoot", "SYSTEMROOT", "TMP", "TEMP", "COMSPEC", "PATHEXT", "LANG")}
        self.env.update(HOME=str(self.root), USERPROFILE=str(self.root), GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
            GIT_AUTHOR_NAME="Owned Fixture", GIT_COMMITTER_NAME="Owned Fixture",
            GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_EMAIL="fixture@example.invalid",
            GIT_AUTHOR_DATE="2026-01-01T00:00:00Z", GIT_COMMITTER_DATE="2026-01-01T00:00:00Z")
        self.run_command(self.root, "git", "init", "--bare", "--initial-branch=main", str(self.root / "remote.git"))
        self.run_command(self.root, "git", "clone", str(self.root / "remote.git"), str(self.repo))
        (self.repo / "releaserc.toml").write_bytes(CONFIG.read_bytes())
        self.count = 0
        self.commit("chore(REM-56): fixture baseline", "app.txt")
        self.git("tag", "-a", "v0.4.0", "-m", "Owned baseline")
        self.git("push", "origin", "main", "--tags")
        self.git("remote", "set-url", "origin", "https://github.com/fixture/never-contact.git")
        self.git("config", f"url.{(self.root / 'remote.git').as_uri()}.insteadOf",
                 "https://github.com/fixture/never-contact.git")

    def run_command(self, cwd, *args):
        return subprocess.run(args, cwd=cwd, env=self.env, text=True, capture_output=True, timeout=40)

    def git(self, *args):
        result = self.run_command(self.repo, "git", *args)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def commit(self, message, path):
        self.count += 1
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(str(self.count))
        self.git("add", ".")
        self.git("commit", "-m", message)

    def version(self, *, print_only=False, push=False):
        args = [TOOL, "-c", "releaserc.toml", "version"]
        args += ["--print-tag"] if print_only else [
            "--no-commit", "--no-changelog", "--skip-build", "--push" if push else "--no-push", "--no-vcs-release"]
        return self.run_command(self.repo, *args)

    def candidate(self):
        result = self.version(print_only=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def test_equal_dates_readonly_candidate_then_late_tag_and_retry(self):
        for subject, expected in [("feat(REM-56): feature", "v0.5.0"), ("fix(REM-56): fix", "v0.5.1")]:
            self.commit(subject, "src/app.rs")
            sha = self.git("rev-parse", "HEAD")
            before = self.git("show-ref", "--tags")
            self.assertEqual(self.candidate(), expected)
            self.assertEqual(self.git("show-ref", "--tags"), before, "Candidate must not mint a tag")
            release = Release(expected, sha)
            source_identity(self.repo, release)
            self.assertEqual(self.version(push=True).returncode, 0)
            remote_identity(self.repo, release)
            self.assertEqual(self.git("rev-parse", "HEAD"), sha)
            self.assertEqual(self.version(push=True).returncode, 0, "Exact successful retry")
            remote_identity(self.repo, release)
        self.assertEqual(len(set(self.git("log", "--format=%ct").splitlines())), 1)
        self.assertEqual(self.git("tag").splitlines(), ["v0.4.0", "v0.5.0", "v0.5.1"])
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_failed_commit_is_not_backfilled_by_later_success(self):
        self.commit("feat(REM-56): failed checks", "src/app.rs")
        failed = self.git("rev-parse", "HEAD")
        self.assertEqual(self.candidate(), "v0.5.0")
        self.assertEqual(self.git("tag").splitlines(), ["v0.4.0"])
        self.commit("fix(REM-56): successful later source", "src/app.rs")
        success = self.git("rev-parse", "HEAD")
        self.assertEqual(self.version(push=True).returncode, 0)
        self.assertEqual(self.git("rev-parse", "v0.5.0^{commit}"), success)
        self.assertNotEqual(success, failed)
        self.assertEqual(self.git("tag").splitlines(), ["v0.4.0", "v0.5.0"])

    def test_docs_types_bodies_and_paths_do_not_inflate_history(self):
        for path in ("README.md", "docs/guide.md", "docs/deep/guide.md", "docs/images/proof.png",
                     "docs/images/deep/proof.svg", "docs/licenses/legal.txt", ".codex/skills/guide.md"):
            self.commit("feat(REM-56)!: documentation only\n\nBREAKING CHANGE: example", path)
            self.assertEqual(self.candidate(), "v0.4.0", path)
        self.commit("fix(REM-56): actual application\n\nfeat(REM-56)!: quoted example", "src/app.rs")
        self.assertEqual(self.candidate(), "v0.4.1")

    def test_unknown_scripts_fixtures_and_renames_are_relevant(self):
        for path in ("docs/images/helper.py", "docs/licenses/tool.sh", "docs/fixture.json", ".codex/helper.py", "unknown"):
            self.commit("fix(REM-56): executable change", path)
            self.assertEqual(self.candidate(), "v0.4.1", path)
        self.git("mv", "unknown", "README.md")
        self.git("commit", "-m", "feat(REM-56): rename application to docs")
        self.assertEqual(self.candidate(), "v0.5.0")

    def test_actual_subject_cannot_borrow_an_earlier_failed_history_bump(self):
        from semantic_source import validate_source
        self.commit("feat(REM-56): earlier failed source", "src/app.rs")
        for subject in ("docs(REM-56): misclassified app", "unsupported message", "fix: missing scope",
                        " fix(REM-56): leading whitespace is not normalized"):
            self.commit(subject, "src/app.rs")
            # Read-only PSR still sees the earlier untagged feat. That is not
            # permission for this ineligible current source to borrow its bump.
            self.assertEqual(self.candidate(), "v0.5.0")
            with self.assertRaisesRegex(ValueError, "Actual merged source"):
                validate_source(self.repo, self.git("rev-parse", "HEAD"))
            self.assertEqual(self.git("tag").splitlines(), ["v0.4.0"])
        self.commit("fix(REM-56): supported actual source", "src/app.rs")
        validate_source(self.repo, self.git("rev-parse", "HEAD"))

    def test_zero_major_breaking_is_reported_and_held_not_downgraded(self):
        self.commit("fix(REM-56)!: break\n\nBREAKING CHANGE: API", "src/app.rs")
        self.assertEqual(self.candidate(), "v1.0.0")
        with self.assertRaisesRegex(ValueError, "REM-35"):
            Release("v1.0.0", self.git("rev-parse", "HEAD"))
        self.assertEqual(self.git("tag").splitlines(), ["v0.4.0"])

    def test_later_main_never_retargets_admitted_candidate(self):
        self.commit("fix(REM-56): admitted source", "src/app.rs")
        release = Release(self.candidate(), self.git("rev-parse", "HEAD"))
        self.git("branch", "admitted")
        self.commit("feat(REM-56): later source", "src/app.rs")
        self.git("push", "origin", "main")
        self.git("checkout", "admitted")
        # The workflow makes a local main branch at its frozen checkout only.
        self.git("checkout", "-B", "main")
        self.assertEqual(self.version(push=True).returncode, 0)
        remote_identity(self.repo, release)

    def test_local_only_tag_and_conflicting_existing_tag_are_not_authority(self):
        self.commit("fix(REM-56): only local", "src/app.rs")
        release = Release(self.candidate(), self.git("rev-parse", "HEAD"))
        self.assertEqual(self.version().returncode, 0)
        with self.assertRaises(ValueError):
            remote_identity(self.repo, release)
        self.commit("fix(REM-56): different source", "src/app.rs")
        with self.assertRaises(ValueError):
            source_identity(self.repo, Release(release.tag, self.git("rev-parse", "HEAD")))

if __name__ == "__main__":
    unittest.main()
