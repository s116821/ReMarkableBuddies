"""Read-only exact release/retained-byte proofs around maintained publisher Actions."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import urllib.error
import urllib.parse
import urllib.request
from artifacts import Release, output, sha256
from build import verify_packages

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None

def download_asset(repository, asset_id, expected_size, expected_hash, token):
    # Never forward the API credential to the signed asset host.
    url = f"https://api.github.com/repos/{repository}/releases/assets/{asset_id}"
    request = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}",
        "Accept": "application/octet-stream", "X-GitHub-Api-Version": "2022-11-28"})
    try:
        response = urllib.request.build_opener(NoRedirect).open(request, timeout=30)
    except urllib.error.HTTPError as error:
        if error.code != 302:
            raise
        location = error.headers.get("Location", "")
        parsed = urllib.parse.urlsplit(location)
        if parsed.scheme != "https" or parsed.hostname != "release-assets.githubusercontent.com" or parsed.username or parsed.password:
            raise ValueError("Unexpected release asset redirect")
        response = urllib.request.build_opener(NoRedirect).open(location, timeout=30)
    import hashlib
    digest = hashlib.sha256()
    count = 0
    with response:
        while chunk := response.read(1024 * 1024):
            count += len(chunk)
            if count > expected_size:
                raise ValueError("Retained asset exceeds expected bytes")
            digest.update(chunk)
    if count != expected_size or digest.hexdigest() != expected_hash:
        raise ValueError("Retained release asset bytes differ")

def observe(data, expected_tag):
    repository = data.get("repository")
    if type(repository) is not dict or "release" not in repository:
        raise ValueError("Incomplete exact release observation")
    release = repository["release"]
    if release is None:
        return None
    if type(release) is not dict or release.get("tagName") != expected_tag:
        raise ValueError("Wrong exact release")
    if type(release.get("databaseId")) is not int or not 0 < release["databaseId"] < 2**53:
        raise ValueError("Invalid release ID")
    if type(release.get("isDraft")) is not bool:
        raise ValueError("Invalid draft state")
    assets = release.get("releaseAssets")
    if type(assets) is not dict or type(assets.get("totalCount")) is not int or not 0 <= assets["totalCount"] <= 4:
        raise ValueError("Unbounded release inventory")
    return release

def observe_assets(data, expected_tag):
    if type(data) is not dict or type(data.get("assets")) is not list or len(data["assets"]) > 4:
        raise ValueError("Incomplete or oversized exact-ID REST observation")
    release = {"databaseId": data.get("id"), "tagName": data.get("tag_name"),
               "isDraft": data.get("draft"), "description": data.get("body"),
               "releaseAssets": {"totalCount": len(data["assets"]), "nodes": [
                   {"databaseId": item.get("id"), "name": item.get("name"), "size": item.get("size")}
                   for item in data["assets"]]}}
    observe({"repository": {"release": release}}, expected_tag)
    nodes = release["releaseAssets"]["nodes"]
    if type(nodes) is not list or len(nodes) != release["releaseAssets"]["totalCount"]:
        raise ValueError("Incomplete asset inventory")
    if len({asset.get("name") for asset in nodes}) != len(nodes):
        raise ValueError("Duplicate asset names")
    for asset in nodes:
        if (type(asset.get("databaseId")) is not int or not 0 < asset["databaseId"] < 2**53
                or type(asset.get("size")) is not int or asset["size"] < 0):
            raise ValueError("Invalid asset ID/size")
    return release

def merged_source(data, sha):
    commit = data.get("repository", {}).get("object")
    if type(commit) is not dict or commit.get("oid") != sha:
        raise ValueError("Wrong observed source")
    prs = commit.get("associatedPullRequests", {}).get("nodes")
    if type(prs) is not list or not any(
        type(pr) is dict and pr.get("merged") is True and pr.get("baseRefName") == "main"
        and pr.get("mergeCommit", {}).get("oid") == sha for pr in prs):
        raise ValueError("Release requires exact merged main PR, including squash merges")

def retained(directory, candidate, observation, repository, token, missing, require_complete=False, downloader=download_asset):
    manifest = verify_packages(directory, candidate)
    expected = dict(manifest["packages"], **{"provenance.json": sha256(directory / "provenance.json")})
    release = observe_assets(observation, candidate.tag)
    present = set()
    for asset in release["releaseAssets"]["nodes"]:
        name = asset["name"]
        if name not in expected or asset["size"] != (directory / name).stat().st_size:
            raise ValueError("Unexpected retained release asset")
        downloader(repository, asset["databaseId"], asset["size"], expected[name], token)
        present.add(name)
    absent = set(expected) - present
    if require_complete and absent or not release["isDraft"] and absent:
        raise ValueError("Incomplete release inventory")
    files = []
    if release["isDraft"] and not require_complete:
        missing.mkdir(parents=True, exist_ok=False)
        for name in sorted(absent):
            destination = missing / name
            shutil.copyfile(directory / name, destination)
            files.append(destination.resolve().as_posix())
    record = dict(manifest, provenance_sha256=expected["provenance.json"])
    marker = "manager" if any(name.startswith("manager-") for name in manifest["packages"]) else "reader-buddy"
    notes = f"{candidate.tag}\n\nSource: {candidate.sha}\n\n<!-- {marker}-complete:{json.dumps(record, separators=(',', ':'))} -->"
    if not release["isDraft"] and release.get("description") != notes:
        raise ValueError("Published completion provenance differs")
    Path("release-notes.txt").write_text(notes, encoding="utf-8")
    output(release_id=release["databaseId"], draft=str(release["isDraft"]).lower(),
           absent="false", files=";".join(files), notes=notes)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("merged", "lookup", "retained"))
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--packages", type=Path, default=Path("packages"))
    parser.add_argument("--missing", type=Path, default=Path("missing-packages"))
    parser.add_argument("--complete", action="store_true")
    parser.add_argument("--release-id", type=int)
    args = parser.parse_args()
    candidate = Release(args.tag, args.sha)
    data = json.loads(os.environ["RELEASE_OBSERVATION"])
    if args.mode == "merged":
        merged_source(data, candidate.sha)
    elif args.mode == "lookup":
        release = observe(data, candidate.tag)
        output(release_id=release["databaseId"] if release else 0, absent=str(release is None).lower())
    else:
        current = observe_assets(data, candidate.tag)
        if args.release_id is not None and (current is None or current["databaseId"] != args.release_id):
            raise ValueError("Exact release ID changed")
        repository = os.environ["GITHUB_REPOSITORY"]
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            raise ValueError("Malformed repository")
        retained(args.packages, candidate, data, repository, os.environ["GITHUB_TOKEN"],
                 args.missing, args.complete)
