"""Freeze a maintained tool's read-only candidate; no version calculation or tag writes."""
import argparse
from pathlib import Path
from artifacts import Release, source_identity, remote_identity, output
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("candidate", "remote"))
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--require-semantic-source", action="store_true")
    args = parser.parse_args()
    release = Release(args.tag, args.sha)
    if args.mode == "candidate":
        source_identity(Path.cwd(), release)
        if args.require_semantic_source:
            from semantic_source import validate_source
            validate_source(Path.cwd(), release.sha)
        output(tag=release.tag, sha=release.sha)
    else:
        remote_identity(Path.cwd(), release)
