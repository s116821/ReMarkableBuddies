"""Owned Linux filesystem regressions; proc/service observations are explicit mocks.

Runs exact shell strings extracted from the operator, never the operator itself.
No transport, systemd, UI, payload loading or tablet is involved.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
SOURCE = (HERE / "run-qt-page-facts-one-shot.ps1").read_text()
NONCE = "b3e3ca0475a84432a9b328218395de0c"
EXE_HASH = "071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df"
if os.geteuid() != 0:
    raise SystemExit("Owned test requires root in the isolated pinned container")

def extract(pattern):
    match = re.search(pattern, SOURCE, re.S)
    if not match:
        raise AssertionError("Exact operator shell block not found")
    return match.group(1)

LOCK = extract(r"# BEGIN private initial lock[^\n]*\n(.*?)# END private initial lock")
IDENTITY = extract(r"\$identity=ObservationSSH \(Expand @'\n(.*?)\n'@\)")
CALLBACK = extract(r"\$observed=ObservationSSH \(Expand @'\n(.*?)\n'@\)")
MOCK_PROC_SERVICE = r'''
systemctl() {
 case "$*" in *MainPID*) printf '1234\n';; *Job*) printf '\n';; *) return 90;; esac
}
awk() {
 case "$*" in */proc/1234/stat) printf '5678\n';; */proc/1234/maps) return 0;; *) /usr/bin/awk "$@";; esac
}
sha256sum() {
 case "$1" in /proc/1234/exe) printf '@EXEHASH@  /proc/1234/exe\n';; *) /usr/bin/sha256sum "$@";; esac
}
'''.replace("@EXEHASH@", EXE_HASH)

CASES = 0
def run(text, root, success, name, prefix=""):
    global CASES
    # tempfile paths contain no shell metacharacters; nonce and path are owned.
    script = prefix + "\nset -eu\n" + text.replace("@ROOT@", str(root)).replace("@NONCE@", NONCE)
    result = subprocess.run(["/bin/sh", "-c", script], capture_output=True, text=True,
                            preexec_fn=lambda: os.umask(0o022))
    if (result.returncode == 0) != success:
        raise AssertionError(f"{name}: exit={result.returncode}, stderr={result.stderr}")
    CASES += 1
    return result.stdout

def write(root, name, value, mode=0o600):
    path = root / name
    path.write_text(value)
    path.chmod(mode)

with tempfile.TemporaryDirectory(prefix="facts-shell-") as directory:
    root = Path(directory)
    root.chmod(0o700)
    lock = root / "admission.lock"
    run(LOCK, root, True, "initial lock under022")
    assert lock.stat().st_mode & 0o777 == 0o600
    # Reopening (as the unchanged launcher does) must preserve private mode.
    run('umask 077; exec 9>"@ROOT@/admission.lock"; exec 9>&-', root, True, "launcher reopen")
    assert lock.stat().st_mode & 0o777 == 0o600
    lock.chmod(0o644)
    run(LOCK, root, False, "existing nonprivate lock refuses")
    lock.unlink()
    write(root, "other", "")
    lock.symlink_to(root / "other")
    run(LOCK, root, False, "symlink lock refuses")
    lock.unlink()

    stat = root.stat()
    token = f"{NONCE} 1234 5678 {stat.st_dev} {stat.st_ino} read-facts 120000 main-dev-facts-120s\n"
    write(root, "owner", NONCE)
    write(root, "attempt.identity", "1234 5678\n")
    write(root, "facts-request", token)
    # SDK finish removes waiting BEFORE callback; deliberately no waiting exists.
    callback = dict(nonce=NONCE, stage="facts-observed-no-change-during-read",
                    application_thread=True, engine_thread=True)
    write(root, "callback.json", json.dumps(callback))
    write(root, "diagnostics.json", json.dumps({"owned": "diagnostics present"}))
    assert not (root / "facts-waiting").exists()
    observed = run(CALLBACK, root, True, "completed callback with waiting absent")
    assert json.loads(observed) == callback
    identity = run(IDENTITY, root, True, "durable request success lifecycle", MOCK_PROC_SERVICE)
    assert identity == f"1234 5678 {stat.st_dev} {stat.st_ino}\n"

    for name, bad in (
        ("nonce", token.replace(NONCE, "0" * 32)),
        ("directory inode", token.replace(f"{stat.st_dev} {stat.st_ino}", f"{stat.st_dev} {stat.st_ino + 1}")),
        ("request stage", token.replace("read-facts", "waiting-facts")),
        ("budget", token.replace("120000", "20000")),
        ("profile", token.replace("main-dev-facts-120s", "default-dev-20s")),
        ("extra field", token.rstrip("\n") + " extra\n"),
        ("multiple lines", token + "extra\n"),
        ("over cap", token + "x" * 128),
    ):
        write(root, "facts-request", bad)
        run(IDENTITY, root, False, f"request {name} refuses", MOCK_PROC_SERVICE)
    write(root, "facts-request", token, 0o644)
    run(IDENTITY, root, False, "request mode refuses", MOCK_PROC_SERVICE)
    (root / "facts-request").unlink()
    write(root, "request-target", token)
    (root / "facts-request").symlink_to(root / "request-target")
    run(IDENTITY, root, False, "request symlink refuses", MOCK_PROC_SERVICE)
    (root / "facts-request").unlink()
    run(IDENTITY, root, False, "missing durable request refuses", MOCK_PROC_SERVICE)

print(f"PASS {CASES} source-exact Linux lock/lifecycle cases; real private files, mocked proc/service only")
