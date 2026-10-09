"""Private Main-only coordinator. Default renders only; transport requires --execute-main."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

HERE = Path(__file__).resolve().parent
ALIAS = "rem25-usb-rm2"
FILES = {"actor.sh", "launch.sh", "runtime.files", "document-immutable.files", "document-mutable.files", "payload.so", "owner",
         "guard.conf", "activation.conf", "stock.policy", "guard.policy", "baseline.files", "service.files",
         "stock-unit.original", "vendor-dropin.original", "unit-shadow.service", "vendor-shadow.conf"}
SPENT = {"0" * 32, "2b34ff126957497285b66f77311df07e", "8287c096cca84cf6a0998b8d5f178c0d", "0404ebf9b5794196a85bfba7ee6859ba", "301a86a1a18d48d885c5ddcec0af549c", "b3e3ca0475a84432a9b328218395de0c",
         "4459cae2f852426b8262f658c6aa59ed", "a22fc58d4c9948098f7d2ccc8a9de8c4", "94f4037a4f3c40b881fe491b9b404bc3", "94774266c10f41e2aa8d410012fee601"}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def regular(path):
    path = Path(path).absolute()
    if not path.is_file() or any(p.is_symlink() or getattr(p, "is_junction", lambda: False)() for p in [path, *path.parents]):
        raise ValueError("Regular file without link ancestors required")
    return path


def load_packet(packet, receipt_hash):
    packet = Path(packet).absolute()
    receipt_bytes = regular(packet / "preparation.json").read_bytes()
    if len(receipt_bytes) > 65536 or sha(receipt_bytes) != receipt_hash:
        raise ValueError("Reviewed receipt mismatch")
    receipt = json.loads(receipt_bytes)
    nonce = receipt.get("nonce", "")
    if not isinstance(nonce, str) or not re.fullmatch("[0-9a-f]{32}", nonce) or nonce in SPENT:
        raise ValueError("Fresh selected nonce required")
    if receipt.get("remote_root") != "/run/rmb-qt-probe-" + nonce or receipt.get("absolute_budget_seconds") != 360:
        raise ValueError("Receipt root/budget refused")
    if receipt.get("experiment_kind") != "caller-selected-insertion-v1":
        raise ValueError("Insertion-specific receipt required")
    if receipt.get("evidence_class") != "prepared only; no native execution" or set(receipt.get("files", {})) != FILES:
        raise ValueError("Complete prepared packet required")
    expected_manifest = ""
    for name, digest in sorted(receipt["files"].items()):
        if not isinstance(digest, str) or not re.fullmatch("[0-9a-f]{64}", digest):
            raise ValueError("Canonical packet digest required")
        if sha(regular(packet / name).read_bytes()) != digest:
            raise ValueError("Packet bytes changed: " + name)
        expected_manifest += f"{digest}  {name}\n"
    manifest = regular(packet / "packet.files").read_bytes()
    if manifest != expected_manifest.encode("ascii") or sha(manifest) != receipt.get("packet_manifest_sha256"):
        raise ValueError("Whole packet manifest mismatch")
    return receipt


def pin_hosts(path, digest):
    data = regular(path).read_bytes()
    if sha(data) != digest or len(data) > 8192:
        raise ValueError("Reviewed host key file mismatch")
    lines = data.decode("ascii").splitlines()
    if len(lines) != 1:
        raise ValueError("One explicit pinned key required")
    parts = lines[0].split()
    if len(parts) != 3 or parts[0] != ALIAS or parts[1] not in {"ssh-ed25519", "ecdsa-sha2-nistp256", "ssh-rsa"}:
        raise ValueError("Explicit fixed host key alias required")
    if not base64.b64decode(parts[2], validate=True):
        raise ValueError("Empty key refused")
    return regular(path)


def ssh_args(known_hosts, identity=None):
    # Bypass user alias/config/proxy resolution and the known WiFi alias.
    pinned_path = Path(known_hosts).as_posix()
    if any(c in pinned_path for c in '\r\n"'):
        raise ValueError("Host key file path refused")
    arguments = ["ssh", "-F", "none", "-o", "HostName=10.11.99.1", "-o", "HostKeyAlias=" + ALIAS,
            "-o", 'UserKnownHostsFile="' + pinned_path + '"', "-o", "GlobalKnownHostsFile=" + ("NUL" if os.name == "nt" else "/dev/null"),
            "-o", "StrictHostKeyChecking=yes", "-o", "BatchMode=yes", "-o", "ConnectTimeout=2",
            "-o", "ConnectionAttempts=1", "-o", "UpdateHostKeys=no", "-o", "ClearAllForwardings=yes",
            "-o", "ControlMaster=no", "-o", "ControlPath=none"]
    if identity is not None:
        arguments += ["-i", str(regular(identity)), "-o", "IdentitiesOnly=yes"]
    return arguments + ["-T", "root@10.11.99.1", "/bin/sh -s"]


def render(receipt, receipt_hash):
    text = regular(HERE / "arm.sh.in").read_text()
    replacements = {"@ROOT@": receipt["remote_root"], "@NONCE@": receipt["nonce"],
                    "@GUARDHASH@": receipt["files"]["guard.conf"], "@MANIFESTHASH@": receipt["packet_manifest_sha256"],
                    "@UNITSHADOWHASH@": receipt["files"]["unit-shadow.service"],
                    "@VENDORSHADOWHASH@": receipt["files"]["vendor-shadow.conf"],
                    "@RECEIPTHASH@": receipt_hash,
                    "@ALLOWEDFILES@": "|".join(sorted(FILES | {"packet.files", "preparation.json"})),
                    "@PRIVATEFILES@": "\n    ".join(f'private "$root/{name}"' for name in sorted(FILES))}
    for key, value in replacements.items():
        text = text.replace(key, value)
    if re.search("@[A-Z]+@", text):
        raise ValueError("Unbound coordinator template")
    return text


def transport(arguments, script, timeout):
    # One call per stage; subprocess timeout kills only the local SSH client.
    # A remote action can be uncertain: never reconnect to repeat admission.
    try:
        result = subprocess.run(arguments, input=script.encode(), capture_output=True, timeout=timeout)
        return {"exit": result.returncode, "timeout": False,
                "stdout": result.stdout.decode("utf-8", "replace"), "stderr": result.stderr.decode("utf-8", "replace")}
    except subprocess.TimeoutExpired as error:
        return {"exit": None, "timeout": True, "stdout": (error.stdout or b"").decode("utf-8", "replace"),
                "stderr": (error.stderr or b"").decode("utf-8", "replace"), "uncertain_remote_action": True}


def monitor_script(receipt):
    root = receipt["remote_root"]
    # Fixed read-only evidence set with per-file caps, no policy or process changes.
    names = ["deadline", "host-arm.claim", "host-actor.identity", "host-actor.properties", "host-guard.properties",
             "guard.request", "start.request", "insertion.outcome", "insertion.failed", "initial-stock-stop",
             "candidate.pre-stop", "candidate.post-stop", "attempt.identity", "callback.json", "diagnostics.json", "stop.request", "stop.admitted", "document.preservation", "document.mutation", "document-immutable-check", "document-mutable-check", "stock.restored",
             "recovery.failed", "query.unconfirmed", "host-query.unconfirmed", "query.stdout", "query.stderr"]
    names += ["stock.policy.observed", "guard.policy.observed", "guard.directory-created",
              "unit-shadow.install-intent", "vendor-shadow.install-intent", "guard-file.install-intent", "final-byte"]
    commands = ["set -eu", f"root='{root}'", "test -d \"$root\" && test ! -L \"$root\""]
    for name in names:
        commands += [f"printf '\\nFILE {name}\\n'", f'if test -f "$root/{name}" && test ! -L "$root/{name}"; then dd if="$root/{name}" bs=18433 count=1 2>/dev/null; fi']
    # Marker indicates files exist only, not proven healthy/native completion.
    commands += ['if test -f "$root/stock.restored" || test -f "$root/recovery.failed"; then printf "\\nEVIDENCE_TERMINAL_PRESENT\\n"; fi']
    return "\n".join(commands) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packet", type=Path, required=True)
    parser.add_argument("--receipt-sha256", required=True)
    parser.add_argument("--known-hosts", type=Path, required=True)
    parser.add_argument("--known-hosts-sha256", required=True)
    parser.add_argument("--identity-file", type=Path, required=True, help="Existing Main SSH identity; checked by metadata only, never read or hashed")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--execute-main", action="store_true")
    args = parser.parse_args()
    receipt = load_packet(args.packet, args.receipt_sha256)
    known_hosts = pin_hosts(args.known_hosts, args.known_hosts_sha256)
    identity = regular(args.identity_file)
    output = args.output.absolute()
    if output.exists() or any(p.is_symlink() or getattr(p, "is_junction", lambda: False)() for p in [output, *output.parents]):
        raise ValueError("New private output required")
    output.mkdir(mode=0o700, parents=True)
    # Freeze public trusted host-key bytes locally; never copy private identity bytes.
    pinned_copy = output / "pinned-known-hosts"
    pinned_copy.write_bytes(known_hosts.read_bytes())
    pin_hosts(pinned_copy, args.known_hosts_sha256)
    script = render(receipt, args.receipt_sha256)
    (output / "arm.sh").write_text(script, encoding="utf-8", newline="\n")
    bindings = {"receipt_sha256": args.receipt_sha256, "known_hosts_sha256": args.known_hosts_sha256,
                "coordinator_sha256": sha(regular(__file__).read_bytes()), "arm_template_sha256": sha(regular(HERE / "arm.sh.in").read_bytes()),
                "rendered_arm_sha256": sha(script.encode()), "nonce": receipt["nonce"], "remote_root": receipt["remote_root"],
                "source_only": not args.execute_main, "no_retry": True}
    (output / "bindings.json").write_text(json.dumps(bindings, indent=2) + "\n")
    if not args.execute_main:
        print(json.dumps(bindings)); return
    arguments = ssh_args(pinned_copy, identity)
    # Separate packet staging must already be completed; no copy or xochitl control.
    started = time.monotonic()
    arm = transport(arguments, script, 55)
    (output / "arming.json").write_text(json.dumps(arm, indent=2) + "\n")
    # Bounded read-only monitoring even after uncertain/failing arm; never replay.
    final = started + 440
    index = 0
    while time.monotonic() < final:
        remaining = final - time.monotonic()
        observed = transport(arguments, monitor_script(receipt), min(3, remaining))
        (output / f"monitor-{index:03}.json").write_text(json.dumps(observed, indent=2) + "\n")
        index += 1
        if not observed["timeout"] and observed["exit"] == 0 and "\nEVIDENCE_TERMINAL_PRESENT\n" in observed["stdout"]:
            break
        time.sleep(min(5, max(0, final-time.monotonic())))
    print(json.dumps({"arming_exit": arm["exit"], "arming_timeout": arm["timeout"], "observations": index,
                      "evidence_directory": str(output), "native_success": "not inferred; Main must verify evidence", "replay_permitted": False}))
    raise SystemExit(0 if not arm["timeout"] and arm["exit"] == 0 else 90)


if __name__ == "__main__":
    main()
