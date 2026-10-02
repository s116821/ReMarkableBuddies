"""Generate local, explicitly non-runnable E0T unit/manifest drafts.

No SSH, service manager, runtime directory, staging or process operation exists here.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import uuid

ROLES = ("cleanup", "controller", "guard", "stock", "fail-a", "fail-b",
         "claim", "norestart", "separate", "notify", "barrier", "queued")


def prepare(destination, nonce):
    if not re.fullmatch("[0-9a-f]{32}", nonce):
        raise ValueError("fixed 128-bit lowercase hexadecimal nonce required")
    destination = Path(destination)
    destination.mkdir(parents=False, exist_ok=False)
    root = "/run/buddy-e0t-" + nonce
    names = {role: "buddy-e0t-" + nonce + "-" + role + ".service" for role in ROLES}
    fragments = []
    owned_paths = [{"path": root, "kind": "directory", "mode": "0700"}]
    for role in ROLES:
        dependencies = ""
        if role == "norestart":
            dependencies += "OnFailure=" + names["fail-a"] + "\n"
        if role == "queued":
            dependencies += "Requires=" + names["barrier"] + "\nAfter=" + names["barrier"] + "\n"
        kind = "notify" if role in ("notify", "barrier") else "simple"
        restart = "Restart=on-failure\nRestartMode=direct\nRestartSec=100ms\n" if role == "claim" else "Restart=no\n"
        watchdog = "WatchdogSec=3s\nNotifyAccess=main\n" if role == "notify" else "WatchdogSec=0\n"
        if role == "barrier":
            watchdog += "NotifyAccess=main\n"
        runtime = "180s" if role == "cleanup" else "15s"
        text = ("[Unit]\nDescription=Owned E0T draft " + role + "\nDefaultDependencies=no\n"
                "StartLimitIntervalSec=180s\nStartLimitBurst=4\nStartLimitAction=none\nFailureAction=none\n"
                "JobTimeoutSec=15s\nJobRunningTimeoutSec=15s\nJobTimeoutAction=none\n" + dependencies
                + "[Service]\nType=" + kind + "\nExecStart=" + root + "/helper " + role + " current\n"
                "User=root\nGroup=root\nUMask=0077\nKillMode=control-group\nDelegate=no\n"
                "TimeoutStartSec=10s\nTimeoutStopSec=2s\nTimeoutAbortSec=2s\nRuntimeMaxSec=" + runtime + "\n"
                "LimitCORE=0\nLimitFSIZE=2048\nMemoryMax=8M\nTasksMax=2\n"
                "StandardInput=null\nStandardOutput=null\nStandardError=null\n"
                "UnsetEnvironment=LD_PRELOAD LD_LIBRARY_PATH\n" + restart + watchdog)
        filename = names[role]
        (destination / filename).write_text(text, newline="\n")
        runtime_path = "/run/systemd/system/" + filename
        owned_paths.append({"path": runtime_path, "kind": "regular", "mode": "0644"})
        fragments.append({"role": role, "name": filename, "path": runtime_path,
                          "sha256": hashlib.sha256(text.encode()).hexdigest()})
    parent = "/run/systemd/system/" + names["norestart"] + ".d"
    dropin = "[Unit]\nOnFailure=\nOnFailure=" + names["fail-b"] + "\n"
    (destination / "norestart-owned.conf").write_text(dropin, newline="\n")
    owned_paths.extend([{"path": parent, "kind": "directory", "mode": "0755"},
                        {"path": parent + "/owned.conf", "kind": "regular", "mode": "0644"}])
    for filename in ("helper", "owner", "case", "claim-T2"):
        owned_paths.append({"path": root + "/" + filename, "kind": "regular",
                            "mode": "0700" if filename == "helper" else "0600"})
    for role in ROLES:
        owned_paths.append({"path": root + "/control-" + role, "kind": "fifo", "mode": "0600"})
    for case in range(1, 9):
        for role in ROLES:
            owned_paths.append({"path": root + f"/events-T{case}-" + role,
                                "kind": "regular", "mode": "0600", "max_bytes": 2048})
    for case in (6, 7, 8):
        files = [f"identity-T{case}-{role}" for role in ("controller", "guard", "stock")]
        files += [f"{kind}-T{case}" for kind in ("state", "restore-claim", "closed", "receipt", "publication-lock")]
        files += [f"{kind}-T{case}-{role}" for kind in ("state-partial", "activation-partial")
                  for role in ("controller", "guard")]
        owned_paths.extend({"path": root + "/" + name, "kind": "regular", "mode": "0600", "max_bytes": 128}
                           for name in files)
    lookup = set()
    for name in names.values():
        stem = name.removesuffix(".service")
        directories = {"service.d", name + ".d"}
        pieces = stem.split("-")
        directories.update("-".join(pieces[:end]) + "-.service.d" for end in range(1, len(pieces)))
        for base in ("/etc/systemd/system", "/run/systemd/system", "/usr/lib/systemd/system",
                     "/usr/local/lib/systemd/system"):
            lookup.update(base + "/" + directory for directory in directories)
    manifest = {"schema": 1, "status": "INCOMPLETE_PREPARATION_ONLY", "runnable": False,
                "nonce": nonce, "root": root, "roles": list(ROLES), "units": fragments,
                "dropin": {"path": parent + "/owned.conf", "sha256": hashlib.sha256(dropin.encode()).hexdigest()},
                "owned_paths": owned_paths, "read_only_dropin_lookup_paths": sorted(lookup),
                "effective_profile_frozen": False, "helper_artifact_sha256": None, "operator_sha256": None,
                "missing_gates": ["experiment cleanup actor implementation",
                                  "independent review of preparation-only process/file actors",
                                  "barrier READY control", "exact effective-property operator gate",
                                  "original baselines and configuration/job drift rejection",
                                  "identity/job/late-writer/cleanup fencing", "per-barrier concurrency measurement",
                                  "artifact imports/build manifest", "independent frozen packet review",
                                  "final coordinator execution authorization"],
                "limits": {"units": 12, "helpers": 8, "tasks": 16, "case_seconds": 15,
                           "cleanup_initiation_seconds": 180, "cleanup_seconds": 30,
                           "stage_bytes": 16777216, "evidence_bytes": 262144}}
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", newline="\n")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", help="new local preparation directory")
    parser.add_argument("--nonce", default=None)
    arguments = parser.parse_args()
    packet = prepare(arguments.destination, arguments.nonce or uuid.uuid4().hex)
    print(json.dumps({"status": packet["status"], "runnable": False, "nonce": packet["nonce"],
                      "unit_count": len(packet["units"]), "path_count": len(packet["owned_paths"])}))
