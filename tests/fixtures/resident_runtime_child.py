"""Owned Linux protocol fixture; no tablet libraries or native operations."""
import hashlib
import os
import pathlib
import socket
import struct
import sys
import time
import uuid

root = pathlib.Path(os.environ["RSDK_ACTIVATION_DIRECTORY"])
mode = sys.argv[1]
assert sys.argv[2:] == ["--system", "two words", "", "$(not-a-command); *", "λ"]
assert "LD_PRELOAD" not in os.environ  # fixture-only launch, explicitly removed
assert not any(key in os.environ for key in ("LD_AUDIT", "LD_LIBRARY_PATH", "QT_PLUGIN_PATH", "QML_IMPORT_PATH", "HOSTNAME"))
admission = (root / "admission.bin").read_bytes()
assert len(admission) == 168 and admission[:8] == b"RSDKACT1"
assert admission[104:120] == uuid.UUID(pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip()).bytes
assert struct.unpack(">II", admission[136:144]) == (os.geteuid(), os.getppid())
parent_start = int(pathlib.Path(f"/proc/{os.getppid()}/stat").read_text().rsplit(")", 1)[1].split()[19])
assert struct.unpack(">Q", admission[144:152])[0] == parent_start
assert time.clock_gettime(time.CLOCK_BOOTTIME) * 1000 < struct.unpack(">Q", admission[152:160])[0]
if mode == "exit":
    sys.exit(119)
if mode == "no-socket":
    time.sleep(10)
if mode == "replaced-admission":
    (root / "admission.bin").rename(root / "original-admission.bin")
    (root / "admission.bin").write_bytes(admission)
    (root / "admission.bin").chmod(0o600)
if mode != "no-consume":
    os.link(root / "admission.bin", root / "admission.consumed")
listener = socket.socket(socket.AF_UNIX)
listener.bind(str(root / "sdk.sock"))
(root / "sdk.sock").chmod(0o600)
listener.listen(1)
peer, _ = listener.accept()
peer.settimeout(3)
credentials = peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
assert struct.unpack("3i", credentials)[:2] == (os.getppid(), os.geteuid())

def receive(count):
    result = b""
    while len(result) < count:
        block = peer.recv(count - len(result))
        if not block:
            raise EOFError()
        result += block
    return result

def frame():
    size = struct.unpack(">I", receive(4))[0]
    assert size <= 65536
    return receive(size)

def send(payload):
    peer.sendall(struct.pack(">I", len(payload)) + payload)

hello = bytearray(frame())
assert len(hello) == 203 and hello[:6] == b"RSDK\x00\x02" and hello[202] == 0
assert hello[6:118] == admission[8:120]
assert struct.unpack(">I", hello[118:122])[0] == os.getpid()
assert hello[130:146] == admission[120:136]
(root / "handshakes").write_bytes(b"1")
if mode == "late-ready":
    time.sleep(10)
hello[162:178] = uuid.uuid4().bytes
hello[202] = 1
if mode == "bad-ready":
    hello[130:146] = uuid.uuid4().bytes
send(hello)
try:
    while True:
        request = frame()
        digest = hashlib.sha256(struct.pack(">I", len(request)) + request).digest()
        if mode == "selection":
            assert request[202:204] == b"\x02\x03"
            # Owned current-target selection fixture: unchanged original facts.
            assert request[220:236] == request[-16:]
            send(request[:202] + b"\x03" + digest + b"\x05" + request[204:-16])
        else:
            assert len(request) == 204 and request[202:] == b"\x02\x00"
            send(request[:202] + b"\x03" + digest + b"\x03\x00")
except EOFError:
    pass
