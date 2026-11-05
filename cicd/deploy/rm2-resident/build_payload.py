"""Prepare the fixed RM2 resident payload for the Buddy package builder."""

import argparse
import hashlib
import io
import json
from pathlib import Path
import tarfile


HERE = Path(__file__).resolve().parent
LIB = "home/root/.local/lib/remarkable-buddies"
UNITS = "usr/lib/systemd/system"
BUNDLE_SHA256 = "f9c4b44b9baf281bc47c74470ddd8d098853e79383e9323e5ff02ef2846fb477"
SDK_FILES = {"profile.json", "libremarkable-resident.so", "LICENSE", "Dobby-LICENSE", "SHA256SUMS"}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def read_verified(path, expected, limit):
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"Expected a regular file at {path}")
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if not 0 < len(data) <= limit or sha256(data) != expected:
        raise ValueError(f"Size or digest mismatch for {path}")
    return data


def sdk_bundle(path):
    raw = read_verified(path, BUNDLE_SHA256, 8 * 1024 * 1024)
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as archive:
        files = archive.getmembers()
        if (
            len(files) != len(SDK_FILES)
            or {member.name for member in files} != SDK_FILES
            or any(not member.isreg() or not 0 < member.size <= 8 * 1024 * 1024 for member in files)
        ):
            raise ValueError("Unexpected native SDK bundle inventory")
        return {member.name: archive.extractfile(member).read() for member in files}


def build(app, app_sha256, bundle, output):
    binary = read_verified(app, app_sha256, 128 * 1024 * 1024)
    if binary[:6] != b"\x7fELF\x01\x01" or int.from_bytes(binary[18:20], "little") != 40:
        raise ValueError("The resident payload requires the ARMv7 Buddy executable")
    sdk = sdk_bundle(bundle)
    buddy_unit = (HERE / "reader-buddy.service").read_bytes()
    helper_unit = (HERE / "reader-buddy-stock-recovery.service").read_bytes()
    profile = json.loads((HERE / "policy.json").read_bytes())
    for key in ("stock_executable_sha256", "stock_unit_sha256", "stock_dropin_sha256"):
        profile[key] = list(bytes.fromhex(profile[key]))
    profile.update(
        version=6,
        app_sha256=list(bytes.fromhex(app_sha256)),
        native_profile_sha256=list(hashlib.sha256(sdk["profile.json"]).digest()),
        adapter_sha256=list(hashlib.sha256(sdk["libremarkable-resident.so"]).digest()),
        buddy_unit_sha256=list(hashlib.sha256(buddy_unit).digest()),
        helper_unit_sha256=list(hashlib.sha256(helper_unit).digest()),
    )
    files = {
        f"{LIB}/reader-buddy": (binary, 0o755),
        f"{UNITS}/reader-buddy.service": (buddy_unit, 0o644),
        f"{UNITS}/reader-buddy-stock-recovery.service": (helper_unit, 0o644),
        f"{LIB}/resident-recovery.json": (
            (json.dumps(profile, sort_keys=True, separators=(",", ":")) + "\n").encode(),
            0o644,
        ),
        f"{LIB}/resident-profile.json": (sdk["profile.json"], 0o644),
        f"{LIB}/libremarkable-resident.so": (sdk["libremarkable-resident.so"], 0o644),
        f"{LIB}/OpenSDK-LICENSE": (sdk["LICENSE"], 0o644),
        f"{LIB}/Dobby-LICENSE": (sdk["Dobby-LICENSE"], 0o644),
    }
    output.mkdir()
    for name, (data, mode) in files.items():
        path = output / "root" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(data)
        path.chmod(mode)
    receipt = {
        "sdk_bundle_sha256": BUNDLE_SHA256,
        "files": {
            name: {"sha256": sha256(data), "bytes": len(data), "mode": mode}
            for name, (data, mode) in sorted(files.items())
        },
    }
    with (output / "payload.json").open("x", encoding="ascii", newline="\n") as stream:
        json.dump(receipt, stream, indent=2)
        stream.write("\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--app-sha256", required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.app, args.app_sha256, args.bundle, args.output)
