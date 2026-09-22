"""Shared path/acceptance policy; semantic version arithmetic belongs to git-cliff."""

import argparse
import fnmatch
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

CONFIG = Path(__file__).with_name("cliff.toml")
CLIFF_VERSION = "2.14.2"
TAG = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")
TITLE = re.compile(r"(?:feat|fix|perf|refactor|build|ci|chore|test|revert)\([^()\r\n]+\)!?: .+")


def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True, encoding="utf-8").strip()


def documentation(path):
    with CONFIG.open("rb") as source:
        patterns = tomllib.load(source)["git"]["exclude_paths"]
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def relevant(paths):
    return any(path and not documentation(path) for path in paths)


def changed_paths(repo, base, head, *, history=False):
    if history:
        # A net diff loses an application edit followed by its revert.
        output = git(repo, "log", "--format=", "--name-only", "-z", "--no-renames",
                     "--first-parent", "-m", f"{base}..{head}")
    else:
        output = git(repo, "diff", "--name-only", "-z", "--no-renames", base, head)
    return [path.strip("\n") for path in output.split("\0") if path.strip("\n")]


def commit_relevant(repo, sha):
    return relevant(git(repo, "diff-tree", "--root", "--no-commit-id", "--name-only",
                        "--no-renames", "-r", "-z", sha).split("\0"))


def validate_title(title):
    if not TITLE.fullmatch(title.splitlines()[0] if title else ""):
        raise ValueError("Application changes need a scoped feat/fix/perf/refactor/build/ci/chore/test/revert title; docs is not an application release type")


def classify_event(repo, event, event_name):
    if event_name == "workflow_dispatch":
        return True
    if event_name == "pull_request":
        pr = event["pull_request"]
        base = git(repo, "merge-base", pr["base"]["sha"], pr["head"]["sha"])
        application = relevant(changed_paths(repo, base, pr["head"]["sha"]))
        if application:
            validate_title(pr["title"])
        return application
    if event_name != "push":
        raise ValueError(f"Unsupported event: {event_name}")
    base, head = event["before"], event["after"]
    if not base.strip("0"):
        raise ValueError("A new main branch requires an explicit reviewed baseline")
    return relevant(changed_paths(repo, base, head, history=True))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_PATH"))
    parser.add_argument("--event-name", default=os.environ.get("GITHUB_EVENT_NAME"))
    args = parser.parse_args()
    application = classify_event(Path.cwd(), json.loads(Path(args.event).read_text()), args.event_name)
    value = f"application={str(application).lower()}"
    print(value)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as dest:
            dest.write(value + "\n")


if __name__ == "__main__":
    main()
