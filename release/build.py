"""Build candidates, then independently exercise retained packages. No remote writes."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
from abi import qualify
from artifacts import Release, git, sha256, source_identity, verify_identity as identity, verify_packages as packages

TARGETS = ("armv7-unknown-linux-gnueabihf", "aarch64-unknown-linux-gnu")
NAMES = tuple(f"reader-buddy-{target}.tar.gz" for target in TARGETS)
CONFIG = Path(__file__).with_name("Cross.release.toml").resolve()

def verify_identity(manifest, release):
    identity(manifest, release, NAMES)

def verify_packages(directory, release):
    return packages(directory, release, NAMES)

def build(repo, release, directory):
    source_identity(repo, release)
    directory.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, READER_BUDDY_CANDIDATE_TAG=release.tag,
               READER_BUDDY_CANDIDATE_SHA=release.sha, CROSS_CONFIG=str(CONFIG))
    images = tomllib.loads(CONFIG.read_text())["target"]
    toolchain = {"host_rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True).strip(),
                 "cross_binary_sha256": sha256(Path(shutil.which("cross")))}
    artifacts = {}
    # Keep the old container ownership accommodation scoped inside this clone.
    gitdir = Path(git(repo, "rev-parse", "--absolute-git-dir"))
    with tempfile.TemporaryDirectory(prefix="release-xdg-", dir=gitdir) as config_home:
        home = Path(config_home)
        (home / "git").mkdir()
        git(repo, "config", "--file", str(home / "git/config"), "safe.directory", repo.as_posix())
        env["XDG_CONFIG_HOME"] = str(home)
        for target in TARGETS:
            subprocess.run(["cross", "build", "--locked", "--release", "--target", target,
                            "--bin", "reader-buddy"], cwd=repo, env=env, check=True)
            binary = repo / "target" / target / "release/reader-buddy"
            artifacts[target] = qualify(binary, target)
            artifacts[target]["build_image"] = images[target]["image"]
            archive = directory / f"reader-buddy-{target}.tar.gz"
            with tarfile.open(archive, "w:gz") as package:
                item = package.gettarinfo(str(binary), arcname="reader-buddy")
                item.mode = 0o755
                with binary.open("rb") as stream:
                    package.addfile(item, stream)
    manifest = dict(tag=release.tag, sha=release.sha, version=release.tag[1:],
                    packages={name: sha256(directory / name) for name in NAMES},
                    toolchain=toolchain, artifacts=artifacts)
    (directory / "provenance.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    verify_packages(directory, release)

def integrate(directory, release):
    verify_packages(directory, release)
    images = tomllib.loads(CONFIG.read_text())["target"]
    with tempfile.TemporaryDirectory(prefix="retained-binaries-") as temp:
        root = Path(temp).resolve()
        for target in TARGETS:
            folder = root / target
            folder.mkdir()
            with tarfile.open(directory / f"reader-buddy-{target}.tar.gz") as package:
                members = package.getmembers()
                if len(members) != 1 or members[0].name != "reader-buddy" or not members[0].isreg():
                    raise ValueError("Expected only the regular built binary")
                package.extractall(folder, filter="data")
            binary = folder / "reader-buddy"
            binary.chmod(0o755)
            qualify(binary, target)
            arch = "armv7hf" if target.startswith("armv7") else "aarch64"
            # The pinned cross image's public runner executes the extracted binary;
            # this command has no Cargo/source mount and cannot silently rebuild it.
            result = subprocess.check_output(["docker", "run", "--rm", "--network", "none",
                "-e", "LD_BIND_NOW=1", "-v", f"{folder}:/artifacts:ro",
                "--entrypoint", "/linux-runner", images[target]["image"],
                arch, "/artifacts/reader-buddy", "--version"], text=True).strip()
            if result.split()[-1:] != [release.tag[1:]]:
                raise ValueError(f"Retained {target} version mismatch: {result!r}")
            print(f"Retained {target}: {result}; emulation only, native unqualified")
    verify_packages(directory, release)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("build", "verify", "integrate"))
    parser.add_argument("--source", type=Path, default=Path.cwd())
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    release = Release(args.tag, args.sha)
    if args.mode == "build":
        build(args.source.resolve(), release, args.output.resolve())
    elif args.mode == "verify":
        verify_packages(args.output.resolve(), release)
    else:
        integrate(args.output.resolve(), release)
