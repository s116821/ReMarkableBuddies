"""Compile the actual build.rs in a tiny isolated crate; no tablet is required."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from policy import git

ROOT = Path(__file__).resolve().parents[1]


class MetadataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        cls.repo = Path(cls.temp.name) / "metadata"
        (cls.repo / "src").mkdir(parents=True)
        shutil.copyfile(ROOT / "build.rs", cls.repo / "build.rs")
        (cls.repo / "Cargo.toml").write_text('''[package]
name = "release-metadata-fixture"
version = "0.0.0"
edition = "2021"
[build-dependencies]
vergen-gitcl = { version = "=10.0.3", features = ["emit_and_set"] }
''')
        (cls.repo / "src/main.rs").write_text('fn main() { println!("{}", env!("READER_BUDDY_VERSION")); }\n')
        (cls.repo / ".gitignore").write_text("target/\n")
        git(cls.repo, "init", "-b", "main")
        git(cls.repo, "config", "user.name", "Metadata Fixture")
        git(cls.repo, "config", "user.email", "fixture@example.invalid")
        subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=cls.repo, check=True, capture_output=True)
        git(cls.repo, "add", ".")
        git(cls.repo, "commit", "-m", "feat(REM-30): metadata fixture")
        cls.sha = git(cls.repo, "rev-parse", "HEAD")
        cls.env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("READER_BUDDY_RELEASE_", "VERGEN_", "CARGO_TARGET_DIR"))}
        # This dependency cache is separate from the application's target dir.
        cls.env["CARGO_TARGET_DIR"] = str(Path(cls.temp.name) / "target")

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def run_metadata(self, repo=None, **overrides):
        return subprocess.run(["cargo", "run", "--locked", "--offline", "--quiet"],
                              cwd=repo or self.repo, env=dict(self.env, **overrides),
                              text=True, capture_output=True)

    def test_source_metadata_and_cache_transitions(self):
        dev = self.run_metadata()
        self.assertEqual(dev.returncode, 0, dev.stderr)
        self.assertTrue(dev.stdout.strip().startswith("dev."), dev.stdout)
        git(self.repo, "tag", "v9.8.7")
        official = dict(READER_BUDDY_RELEASE_TAG="v9.8.7", READER_BUDDY_RELEASE_SHA=self.sha)
        exact = self.run_metadata(**official)
        self.assertEqual(exact.returncode, 0, exact.stderr)
        self.assertEqual(exact.stdout.strip(), "9.8.7")
        for overrides in [
            dict(official, READER_BUDDY_RELEASE_SHA="0" * 40),
            dict(official, READER_BUDDY_RELEASE_TAG="v9.8.8"),
            dict(official, VERGEN_GIT_DESCRIBE="v9.8.7"),
        ]:
            result = self.run_metadata(**overrides)
            self.assertNotEqual(result.returncode, 0, result.stdout)
        path = self.repo / "src/main.rs"
        original = path.read_text()
        path.write_text(original + "// tracked dirty source\n")
        dirty = self.run_metadata(**official)
        self.assertNotEqual(dirty.returncode, 0)
        dirty_dev = self.run_metadata()
        self.assertEqual(dirty_dev.returncode, 0, dirty_dev.stderr)
        self.assertIn("dirty", dirty_dev.stdout)
        path.write_text(original)
        restored = self.run_metadata(**official)
        self.assertEqual(restored.returncode, 0, restored.stderr)
        self.assertEqual(restored.stdout.strip(), "9.8.7")
        shallow = Path(self.temp.name) / "shallow"
        git(self.repo, "clone", "--depth", "1", self.repo.as_uri(), str(shallow))
        self.assertNotEqual(self.run_metadata(shallow, **official).returncode, 0)
        missing = Path(self.temp.name) / "missing"
        shutil.copytree(self.repo, missing, ignore=shutil.ignore_patterns(".git", "target"))
        self.assertNotEqual(self.run_metadata(missing, **official).returncode, 0)
        unknown = self.run_metadata(missing)
        self.assertEqual(unknown.returncode, 0, unknown.stderr)
        self.assertEqual(unknown.stdout.strip(), "dev.unknown")


if __name__ == "__main__":
    unittest.main()
