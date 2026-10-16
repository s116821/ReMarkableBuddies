"""Build unmodified maintained upload Action source with an audited minimal recipe."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
from artifacts import git, output, sha256

PIN = "29e53e917877a24fad85510ded594ab3c9ca12de"
UPSTREAM = "https://github.com/svenstaro/upload-release-action.git"
LOCKS = Path(__file__).with_name("vendor-locks")

def qualify(source, destination):
    if git(source, "rev-parse", "HEAD") != PIN or git(source, "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Expected clean exact maintained upstream source")
    if destination.exists():
        raise ValueError("Owned fresh output directory required")
    evidence = {"upstream": UPSTREAM, "commit": PIN, "recipe": "unmodified-library/minimal-build-runtime-v1",
                "original": {}, "selected": {}}
    for name in ("src/main.ts", "tsconfig.json", "action.yml", "LICENSE", "package.json", "package-lock.json"):
        blob = subprocess.check_output(["git", "show", f"HEAD:{name}"], cwd=source)
        evidence["original"][name] = hashlib.sha256(blob).hexdigest()
        # Source bytes follow immutable Git objects on Windows as well.
        (source / name).write_bytes(blob)
    for name in ("package.json", "package-lock.json"):
        selected = LOCKS / f"svenstaro-upload-release-action.{name}"
        shutil.copyfile(selected, source / name)
        evidence["selected"][name] = sha256(selected)
    npm = shutil.which("npm.cmd" if __import__("os").name == "nt" else "npm")
    def run(*args):
        subprocess.run([npm, *args], cwd=source, check=True)
    run("ci", "--ignore-scripts", "--no-audit", "--no-fund")
    audit = subprocess.run([npm, "audit", "--json"], cwd=source, check=False, capture_output=True, text=True)
    report = json.loads(audit.stdout)
    if audit.returncode or any(report["metadata"]["vulnerabilities"][key] for key in ("info", "low", "moderate", "high", "critical", "total")):
        raise ValueError("Selected full build/runtime dependency audit failed")
    run("run", "build")
    run("run", "pack")
    for name in ("src/main.ts", "tsconfig.json", "action.yml", "LICENSE"):
        if sha256(source / name) != evidence["original"][name]:
            raise ValueError("Upstream library/build input changed")
    destination.mkdir(parents=True)
    shutil.copytree(source / "dist", destination / "dist")
    for name in ("action.yml", "LICENSE"):
        shutil.copyfile(source / name, destination / name)
    evidence["audit"] = report["metadata"]["vulnerabilities"]
    evidence["node"] = subprocess.check_output(["node", "--version"], text=True).strip()
    evidence["files"] = {p.relative_to(destination).as_posix(): sha256(p)
                         for p in sorted(destination.rglob("*")) if p.is_file()}
    (destination / "provenance.json").write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    output(bundle_sha256=evidence["files"]["dist/index.js"])
    return evidence

def verify(directory, expected_bundle):
    record = json.loads((directory / "provenance.json").read_text(encoding="utf-8"))
    if record["commit"] != PIN or record["upstream"] != UPSTREAM:
        raise ValueError("Wrong uploader source identity")
    for name in ("package.json", "package-lock.json"):
        if record["selected"][name] != sha256(LOCKS / f"svenstaro-upload-release-action.{name}"):
            raise ValueError("Wrong selected uploader recipe")
    if record["files"]["dist/index.js"] != expected_bundle:
        raise ValueError("Wrong qualified uploader bundle")
    actual = {p.relative_to(directory).as_posix(): sha256(p) for p in directory.rglob("*")
              if p.is_file() and p.relative_to(directory).as_posix() != "provenance.json"}
    if actual != record["files"]:
        raise ValueError("Uploader files changed in transit")
    if record["audit"]["total"] != 0:
        raise ValueError("Uploader dependency gate not passed")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--expected-bundle")
    args = parser.parse_args()
    if args.expected_bundle:
        verify(args.output.resolve(), args.expected_bundle)
    elif args.source:
        qualify(args.source.resolve(), args.output.resolve())
    else:
        parser.error("--source or --expected-bundle required")
