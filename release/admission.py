"""Reject ambiguous push ranges and bind publication to a main PR merge. Read-only."""
import argparse
import json
import os
from pathlib import Path
import re
from artifacts import git
from verify_release import merged_source

def validate_range(repo, before, sha):
    if (not re.fullmatch(r"[0-9a-f]{40}", before) or before == "0" * 40
            or not re.fullmatch(r"[0-9a-f]{40}", sha)):
        raise ValueError("Initial/unknown main history needs an explicit reviewed baseline")
    if git(repo, "merge-base", before, sha) != before:
        raise ValueError("Non-forward main range refused")
    commits = git(repo, "rev-list", "--first-parent", f"{before}..{sha}").splitlines()
    if commits != [sha]:
        raise ValueError("Ambiguous batch main push refused before build/tag; one first-parent merge required")

def validate(event, event_name, source_sha, repository, observation):
    if (event_name != "push" or event.get("ref") != "refs/heads/main"
            or event.get("after") != source_sha or event.get("forced") is not False
            or event.get("created") is not False or event.get("deleted") is not False
            or event.get("repository", {}).get("full_name") != repository
            or repository not in ("s116821/ReMarkableBuddies", "s116821/ReMarkableBuddiesManager")):
        raise ValueError("Only the exact canonical main merge may tag/publish")
    merged_source(observation, source_sha)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("range", "merge"))
    parser.add_argument("--before")
    parser.add_argument("--sha", required=True)
    args = parser.parse_args()
    if args.mode == "range":
        validate_range(Path.cwd(), args.before or "", args.sha)
    else:
        with open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8") as source:
            validate(json.load(source), os.environ["GITHUB_EVENT_NAME"], args.sha,
                     os.environ["GITHUB_REPOSITORY"], json.loads(os.environ["RELEASE_OBSERVATION"]))