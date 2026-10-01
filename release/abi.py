"""Inspect ELF artifacts and verify their selected runtime baseline; no device/API IO."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess


def inspect_elf(binary):
    command = shlex.split(os.environ.get("READELF", "readelf"))
    def read(*args):
        return subprocess.check_output(command + list(args) + [str(binary)], text=True)
    headers = read("--wide", "--file-header", "--program-headers", "--dynamic")
    symbols = read("--wide", "--dyn-syms")
    versions = read("--wide", "--version-info")
    return parse_elf(headers, symbols, versions)


def parse_elf(headers, symbols, versions):
    def field(name):
        match = re.search(rf"^\s*{name}:\s*(.+)$", headers, re.MULTILINE)
        if not match:
            raise ValueError(f"ELF lacks {name}")
        return match[1].strip()
    interpreter = re.search(r"Requesting program interpreter: ([^\]]+)\]", headers)
    if not interpreter:
        raise ValueError("ELF lacks a dynamic interpreter")
    libraries = re.findall(r"\(NEEDED\).*?\[([^\]]+)\]", headers)
    providers = {}
    library = None
    in_needs = False
    for line in versions.splitlines():
        if "Version needs section" in line:
            in_needs = True
        if not in_needs:
            continue
        match = re.search(r"File: (\S+)", line)
        if match:
            library = match[1]
        match = re.search(r"Name: (\S+).*Version: (\d+)", line)
        if match:
            if not library:
                raise ValueError("Version need has no provider")
            providers[match[2]] = (library, match[1])
    required = {name: [] for name in libraries}
    required_versions = {name: set() for name in libraries}
    for provider, version in providers.values():
        if provider not in required_versions:
            raise ValueError("Version need does not name a NEEDED provider")
        required_versions[provider].add(version)
    weak = []
    for line in symbols.splitlines():
        fields = line.split()
        if len(fields) < 8 or not fields[0].endswith(":") or fields[6] != "UND":
            continue
        name = fields[7]
        if fields[4] == "WEAK":
            weak.append(name)
            continue
        if fields[4] != "GLOBAL":
            continue
        index = re.search(r"\((\d+)\)\s*$", line)
        if not index or index[1] not in providers:
            raise ValueError(f"Strong undefined symbol has no qualified provider: {name}")
        provider, version = providers[index[1]]
        if provider not in required or name != name.split("@")[0] + "@" + version:
            raise ValueError(f"Inconsistent version/provider: {name}")
        if version == "GLIBC_PRIVATE":
            raise ValueError("Private libc ABI is not a supported application interface")
        required[provider].append(name)
    if not libraries or not any(required.values()):
        raise ValueError("No dynamic runtime dependency evidence")
    return dict(elf_class=field("Class"), machine=field("Machine"),
                flags=field("Flags"), interpreter=interpreter[1],
                required_symbols={name: sorted(set(items)) for name, items in required.items()},
                required_versions={name: sorted(items) for name, items in required_versions.items()},
                weak_symbols=sorted(set(weak)))


def verify_runtime(artifact, target, baseline=None):
    expected = {
        "armv7-unknown-linux-gnueabihf": ("ELF32", "ARM", "/lib/ld-linux-armhf.so.3"),
        "aarch64-unknown-linux-gnu": ("ELF64", "AArch64", "/lib/ld-linux-aarch64.so.1"),
    }[target]
    if tuple(artifact[key] for key in ("elf_class", "machine", "interpreter")) != expected:
        raise ValueError(f"ELF architecture/interpreter does not match {target}")
    if target.startswith("armv7") and "hard-float ABI" not in artifact["flags"]:
        raise ValueError("ARMv7 artifact does not declare the hard-float ABI")
    if baseline is None:
        if target.startswith("armv7"):
            raise ValueError("RM2 requires the selected vendor runtime baseline")
        return dict(qualification="selected cross runtime under emulation with LD_BIND_NOW; native unverified")
    if baseline.get("format") != 1 or baseline.get("target") != target:
        raise ValueError("Unsupported runtime baseline format/target")
    for name, symbols in artifact["required_symbols"].items():
        provider = baseline["providers"].get(name)
        if provider is None:
            raise ValueError(f"Runtime baseline lacks NEEDED provider: {name}")
        missing = set(symbols) - set(provider["symbols"])
        if missing:
            raise ValueError(f"Runtime provider {name} lacks required symbols: {sorted(missing)}")
        defined_versions = {symbol.split("@", 1)[1] for symbol in provider["symbols"] if "@" in symbol}
        missing_versions = set(artifact["required_versions"][name]) - defined_versions
        if missing_versions:
            raise ValueError(f"Runtime provider {name} lacks required version definitions: {sorted(missing_versions)}")
    return dict(qualification="required strong symbols match selected vendor SDK sysroot; native app unverified",
                firmware_baseline=baseline["firmware_baseline"],
                sdk_metadata_revision=baseline["sdk_metadata_revision"],
                baseline_source=baseline["source_basis"],
                providers={name: {key: value for key, value in baseline["providers"][name].items() if key != "symbols"}
                           for name in artifact["required_symbols"]})


def qualify(binary, target):
    artifact = inspect_elf(binary)
    baseline_path = Path(__file__).with_name("rm2-runtime-baseline.json")
    baseline = json.loads(baseline_path.read_text()) if target.startswith("armv7") else None
    qualification = verify_runtime(artifact, target, baseline)
    if baseline is not None:
        qualification["baseline_sha256"] = hashlib.sha256(baseline_path.read_bytes()).hexdigest()
    return dict(target=target, elf=artifact, runtime=qualification,
                remarkable_open_sdk_revision=None,
                build_backend="legacy cross toolchain; no ReMarkableOpenSDK consumption claimed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    print(json.dumps(qualify(args.binary, args.target), indent=2))
