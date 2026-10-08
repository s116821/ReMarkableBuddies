"""Prepare a fresh Main-selected lifecycle diagnostic packet; no transport or execution."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil

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


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def regular(path):
    path = path.absolute()
    if any(p.is_symlink() or p.is_junction() for p in [path, *path.parents]) or not path.is_file():
        raise ValueError("Regular input without link ancestors required")
    return path.resolve()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", type=Path, required=True)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    selection_file, payload = map(regular, [args.selection, args.payload])
    if selection_file.stat().st_size > 64 * 1024:
        raise ValueError("Selection exceeds bound")
    selected = json.loads(selection_file.read_text(encoding="utf-8-sig"))
    nonce = selected.get("nonce", "")
    if not isinstance(nonce, str) or not re.fullmatch(r"[0-9a-f]{32}", nonce) or nonce == "0" * 32:
        raise ValueError("Fresh Main-selected nonce required; all-zero compile fixture is not native")
    if nonce in {"0404ebf9b5794196a85bfba7ee6859ba", "301a86a1a18d48d885c5ddcec0af549c",
                 "b3e3ca0475a84432a9b328218395de0c"}:
        raise ValueError("Historical nonce refused")
    if selected.get("budget_seconds") != 360 or selected.get("original_policy") != POLICY:
        raise ValueError("Exact selected budget/original policy required")
    if any(selected.get(key) != value[1] for key, value in SERVICE_HASHES.items()):
        raise ValueError("Exact original unit/vendor source hashes required")
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
    if not isinstance(files, list) or len(files) != 24:
        raise ValueError("Exact fresh 14-document/10-baseline hash inventory required")
    paths = set()
    for entry in files:
        path = entry.get("path", "")
        digest = entry.get("sha256", "")
        if not re.fullmatch(r"/[A-Za-z0-9_./-]+", path) or ".." in Path(path).parts or path in paths:
            raise ValueError("Protected path refused")
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError("Protected hash refused")
        paths.add(path)
    output = args.output.absolute()
    if output.exists() or any(p.is_symlink() or p.is_junction() for p in [output, *output.parents]):
        raise ValueError("Packet output must be new without link ancestors")
    output.mkdir(mode=0o700, parents=True)
    root = "/run/rmb-qt-shutdown-" + nonce
    substitutions = {"@ROOT@": root, "@NONCE@": nonce, "@STOCKPID@": selected["stock_pid"],
                     "@STOCKSTART@": selected["stock_start"], "@PAYLOADHASH@": payload_hash, "@EXEHASH@": exe_hash}
    for name in ("actor.sh", "launch.sh"):
        text = regular(HERE / (name + ".in")).read_text()
        for key, value in substitutions.items():
            text = text.replace(key, value)
        if re.search(r"@[A-Z]+@", text):
            raise ValueError("Unbound packet template")
        (output / name).write_text(text, encoding="utf-8", newline="\n")
    shutil.copyfile(regular(HERE / "trace-stop-proof.awk"), output / "trace-stop-proof.awk")
    shutil.copyfile(payload, output / "payload.so")
    (output / "owner").write_text(nonce, encoding="ascii")
    (output / "guard.conf").write_text("[Unit]\nOnFailure=\nFailureAction=none\nStartLimitAction=none\n[Service]\nRestart=no\n", encoding="ascii")
    (output / "activation.conf").write_text(f"[Service]\nExecStart=\nExecStart=/bin/sh {root}/launch.sh\n", encoding="ascii")
    (output / "stock.policy").write_text("".join(f"{k}={v}\n" for k, v in POLICY.items()), encoding="ascii")
    guarded = POLICY | {"OnFailure": "", "Restart": "no"}
    (output / "guard.policy").write_text("".join(f"{k}={v}\n" for k, v in guarded.items()), encoding="ascii")
    (output / "baseline.files").write_text("".join(f"{e['sha256']}  {e['path']}\n" for e in files), encoding="ascii")
    (output / "service.files").write_text("".join(f"{digest}  {path}\n" for path, digest in SERVICE_HASHES.values()), encoding="ascii")
    for file in output.iterdir():
        file.chmod(0o600)
    hashes = {p.name: sha(p) for p in sorted(output.iterdir())}
    (output / "packet.files").write_text("".join(f"{v}  {k}\n" for k, v in hashes.items()), encoding="ascii")
    (output / "packet.files").chmod(0o600)
    receipt = {"evidence_class": "prepared only; no native execution", "nonce": nonce, "remote_root": root,
               "absolute_budget_seconds": 360, "selection_sha256": sha(selection_file), "files": hashes,
               "packet_manifest_sha256": sha(output / "packet.files")}
    (output / "preparation.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    (output / "preparation.json").chmod(0o600)
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
