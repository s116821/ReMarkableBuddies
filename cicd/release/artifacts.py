"""Immutable candidate/package checks, not a version calculator or publisher."""
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess

TAG = re.compile(r"v0\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")
SHA = re.compile(r"[0-9a-f]{40}\Z")

@dataclass(frozen=True)
class Release:
    tag: str
    sha: str
    def __post_init__(self):
        if not TAG.fullmatch(self.tag) or not SHA.fullmatch(self.sha):
            raise ValueError("Expected pre-1.0 tag and full source SHA; REM-35 owns 1.0")

def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True, encoding="utf-8").strip()

def source_identity(repo, release):
    if git(repo, "rev-parse", "--is-shallow-repository") != "false":
        raise ValueError("Full source history required")
    if git(repo, "rev-parse", "HEAD") != release.sha or git(repo, "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("Expected exact clean source")
    existing = subprocess.run(["git", "rev-parse", "--verify", f"refs/tags/{release.tag}^{{commit}}"],
                              cwd=repo, capture_output=True, text=True)
    if existing.returncode == 0 and existing.stdout.strip() != release.sha:
        raise ValueError("Candidate tag already belongs to another source")
    # A candidate is not remote tag/publication authority.

def remote_identity(repo, release):
    lines = git(repo, "ls-remote", "--tags", "origin",
                f"refs/tags/{release.tag}", f"refs/tags/{release.tag}^{{}}").splitlines()
    refs = dict(line.split()[::-1] for line in lines)
    obj = refs.get(f"refs/tags/{release.tag}")
    if not obj or refs.get(f"refs/tags/{release.tag}^{{}}") != release.sha:
        raise ValueError("Expected existing annotated remote tag at exact validated SHA")
    git(repo, "fetch", "--no-tags", "origin", obj)
    if git(repo, "cat-file", "-t", obj) != "tag" or git(repo, "rev-parse", f"{obj}^{{commit}}") != release.sha:
        raise ValueError("Fetched remote tag differs")
    if git(repo, "ls-remote", "--tags", "origin", f"refs/tags/{release.tag}", f"refs/tags/{release.tag}^{{}}").splitlines() != lines:
        raise ValueError("Remote tag changed during verification")

def sha256(path):
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("Expected regular package file")
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()

def verify_identity(manifest, release, names):
    if (manifest["tag"], manifest["sha"], manifest["version"]) != (release.tag, release.sha, release.tag[1:]):
        raise ValueError("Package identity differs from frozen candidate")
    if set(manifest["packages"]) != set(names):
        raise ValueError("Fixed package inventory differs")
    if any(not re.fullmatch(r"[0-9a-f]{64}", value) for value in manifest["packages"].values()):
        raise ValueError("Malformed package hash")

def verify_packages(directory, release, names):
    manifest = json.loads((directory / "provenance.json").read_text(encoding="utf-8"))
    verify_identity(manifest, release, names)
    if {p.name for p in directory.iterdir()} != set(names) | {"provenance.json"}:
        raise ValueError("Unexpected or missing package file")
    for name, digest in manifest["packages"].items():
        if sha256(directory / name) != digest:
            raise ValueError(f"Package hash differs: {name}")
    return manifest

def output(**values):
    import uuid
    text = ""
    for name, value in values.items():
        delimiter = uuid.uuid4().hex
        text += f"{name}<<{delimiter}\n{value}\n{delimiter}\n"
    print(text, end="")
    if destination := os.environ.get("GITHUB_OUTPUT"):
        with open(destination, "a", encoding="utf-8") as stream:
            stream.write(text)
