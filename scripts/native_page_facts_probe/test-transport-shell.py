"""Execute the actual captured transport argument locally; no SSH/device calls."""
import json
import subprocess
import sys
from pathlib import Path

fixture = json.loads(Path(sys.argv[1]).read_text())
assert "\r\n" in fixture["raw"]
raw = subprocess.run(["/bin/sh", "-c", fixture["raw"]], capture_output=True)
assert raw.returncode != 0 and not raw.stdout, raw
for key in ("ssh", "observation"):
    command = fixture[key]
    assert "\r" not in command and command == fixture["raw"].replace("\r\n", "\n")
    result = subprocess.run(["/bin/sh", "-c", command], capture_output=True)
    assert result.returncode == 0 and result.stdout == b"owned $value\n", result
print("PASS 3 actual captured shell arguments: raw CRLF refuses; both normalized transports execute owned literal")
