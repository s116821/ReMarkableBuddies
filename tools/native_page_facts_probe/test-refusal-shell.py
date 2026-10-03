"""Exact future read/cleanup shell on owned Linux files; no device or service calls."""
import os
from pathlib import Path
import re
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
MODULE = (HERE / "facts-refusal-proof.ps1").read_text()
FUTURE = (HERE / "run-qt-page-facts-diagnostic-source.ps1").read_text()
READ = re.search(r"function Get-FactsRefusalReadCommand.*?@'\n(.*?)\n'@", MODULE, re.S).group(1)
CLEANUP = re.search(r"^for name in payload\.so .*\nrmdir '@ROOT@'\ntest ! -e '@ROOT@'", FUTURE, re.M).group(0)
NAMES = CLEANUP.split("; do", 1)[0].removeprefix("for name in ").split()
assert "refusal.json" in NAMES
if os.geteuid() != 0:
    raise SystemExit("Run in the isolated pinned container as root")
count = 0

def run(command, root, success, label, prefix=""):
    global count
    result = subprocess.run(["/bin/sh", "-c", prefix + "\nset -eu\n" + command.replace("@ROOT@", str(root))],
                            text=True, capture_output=True)
    if (result.returncode == 0) != success:
        raise AssertionError(f"{label}: {result.returncode} {result.stderr}")
    count += 1
    return result.stdout

with tempfile.TemporaryDirectory(prefix="refusal-shell-") as temporary:
    parent = Path(temporary)
    root = parent / "read-stage"
    root.mkdir(mode=0o700)
    file = root / "refusal.json"
    assert run(READ, root, True, "absent") == "absent\n"
    for text, success, label in (("{}", True, "bounded file"), ("{partial", True, "malformed preserved"),
                                 ("x" * 2048, True, "exact byte cap"), ("\0" * 2048, True, "binary byte cap"), ("x" * 2049, False, "over cap"),
                                 ("", False, "empty file")):
        file.write_text(text)
        file.chmod(0o600)
        output = run(READ, root, success, label)
        if success:
            assert output == "present\n" + text
    file.write_text("{}")
    file.chmod(0o644)
    run(READ, root, False, "nonprivate mode")
    file.unlink()
    target = root / "target"
    target.write_text("{}")
    target.chmod(0o600)
    file.symlink_to(target)
    run(READ, root, False, "symlink")
    file.unlink()
    os.mkfifo(file, 0o600)
    run(READ, root, False, "fifo refuses before read")
    file.unlink()
    file.write_text("{}")
    file.chmod(0o600)
    # Owned mutation immediately AFTER the second (last) size sample. Execute
    # the exact read command and prove actual output stays at sentinel cap.
    growth = f'''
wc() {{
 /usr/bin/wc "$@"
 if test -e '{root}/size-sampled'; then printf '%4096s' x > '{file}';
 else : > '{root}/size-sampled'; fi
}}
'''
    output = run(READ, root, True, "post-size growth reader bounded", growth)
    assert output.startswith("present\n") and len(output.encode()) == 8 + 2049
    file.unlink()

    for extra in (False, True):
        stage = parent / ("cleanup-extra" if extra else "cleanup-exact")
        stage.mkdir(mode=0o700)
        for name in NAMES:
            (stage / name).write_text("owned fixture")
        if extra:
            (stage / "preserve-extra").write_text("unknown owned fixture")
        run(CLEANUP, stage, not extra, "unknown extra preserves stage" if extra else "exact cleanup including refusal")
        if extra:
            assert stage.is_dir() and (stage / "preserve-extra").read_text() == "unknown owned fixture"
        else:
            assert not stage.exists()

print(f"PASS {count} source-exact refusal presence/cap/type/cleanup cases; real owned files, no services")
