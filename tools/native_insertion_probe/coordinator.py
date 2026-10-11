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
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, TOOLS as CAPTURE_TOOLS, HOST_KINDS, OBSERVATION_KIND
from open_document_timing import PRODUCTION_V2, V2_LIBRARY_SHA256, EIGHT_LIBRARY_SHA256, V2_PUBLISHER_SHA256, validate_v2_binding, fixture_inventory_selection, PRESERVED_EIGHT

HERE = Path(__file__).resolve().parent
ALIAS = "rem25-usb-rm2"
FILES = {"actor.sh", "launch.sh", "runtime.files", "document-immutable.files", "document-mutable.files", "payload.so", "owner",
         "guard.conf", "activation.conf", "stock.policy", "guard.policy", "baseline.files", "service.files",
         "stock-unit.original", "vendor-dropin.original", "unit-shadow.service", "vendor-shadow.conf"}
HOST_FILES = {"host-profile.json", "native_frame_acquire", "owned_frame", "owned_frame_alarm", "owned_tap_guardian", "observe_input", "publish_creation_arm", "library-reference.png", "source-body-reference.png", "source-footer-reference.png"}


def packet_files(receipt):
    return FILES | (HOST_FILES if receipt.get("experiment_kind") in HOST_KINDS else set())


def run_host_sequence(sequence, purpose):
    if purpose == OBSERVATION_KIND:
        return sequence.run_diagnostic()
    if purpose in {'open-document-insertion-v1', PRODUCTION_V2}:
        return sequence.run()
    raise ValueError('Unknown host purpose refused')


def load_host_profile(packet, receipt):
    try:
        profile = json.loads(regular(Path(packet) / "host-profile.json").read_bytes())
        if profile.get('experiment_kind') != receipt.get('experiment_kind') or profile.get('experiment_kind') not in HOST_KINDS:
            raise ValueError('Explicit matching host purpose required')
        sources = {"open_document_host.py", "open_document_transport.py", "open_document_protocol.py",
                   "open_document_timing.py", "publish_creation_arm.c", "coordinator.py",
                   "prepare_packet.py", "actor.sh.in", "arm.sh.in", "owned_tap_guardian.c", "owned_frame_capture.py"}
        contract = 'bounded-host-v6-production-v2' if profile['experiment_kind'] == PRODUCTION_V2 else 'bounded-host-v5'
        if profile.get("host_sequence_contract") != contract or set(profile.get("source_sha256", {})) != sources:
            raise ValueError("Host source contract incomplete")
        if any(sha(regular(HERE / name).read_bytes()) != digest for name, digest in profile["source_sha256"].items()):
            raise ValueError("Reviewed host source changed")
        if profile['experiment_kind'] == PRODUCTION_V2:
            validate_v2_binding(profile)
            from prepare_packet import library_tap_selection
            library_tap_selection(profile)
            library_tap_selection(receipt)
            if profile["library_tap_variant"] != receipt["library_tap_variant"]:
                raise ValueError("Host/packet tap selection mismatch")
            inventory, _ = fixture_inventory_selection(profile)
            if fixture_inventory_selection(receipt, 'before_page_ids')[0] != inventory:
                raise ValueError('Host/packet fixture inventory selection mismatch')
            library_pin = EIGHT_LIBRARY_SHA256 if inventory == PRESERVED_EIGHT else V2_LIBRARY_SHA256
            if (profile.get('tools', {}).get('publish_creation_arm') != V2_PUBLISHER_SHA256 or
                type(library_pin) is not str or profile.get('references', {}).get('library-reference.png') != library_pin):
                raise ValueError('Production v2 publisher/reference binding changed')
        if profile["nonce"] != receipt["nonce"] or profile["document_id"] != receipt["document_id"] or profile["target_id"] != receipt["target_page_id"] or profile["page_ids"] != receipt["before_page_ids"]:
            raise ValueError("Host/packet identity mismatch")
        if profile["sdk_creation_deadline_sha256"] != "c3d8e7124eff6de5868c74f321189ce1f55f8b4111e8983ecdc7eb7c86b3a109":
            raise ValueError("Transitive SDK clock source missing")
        if profile["publisher_source_sha256"] != sha(regular(HERE / "publish_creation_arm.c").read_bytes()):
            raise ValueError("Publisher source binding changed")
        expected = {}
        fixture_count = fixture_inventory_selection(profile)[1]
        for name, count in (("runtime.files", 10), ("document-immutable.files", 16 if fixture_count == 8 else 14)):
            lines = regular(Path(packet) / name).read_text(encoding="ascii").splitlines()
            if len(lines) != count:
                raise ValueError("Complete immutable partition required")
            for line in lines:
                digest, path = line.split("  ", 1)
                if not re.fullmatch("[0-9a-f]{64}", digest) or not path.startswith("/") or path in expected:
                    raise ValueError("Exact unique immutable path required")
                expected[path] = digest
        if profile["immutable_hashes"] != expected:
            raise ValueError("Host immutable baseline differs from actor")
        tool_names = {"owned_tap_guardian", "observe_input", "publish_creation_arm"} | set(CAPTURE_TOOLS)
        if profile["tools"] != {name: receipt["files"][name] for name in tool_names}:
            raise ValueError("Host helper hashes differ from packet")
        from prepare_packet import ENCODER_HASHES
        if profile["encoder_hashes"] != ENCODER_HASHES:
            raise ValueError("Exact reviewed target encoder/provider closure required")
        if profile.get("capture_contract") != CAPTURE_CONTRACT or any(profile["tools"][name] != digest for name, digest in CAPTURE_TOOLS.items()):
            raise ValueError("Reviewed native owned-frame capture artifacts required")
        if not re.fullmatch("[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", profile["boot_id"]):
            raise ValueError("Canonical boot identity required")
        from open_document_host import pixels
        reference_names = {"library-reference.png", "source-body-reference.png", "source-footer-reference.png"}
        if profile["references"] != {name: receipt["files"][name] for name in reference_names}:
            raise ValueError("Complete separate reference bindings required")
        for name in reference_names:
            data = regular(Path(packet) / name).read_bytes()
            if sha(data) != profile["references"][name]:
                raise ValueError("Reference bytes changed")
            pixels(data)
        return profile
    except (KeyError, ValueError, OSError) as error:
        raise ValueError("Open-document execution refused: exact host/profile/reference contract missing") from error
SPENT = {"0" * 32, "2b34ff126957497285b66f77311df07e", "8287c096cca84cf6a0998b8d5f178c0d", "0404ebf9b5794196a85bfba7ee6859ba", "301a86a1a18d48d885c5ddcec0af549c", "b3e3ca0475a84432a9b328218395de0c",
         "4459cae2f852426b8262f658c6aa59ed", "a22fc58d4c9948098f7d2ccc8a9de8c4", "94f4037a4f3c40b881fe491b9b404bc3", "94774266c10f41e2aa8d410012fee601",
         "026e74d06f7248ae809bf1b1d6a417ba", "ff497d18868042319864c82bb34c0e5e", "65dd38a2467e4d49b45e40e8397a1486", "de415731986b403094dc55676d794c82",
         "98082ca071a440fda8743bc91d6ea114", "0dc45216e4db4208bf63777058c8b591", "070dcaa51b1c468c984e7c4f3594e913",
         "6ace746bde9b43d78729348ad9cd475e", "3f851eb632a34886bd659e078efaf4f2",
         "37d089df7dee426c999f9a0edbd0569f", "bdd87fec35e341a68c22157d1ecf5680", "32fff92f69f74d27b6759bce6b8a5483", "8e7ce4d9640b4ee7b0a8461fc3509bf8", "82e586a5e01c47ed94700cf777299c6f", "620d845466be4c1c8640b361eb1b6f94", "aa9a36f548564b839861a7764003fd79", "4c20f012616045029add602c5cf6b7fb", "5d591bf598444cf4b96d7d47b8a750b1", "9005ea4e30ba473b804b37183b44aa9a"}


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
    if receipt.get("experiment_kind") not in ({"caller-selected-insertion-v1"} | HOST_KINDS):
        raise ValueError("Insertion-specific receipt required")
    if receipt.get("evidence_class") != "prepared only; no native execution" or set(receipt.get("files", {})) != packet_files(receipt):
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
    replacements = {"@ROOT@": receipt["remote_root"], "@CAPTURESPACE@": 'test "$(df -Pk /run | awk \'NR==2 {print $4}\')" -ge 196608' if receipt.get("experiment_kind") in HOST_KINDS else ":", "@NONCE@": receipt["nonce"],
                    "@GUARDHASH@": receipt["files"]["guard.conf"], "@MANIFESTHASH@": receipt["packet_manifest_sha256"],
                    "@UNITSHADOWHASH@": receipt["files"]["unit-shadow.service"],
                    "@VENDORSHADOWHASH@": receipt["files"]["vendor-shadow.conf"],
                    "@RECEIPTHASH@": receipt_hash,
                    "@ALLOWEDFILES@": "|".join(sorted(packet_files(receipt) | {"packet.files", "preparation.json"})),
                    "@PRIVATEFILES@": "\n    ".join(
                        f'test -f "$root/{name}" && test ! -L "$root/{name}"\n    test "$(stat -c \'%a %u %h\' \"$root/{name}\")" = \'700 0 1\''
                        if name in ({"owned_tap_guardian", "observe_input", "publish_creation_arm"} | set(CAPTURE_TOOLS)) else f'private "$root/{name}"'
                        for name in sorted(packet_files(receipt)))}
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
              "host-prepublication-runtime.files.check", "host-prepublication-document-immutable.files.check",
              "tap.ready", "tap.go", "tap.released", "tap.empty", "tap.recovered", "tap.cleaned", "tap.unresolved", "tap-guardian.log", "tap-recovery.log", "input.recovery.failed",
              "tap.supervisor-complete", "tap.supervisor-spent",
              "tap.go-publication.json", "tap.go-publication.log", "tap.go.tmp",
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
    profile = load_host_profile(args.packet, receipt) if receipt["experiment_kind"] in HOST_KINDS else None
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
    host_error = None
    if profile is not None and not arm["timeout"] and arm["exit"] == 0:
        from open_document_host import HostSequence
        from open_document_transport import SSHAdapter
        adapter = SSHAdapter(arguments, receipt, profile, output, transport,
                             regular(args.packet / "library-reference.png").read_bytes())
        try:
            sequence = HostSequence(adapter, profile, regular(args.packet / "library-reference.png").read_bytes(),
                                    regular(args.packet / "source-body-reference.png").read_bytes(),
                                    regular(args.packet / "source-footer-reference.png").read_bytes())
            report = run_host_sequence(sequence, profile['experiment_kind'])
            (output / "host-report.json").write_text(json.dumps(report, indent=2) + "\n")
        except Exception as error:
            # Permission/input uncertainty never triggers a second attempt.
            # Independent actor recovery and bounded read-only monitoring remain.
            host_error = {"type": type(error).__name__, "message": str(error), "retry_permitted": False}
            (output / "host-failure.json").write_text(json.dumps(host_error, indent=2) + "\n")
        finally:
            try:
                adapter.restore_probe()
            except Exception as error:
                host_error = {"type": type(error).__name__, "message": str(error), "retry_permitted": False, "cleanup_failed": True}
                (output / "probe-restore-failure.json").write_text(json.dumps({"type": type(error).__name__, "message": str(error)}) + "\n")
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
                      "evidence_directory": str(output), "host_error": host_error,
                      "native_success": "not inferred; Main must verify evidence", "replay_permitted": False}))
    raise SystemExit(0 if not arm["timeout"] and arm["exit"] == 0 and host_error is None else 90)


if __name__ == "__main__":
    main()
