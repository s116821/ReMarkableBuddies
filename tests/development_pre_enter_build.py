"""Run the actual Cargo-built build script against disposable Git identities.

Usage: python3 tests/development_pre_enter_build.py /ABS/target/debug/build/
remarkable-reader-buddy-HASH/build-script-build
The developer Cargo checks separately establish effective cfg propagation and
final-target compile-time coupling. This test never tags a project checkout.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

BUILD_SCRIPT = Path(sys.argv.pop(1)).resolve()
REFUSAL = "Development pre-Enter build admission refused"
CHANNELS = (
    "READER_BUDDY_RELEASE_TAG", "READER_BUDDY_RELEASE_SHA",
    "READER_BUDDY_CANDIDATE_TAG", "READER_BUDDY_CANDIDATE_SHA",
)


class BuildAdmission(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="pre-enter-build-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.env = {k: v for k, v in os.environ.items()
                    if k not in CHANNELS and not k.startswith(("VERGEN_", "GIT_"))}
        self.env.update(CARGO_CFG_BUDDY_DEVELOPMENT_NATIVE_PRE_ENTER="",
                        GIT_AUTHOR_NAME="Fixture", GIT_COMMITTER_NAME="Fixture",
                        GIT_AUTHOR_EMAIL="fixture@example.invalid",
                        GIT_COMMITTER_EMAIL="fixture@example.invalid")

    def git(self, *args, data=None):
        return subprocess.check_output(["git", *args], cwd=self.root,
                                       env=self.env, input=data, stderr=subprocess.DEVNULL).strip()

    def refuse(self, extra=None):
        env = self.env | (extra or {})
        result = subprocess.run([str(BUILD_SCRIPT)], cwd=self.root, env=env,
                                capture_output=True, text=True, timeout=15)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(REFUSAL, result.stderr)
        self.assertNotIn("cargo:rustc-cfg=buddy_development_native_pre_enter_admitted", result.stdout)
        self.assertNotIn("private-fixture-value", result.stdout + result.stderr)

    def test_each_identity_channel_presence_including_empty_and_non_utf8(self):
        # Refusal must precede Git identity and official metadata parsing.
        for channel in CHANNELS:
            for value in ("", "private-fixture-value", os.fsdecode(b"\xff")):
                with self.subTest(channel=channel, value=repr(value)):
                    self.refuse({channel: value})
        for extra in (
            {CHANNELS[0]: "", CHANNELS[1]: ""},
            {CHANNELS[2]: "bad", CHANNELS[3]: "bad"},
            dict.fromkeys(CHANNELS, "private-fixture-value"),
        ):
            self.refuse(extra)

    def test_unavailable_git_identity_fails_closed(self):
        self.refuse()
        self.refuse({"PATH": str(self.root)})

    def test_exact_official_tags_refuse_even_with_dirty_lock(self):
        self.git("init", "--quiet")
        tree = self.git("mktree", data=b"")
        commit = self.git("commit-tree", tree.decode(), data=b"fixture\n")
        self.git("update-ref", "HEAD", commit.decode())
        # Tags exist only in this disposable regression fixture.
        self.git("tag", "v7.8.9")
        self.refuse()
        (self.root / "Cargo.lock").write_text("dirty developer lock\n")
        self.refuse()
        self.git("tag", "-d", "v7.8.9")
        self.git("tag", "-a", "v8.9.10", "-m", "fixture")
        self.refuse()


if __name__ == "__main__":
    unittest.main()
