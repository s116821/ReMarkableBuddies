"""Build and verify application packages. No GitHub writes or version selection."""
import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tomllib
from abi import qualify

TAG = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")

@dataclass(frozen=True)
class Release:
    tag: str
    sha: str

def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True, encoding="utf-8").strip()

TARGETS = ("armv7-unknown-linux-gnueabihf", "aarch64-unknown-linux-gnu")

def sha256(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()

def verify_packages(directory, release):
    manifest = json.loads((directory / "provenance.json").read_text())
    verify_identity(manifest, release)
    if {path.name for path in directory.iterdir()} != set(manifest['packages']) | {'provenance.json'}:
        raise ValueError("Package directory does not match the fixed release inventory")
    for name, digest in manifest["packages"].items():
        if sha256(directory / name) != digest:
            raise ValueError(f"Package checksum mismatch: {name}")

def verify_identity(manifest, release):
    if (manifest["tag"], manifest["sha"], manifest["version"]) != (release.tag, release.sha, release.tag[1:]):
        raise ValueError("Release provenance does not match tag and source")
    expected = {f"reader-buddy-{target}.tar.gz" for target in TARGETS}
    if set(manifest["packages"]) != expected:
        raise ValueError("Release must contain both architecture packages")

def build_checkout(repo, release, directory, target_dir=None):
    git(repo, "checkout", "--detach", release.tag)
    if git(repo, "rev-parse", "HEAD") != release.sha:
        raise ValueError("Checkout does not match release SHA")
    configuration = Path(__file__).with_name("Cross.release.toml").resolve()
    container_source = repo.as_posix()
    if os.name == "nt":
        container_source = "/mnt/" + container_source[0].lower() + container_source[2:]
    env = dict(os.environ, READER_BUDDY_RELEASE_TAG=release.tag, READER_BUDDY_RELEASE_SHA=release.sha,
               CROSS_CONFIG=str(configuration), LD_BIND_NOW="1", GIT_CONFIG_COUNT="1",
               GIT_CONFIG_KEY_0="safe.directory", GIT_CONFIG_VALUE_0=container_source)
    target_dir = Path(target_dir or repo / "target").resolve()
    packages = {}
    artifacts = {}
    selected_images = tomllib.loads(configuration.read_text())["target"]
    toolchain = {
        "cross_version_command": subprocess.check_output(["cross", "--version"], text=True).strip(),
        "cross_binary_sha256": sha256(Path(shutil.which("cross"))),
        "host_rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True).strip(),
    }
    for target in TARGETS:
        # cross run builds first, then executes --version under target emulation.
        output = subprocess.check_output(
            ["cross", "run", "--locked", "--release", "--target", target, "--target-dir", str(target_dir),
             "--bin", "reader-buddy", "--", "--version"],
            cwd=repo, env=env, text=True).strip()
        if output.split()[-1:] != [release.tag[1:]]:
            raise ValueError(f"{target} reports {output!r}, expected {release.tag[1:]}")
        package = directory / f"reader-buddy-{target}.tar.gz"
        binary = target_dir / target / "release" / "reader-buddy"
        artifacts[target] = qualify(binary, target)
        artifacts[target]["build_image"] = selected_images[target]["image"]
        artifacts[target]["eager_runtime_binding"] = True
        with tarfile.open(package, "w:gz") as archive:
            info = archive.gettarinfo(str(binary), arcname="reader-buddy")
            info.mode = 0o755
            with binary.open("rb") as data:
                archive.addfile(info, data)
        packages[package.name] = sha256(package)
    manifest = dict(tag=release.tag, sha=release.sha, version=release.tag[1:], packages=packages,
                    toolchain=toolchain, artifacts=artifacts)
    (directory / "provenance.json").write_text(json.dumps(manifest, indent=2) + "\n")
    verify_packages(directory, release)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not TAG.fullmatch(args.tag) or not re.fullmatch(r"[0-9a-f]{40}", args.sha):
        parser.error("An exact stable tag and source SHA are required")
    if args.verify_only:
        verify_packages(args.output.resolve(), Release(args.tag, args.sha))
        print(f"Verified package inventory: {args.tag} at {args.sha}")
        return
    if args.source is None:
        parser.error("--source is required when building")
    repo = args.source.resolve()
    if git(repo, "rev-parse", "HEAD") != args.sha or git(repo, "rev-parse", f"refs/tags/{args.tag}^{{commit}}") != args.sha:
        parser.error("Checkout/tag do not match the verified source SHA")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    build_checkout(repo, Release(args.tag, args.sha), output)
    print(f"Verified packages: {args.tag} at {args.sha}")

if __name__ == "__main__":
    main()
