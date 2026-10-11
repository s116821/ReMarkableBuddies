"""Prepare a fresh Main-selected caller-UUID insertion packet; no transport or execution."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, TOOLS as CAPTURE_TOOLS, HOST_KINDS, OBSERVATION_KIND
from open_document_timing import PRODUCTION_V2, V2_LIBRARY_SHA256, EIGHT_LIBRARY_SHA256, V2_PUBLISHER_SHA256, validate_v2_binding, validate_v2_config, fixture_inventory_selection, PRESERVED_EIGHT, ORIGINAL_PAGES, PRESERVED_PAGES

LIBRARY_TAP_VARIANTS = {
    "primary-1641396": ("--tap-library-v2", [164, 1396]),
    "alternate-2011360": ("--tap-library-v2-alt", [201, 1360]),
}
def library_tap_selection(profile):
    value = profile.get("library_tap_variant")
    if type(value) is not str or value not in LIBRARY_TAP_VARIANTS:
        raise ValueError("Explicit preselected library tap variant required")
    return LIBRARY_TAP_VARIANTS[value]

HERE = Path(__file__).resolve().parent
POLICY = {"OnFailure": "remarkable-fail.service", "OnFailureJobMode": "replace",
          "FailureAction": "none", "StartLimitAction": "none", "Restart": "on-failure",
          "RestartMode": "direct", "TimeoutStopUSec": "1min 30s", "WatchdogUSec": "1min",
          "KillMode": "control-group", "KillSignal": "15", "FinalKillSignal": "9", "SendSIGKILL": "yes"}
SERVICE_HASHES = {
    "stock_unit_sha256": ("/usr/lib/systemd/system/xochitl.service",
                          "adb0a2654ce9ec884f67c0627c22d539d6475dd0af80816819ee80bf13c0e6d6"),
    "vendor_dropin_sha256": ("/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf",
                             "b15560e1dca2f4451b59537c490015aa2f5ea691437bd7ddddc7a65411aaad6e"),
}
EXE_HASH = "071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df"
ENCODER_HASHES = {"/usr/bin/openssl": "73ad5be823389c81d53b032207b2f8d171ab41cb2ce6f757da242ea875d2a33c",
                  "/usr/lib/libssl.so.3": "b39a50a0625cb6f9483d22f8bc00dfd2e46f59f19a2c0d79647a2803fb4ba40f",
                  "/usr/lib/libcrypto.so.3": "b87a447ff09553a9aabe898b36a60904320b42c699f8c256c5e5f762740a984a"}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def regular(path):
    path = path.absolute()
    if any(p.is_symlink() or p.is_junction() for p in [path, *path.parents]) or not path.is_file():
        raise ValueError("Regular input without link ancestors required")
    return path.resolve()


def remove_owned_onfailure(data):
    """Remove exactly the pinned dependency line; preserve every other source byte."""
    if not data or len(data) > 16384 or b"\r" in data or b"\0" in data:
        raise ValueError("Bounded canonical original unit text required")
    section = b""
    removed = 0
    kept = []
    for line in data.splitlines(keepends=True):
        stripped = line.strip()
        if stripped.startswith(b"[") and stripped.endswith(b"]"):
            section = stripped
        if stripped.partition(b"=")[0].strip() == b"OnFailure":
            if section != b"[Unit]" or line != b"OnFailure=remarkable-fail.service\n":
                raise ValueError("Exact scoped dependency assignment required")
            removed += 1
            continue
        kept.append(line)
    if removed != 1:
        raise ValueError("Exactly one original Unit OnFailure assignment required")
    return b"".join(kept)


def partition(selected, files):
    # The coordinator still refuses executing this new mode until the complete
    # bounded host sequence is integrated and independently reviewed.
    open_mode = selected.get("experiment_kind") in HOST_KINDS
    document = selected.get("document_id", "")
    uuid = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
    target = selected.get("target_page_id", "")
    pages = selected.get("before_page_ids")
    if not isinstance(document, str) or not re.fullmatch(uuid, document):
        raise ValueError("Canonical fixture document required")
    variant, open_count = fixture_inventory_selection(selected, 'before_page_ids') if open_mode else ('legacy', 7)
    page_count = open_count if open_mode else 6
    if (not isinstance(pages, list) or len(pages) != page_count or
        any(not isinstance(p, str) or not re.fullmatch(uuid, p) for p in pages) or len(set(pages)) != page_count):
        raise ValueError("Exact distinct original page inventory required")
    if open_mode and pages != list(PRESERVED_PAGES if variant == PRESERVED_EIGHT else ORIGINAL_PAGES):
        raise ValueError("Exact selected fixture order including all preserved pages required")
    if (not isinstance(target, str) or not re.fullmatch(uuid, target) or
        target == "00000000-0000-0000-0000-000000000000" or target == document or target in pages):
        raise ValueError("Fresh distinct canonical target required")
    prefix = "/home/root/.local/share/remarkable/xochitl/" + document
    mutable_paths = {prefix + ".content", prefix + ".metadata"}
    document_files = [e for e in files if e["path"].startswith(prefix + ".") or e["path"].startswith(prefix + "/")]
    runtime_files = [e for e in files if e not in document_files]
    immutable_files = [e for e in document_files if e["path"] not in mutable_paths]
    mutable_files = [e for e in document_files if e["path"] in mutable_paths]
    # The original 24 records are retained, not silently redefined. New-file
    # absence and full before/after content semantics remain Main admission.
    document_count = (18 if variant == PRESERVED_EIGHT else 16) if open_mode else 14
    if len(document_files) != document_count or len(runtime_files) != 10 or {e["path"] for e in mutable_files} != mutable_paths:
        raise ValueError("Exact document/runtime partition and two mutable paths required")
    ink_count = (5 if variant == PRESERVED_EIGHT else 4) if open_mode else 3
    expected_document = {prefix + "." + ext for ext in ("content", "metadata", "local", "pagedata", "pdf")} | {prefix + "/" + p + ".rm" for p in pages[:ink_count]} | {prefix + ".thumbnails/" + p + ".png" for p in pages}
    if {e["path"] for e in document_files} != expected_document:
        raise ValueError("Exact fixture paths required; no substitute preservation files")
    expected_runtime = {"/usr/lib/libQt6" + name + ".so.6.10.3" for name in ("Core", "Qml", "Gui", "Quick", "Network")} | {"/usr/lib/libstdc++.so.6", "/lib/libc.so.6", "/lib/libm.so.6", "/lib/libgcc_s.so.1", "/lib/ld-linux-armhf.so.3"}
    if {e["path"] for e in runtime_files} != expected_runtime:
        raise ValueError("Exact ten reviewed provider paths required")
    return document, target, pages, runtime_files, immutable_files, mutable_files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", type=Path, required=True)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--stock-unit", type=Path, required=True)
    parser.add_argument("--vendor-dropin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    for option in ("tap-guardian", "observe-input", "creation-publisher", "creation-config", "library-reference", "source-body-reference", "source-footer-reference", "native-frame-acquire", "owned-frame", "owned-frame-alarm"):
        parser.add_argument("--" + option, type=Path)
    args = parser.parse_args()
    selection_file, payload = map(regular, [args.selection, args.payload])
    if selection_file.stat().st_size > 64 * 1024:
        raise ValueError("Selection exceeds bound")
    selected = json.loads(selection_file.read_text(encoding="utf-8-sig"))
    kind = selected.get("experiment_kind")
    if kind not in ({"caller-selected-insertion-v1"} | HOST_KINDS):
        raise ValueError("Explicit insertion experiment required; no diagnostic alias")
    if kind == PRODUCTION_V2:
        validate_v2_binding(selected)
        library_tap_selection(selected)
        if args.creation_config is None:
            raise ValueError('Reviewed SDK95 creation configuration input required')
        config = regular(args.creation_config)
        config_text = config.read_text(encoding='utf-8')
        if sha(config) != selected['sdk_configuration_sha256']:
            raise ValueError('Exact reviewed active SDK95 configuration required')
        validate_v2_config(config_text, fixture_inventory_selection(selected, 'before_page_ids')[0])
    if kind in HOST_KINDS and kind != PRODUCTION_V2 and (
        selected.get("development_after_open") is not True or
        selected.get("sdk_revision") != "485ac1a46367bfdf203abe140a8b764728cdf54d"
    ):
        raise ValueError("Explicit gated SDK clock-repair source selection required")
    if kind in HOST_KINDS and not re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", selected.get("boot_id", "")):
        raise ValueError("Canonical selected boot identity required")
    nonce = selected.get("nonce", "")
    if not isinstance(nonce, str) or not re.fullmatch(r"[0-9a-f]{32}", nonce) or nonce == "0" * 32:
        raise ValueError("Fresh Main-selected nonce required; all-zero compile fixture is not native")
    if nonce in {"0404ebf9b5794196a85bfba7ee6859ba", "301a86a1a18d48d885c5ddcec0af549c",
                 "b3e3ca0475a84432a9b328218395de0c", "4459cae2f852426b8262f658c6aa59ed",
                 "a22fc58d4c9948098f7d2ccc8a9de8c4", "94f4037a4f3c40b881fe491b9b404bc3",
                 "94774266c10f41e2aa8d410012fee601", "2b34ff126957497285b66f77311df07e", "8287c096cca84cf6a0998b8d5f178c0d",
                 "026e74d06f7248ae809bf1b1d6a417ba", "ff497d18868042319864c82bb34c0e5e", "65dd38a2467e4d49b45e40e8397a1486", "de415731986b403094dc55676d794c82",
                 "98082ca071a440fda8743bc91d6ea114", "0dc45216e4db4208bf63777058c8b591", "070dcaa51b1c468c984e7c4f3594e913",
                 "6ace746bde9b43d78729348ad9cd475e", "3f851eb632a34886bd659e078efaf4f2",
         "37d089df7dee426c999f9a0edbd0569f", "bdd87fec35e341a68c22157d1ecf5680", "32fff92f69f74d27b6759bce6b8a5483", "8e7ce4d9640b4ee7b0a8461fc3509bf8", "82e586a5e01c47ed94700cf777299c6f", "620d845466be4c1c8640b361eb1b6f94", "aa9a36f548564b839861a7764003fd79", "4c20f012616045029add602c5cf6b7fb", "5d591bf598444cf4b96d7d47b8a750b1", "9005ea4e30ba473b804b37183b44aa9a"}:
        raise ValueError("Historical nonce refused")
    if selected.get("budget_seconds") != 360 or selected.get("original_policy") != POLICY:
        raise ValueError("Exact selected budget/original policy required")
    if any(selected.get(key) != value[1] for key, value in SERVICE_HASHES.items()):
        raise ValueError("Exact original unit/vendor source hashes required")
    unit_file, vendor_file = map(regular, [args.stock_unit, args.vendor_dropin])
    if sha(unit_file) != SERVICE_HASHES["stock_unit_sha256"][1] or sha(vendor_file) != SERVICE_HASHES["vendor_dropin_sha256"][1]:
        raise ValueError("Original unit/vendor input bytes mismatch")
    original_unit, original_vendor = unit_file.read_bytes(), vendor_file.read_bytes()
    unit_shadow = remove_owned_onfailure(original_unit)
    vendor_shadow = remove_owned_onfailure(original_vendor)
    for key in ("stock_pid", "stock_start"):
        if not isinstance(selected.get(key), str) or not re.fullmatch(r"[1-9][0-9]*", selected[key]):
            raise ValueError("Canonical original process identity required")
    if int(selected["stock_pid"]) <= 1:
        raise ValueError("Original process PID refused")
    payload_hash = selected.get("payload_sha256")
    exe_hash = selected.get("executable_sha256")
    if exe_hash != EXE_HASH:
        raise ValueError("Exact selected stock executable hash required")
    if any(not isinstance(x, str) or not re.fullmatch(r"[0-9a-f]{64}", x) for x in (payload_hash, exe_hash)):
        raise ValueError("Exact artifact hashes required")
    if sha(payload) != payload_hash:
        raise ValueError("Selected payload mismatch")
    files = selected.get("protected_files")
    inventory_variant, _ = fixture_inventory_selection(selected, 'before_page_ids') if kind in HOST_KINDS else ('legacy', 7)
    count = (28 if inventory_variant == PRESERVED_EIGHT else 26) if kind in HOST_KINDS else 24
    if not isinstance(files, list) or len(files) != count:
        raise ValueError("Exact fresh mode-specific baseline hash inventory required")
    paths = set()
    for entry in files:
        path = entry.get("path", "")
        digest = entry.get("sha256", "")
        if not re.fullmatch(r"/[A-Za-z0-9_./+-]+", path) or ".." in Path(path).parts or path in paths:
            raise ValueError("Protected path refused")
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError("Protected hash refused")
        paths.add(path)
    document, target, pages, runtime_files, immutable_files, mutable_files = partition(selected, files)
    host_inputs = {}
    if kind in HOST_KINDS:
        if selected.get("encoder_hashes") != ENCODER_HASHES:
            raise ValueError("Fresh verified target encoder/provider binding required")
        if (selected.get("references_reviewed") is not True or
            selected.get("sdk_source_hashes", {}).get("tools/qt_creation_deadline.h") != "c3d8e7124eff6de5868c74f321189ce1f55f8b4111e8983ecdc7eb7c86b3a109" or
            selected.get("publisher_source_sha256") != sha(HERE / "publish_creation_arm.c")):
            raise ValueError("Reviewed references and transitive SDK/publisher source binding required")
        inputs = {"native_frame_acquire": args.native_frame_acquire, "owned_frame": args.owned_frame, "owned_frame_alarm": args.owned_frame_alarm, "owned_tap_guardian": args.tap_guardian, "observe_input": args.observe_input,
                  "publish_creation_arm": args.creation_publisher, "library-reference.png": args.library_reference,
                  "source-body-reference.png": args.source_body_reference,
                  "source-footer-reference.png": args.source_footer_reference}
        declared = selected.get("host_artifacts", {})
        library_pin = EIGHT_LIBRARY_SHA256 if inventory_variant == PRESERVED_EIGHT else V2_LIBRARY_SHA256
        if kind == PRODUCTION_V2 and (declared.get('publish_creation_arm') != V2_PUBLISHER_SHA256 or
                                      type(library_pin) is not str or declared.get('library-reference.png') != library_pin):
            raise ValueError('Exact production v2 publisher/reference required')
        if selected.get("capture_contract") != CAPTURE_CONTRACT or any(declared.get(name) != digest for name, digest in CAPTURE_TOOLS.items()):
            raise ValueError("Exact reviewed native capture source/artifact contract required")
        if set(declared) != set(inputs) or any(path is None for path in inputs.values()):
            raise ValueError("All exact host tools and approved reference artifacts required")
        host_inputs = {name: regular(path) for name, path in inputs.items()}
        if any(sha(path) != declared[name] for name, path in host_inputs.items()):
            raise ValueError("Host tool/reference artifact binding mismatch")
    output = args.output.absolute()
    if output.exists() or any(p.is_symlink() or p.is_junction() for p in [output, *output.parents]):
        raise ValueError("Packet output must be new without link ancestors")
    output.mkdir(mode=0o700, parents=True)
    root = "/run/rmb-qt-probe-" + nonce
    substitutions = {"@ROOT@": root, "@NONCE@": nonce, "@STOCKPID@": selected["stock_pid"],
                     "@STOCKSTART@": selected["stock_start"], "@PAYLOADHASH@": payload_hash, "@EXEHASH@": exe_hash,
                     "@EXPERIMENT@": kind,
                     "@BOOTID@": selected.get("boot_id", "00000000-0000-0000-0000-000000000000"),
                     "@TAPGUARDIANHASH@": selected.get("host_artifacts", {}).get("owned_tap_guardian", "0" * 64)}
    for name in ("actor.sh", "launch.sh"):
        text = regular(HERE / (name + ".in")).read_text()
        for key, value in substitutions.items():
            text = text.replace(key, value)
        if re.search(r"@[A-Z]+@", text):
            raise ValueError("Unbound packet template")
        (output / name).write_text(text, encoding="utf-8", newline="\n")
    shutil.copyfile(payload, output / "payload.so")
    if host_inputs:
        for name, path in host_inputs.items():
            shutil.copyfile(path, output / name)
        profile = {"experiment_kind": kind, "nonce": nonce, "boot_id": selected["boot_id"], "document_id": document,
                   "target_id": target, "page_ids": pages,
                   "immutable_hashes": {e["path"]: e["sha256"] for e in runtime_files + immutable_files},
                   "tools": {name: selected["host_artifacts"][name] for name in (("owned_tap_guardian", "observe_input", "publish_creation_arm") + tuple(CAPTURE_TOOLS))},
                   "capture_contract": CAPTURE_CONTRACT,
                   "publisher_source_sha256": selected["publisher_source_sha256"],
                   "encoder_hashes": selected["encoder_hashes"],
                   "sdk_creation_deadline_sha256": selected["sdk_source_hashes"]["tools/qt_creation_deadline.h"],
                   "host_sequence_contract": "bounded-host-v5",
                   "references": {name: selected["host_artifacts"][name] for name in ("library-reference.png", "source-body-reference.png", "source-footer-reference.png")},
                   "source_sha256": {name: sha(HERE / name) for name in (
                       "open_document_host.py", "open_document_transport.py", "open_document_protocol.py",
                       "open_document_timing.py", "publish_creation_arm.c", "coordinator.py",
                       "prepare_packet.py", "actor.sh.in", "arm.sh.in", "owned_tap_guardian.c", "owned_frame_capture.py")}}
        if kind == PRODUCTION_V2:
            profile.update({key: selected[key] for key in ('library_tap_variant', 'production_contract', 'sdk_revision', 'sdk_source_hashes',
                                                          'sdk_setup_variant', 'sdk_configuration_sha256')})
            profile['host_sequence_contract'] = 'bounded-host-v6-production-v2'
            profile['fixture_inventory_variant'] = inventory_variant
        (output / "host-profile.json").write_text(json.dumps(profile, indent=2) + "\n", encoding="utf-8", newline="\n")
    (output / "stock-unit.original").write_bytes(original_unit)
    (output / "vendor-dropin.original").write_bytes(original_vendor)
    (output / "unit-shadow.service").write_bytes(unit_shadow)
    (output / "vendor-shadow.conf").write_bytes(vendor_shadow)
    (output / "owner").write_text(nonce, encoding="ascii", newline="\n")
    (output / "guard.conf").write_text("[Unit]\nOnFailure=\nFailureAction=none\nStartLimitAction=none\n[Service]\nRestart=no\n", encoding="ascii", newline="\n")
    (output / "activation.conf").write_text(f"[Service]\nExecStart=\nExecStart=/bin/sh {root}/launch.sh\n", encoding="ascii", newline="\n")
    (output / "stock.policy").write_text("".join(f"{k}={v}\n" for k, v in POLICY.items()), encoding="ascii", newline="\n")
    guarded = POLICY | {"OnFailure": "", "Restart": "no"}
    (output / "guard.policy").write_text("".join(f"{k}={v}\n" for k, v in guarded.items()), encoding="ascii", newline="\n")
    (output / "baseline.files").write_text("".join(f"{e['sha256']}  {e['path']}\n" for e in files), encoding="ascii", newline="\n")
    for name, subset in (("runtime.files", runtime_files), ("document-immutable.files", immutable_files), ("document-mutable.files", mutable_files)):
        (output / name).write_text("".join(f"{e['sha256']}  {e['path']}\n" for e in subset), encoding="ascii", newline="\n")
    (output / "service.files").write_text("".join(f"{digest}  {path}\n" for path, digest in SERVICE_HASHES.values()), encoding="ascii", newline="\n")
    for file in output.iterdir():
        file.chmod(0o700 if file.name in (("owned_tap_guardian", "observe_input", "publish_creation_arm") + tuple(CAPTURE_TOOLS)) else 0o600)
    hashes = {p.name: sha(p) for p in sorted(output.iterdir())}
    (output / "packet.files").write_text("".join(f"{v}  {k}\n" for k, v in hashes.items()), encoding="ascii", newline="\n")
    (output / "packet.files").chmod(0o600)
    receipt = {"evidence_class": "prepared only; no native execution", "nonce": nonce, "remote_root": root,
               "experiment_kind": kind, "document_id": document, "target_page_id": target, "before_page_ids": pages,
               "absolute_budget_seconds": 360, "selection_sha256": sha(selection_file), "files": hashes,
               "packet_manifest_sha256": sha(output / "packet.files")}
    if kind == PRODUCTION_V2:
        receipt["library_tap_variant"] = selected["library_tap_variant"]
        receipt['fixture_inventory_variant'] = inventory_variant
    (output / "preparation.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n")
    (output / "preparation.json").chmod(0o600)
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
