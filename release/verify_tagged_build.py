"""Exercise the production builder in an isolated clone with no remote.

Fixture tags exist only in the disposable clone. This verifies both real target
binaries/packages under emulation; it does not test live GitHub publication.
"""

import argparse
from pathlib import Path
import tempfile

from coordinator import Release, build_checkout
from policy import TAG, git


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--tag", default="v9999.0.0")
    args = parser.parse_args()
    if not TAG.fullmatch(args.tag):
        parser.error("Fixture tag must have the form vMAJOR.MINOR.PATCH")
    repo = Path.cwd()
    sha = git(repo, "rev-parse", "HEAD")
    if git(repo, "status", "--porcelain", "--untracked-files=normal"):
        parser.error("Commit changes before verifying an exact source revision")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target_dir = args.target_dir.resolve() if args.target_dir else None
    with tempfile.TemporaryDirectory(prefix="reader-tagged-build-", ignore_cleanup_errors=True) as temp:
        source = Path(temp) / "source"
        git(repo, "clone", "--config", "core.autocrlf=false", "--no-local", "--no-checkout", str(repo), str(source))
        git(source, "remote", "remove", "origin")
        git(source, "checkout", "--detach", sha)
        git(source, "tag", args.tag, sha)
        build_checkout(source, Release(args.tag, sha), output, target_dir)
    print(f"Verified both emulated binaries and packages: {args.tag} at {sha}")
    print(output / "provenance.json")


if __name__ == "__main__":
    main()
