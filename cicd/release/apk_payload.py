"""Construct the APK file inventory from the frozen source-build inputs."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import runpy
import tempfile

from apk_contract import ORDINARY_BINARY, RM2_PROFILE, canonical, require


LICENSE = "usr/share/remarkable-buddies/LICENSE"
METADATA = "usr/share/remarkable-buddies/build.json"
ORDINARY_FILES = {ORDINARY_BINARY: 0o755, LICENSE: 0o644, METADATA: 0o644}
TOOL_FILES = ("build_payload.py", "policy.json", "reader-buddy.service",
              "reader-buddy-stock-recovery.service")
HERE = Path(__file__).resolve().parent
BUNDLE_FILE = "rm2-native-bundle.tar.gz"


def selected_bundle(inputs, build):
    bundle = inputs / BUNDLE_FILE
    present = bundle.exists() or bundle.is_symlink()
    resident = build["build"]["profile"] == RM2_PROFILE
    require(resident == (present and build["target"]["apk_arch"] == "armv7"),
            "resident option and pinned SDK bundle must agree")
    return bundle if resident else None


def directories(layout):
    return {str(parent) for path in layout for parent in PurePosixPath(path).parents
            if str(parent) != "."}


def copy_input(origin, destination, limit, mode, epoch):
    require(origin.is_file() and not origin.is_symlink(), "regular internal build input")
    with origin.open("rb") as source:
        data = source.read(limit + 1)
    require(0 < len(data) <= limit, "input file bound")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as output:
        output.write(data)
    destination.chmod(mode)
    os.utime(destination, (epoch, epoch))
    return {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def prepare(source, build, bundle, tree):
    """Return the expected hashes and modes and materialize the exact package tree."""
    resident = build["build"]["profile"] == RM2_PROFILE
    require(not resident or bundle is not None, "resident build requires pinned SDK bundle")
    tree.mkdir()
    tree.chmod(0o755)
    epoch = build["source"]["epoch"]
    layout = {LICENSE: 0o644, METADATA: 0o644}
    expected = {
        LICENSE: copy_input(source / "LICENSE", tree / LICENSE, 65536, 0o644, epoch),
        METADATA: copy_input(source / "build.json", tree / METADATA, 65536, 0o644, epoch),
    }
    require((tree / METADATA).read_bytes() == canonical(build), "canonical build metadata")
    binary = build["binary"]
    if resident:
        tools = HERE / "rm2-resident"
        if not tools.is_dir():
            tools = HERE.parent / "deploy/rm2-resident"
        assemble = runpy.run_path(str(tools / "build_payload.py"))["build"]
        with tempfile.TemporaryDirectory(dir=tree.parent) as temporary:
            output = Path(temporary) / "payload"
            assemble(source / "reader-buddy", binary["sha256"], bundle, output)
            files = json.loads((output / "payload.json").read_bytes())["files"]
            for path, record in files.items():
                mode = record["mode"]
                expected[path] = copy_input(output / "root" / path, tree / path,
                                            record["bytes"], mode, epoch)
                require(expected[path] == {"bytes": record["bytes"], "sha256": record["sha256"]},
                        "resident payload input correspondence")
                layout[path] = mode
    else:
        layout[ORDINARY_BINARY] = 0o755
        expected[ORDINARY_BINARY] = copy_input(source / "reader-buddy", tree / ORDINARY_BINARY,
                                               134217728, 0o755, epoch)
    require(expected[binary["path"]] == {"bytes": binary["bytes"], "sha256": binary["sha256"]},
            "actual executable identity")
    for directory in directories(layout):
        path = tree / directory
        path.chmod(0o755)
        os.utime(path, (epoch, epoch))
    return expected, layout
