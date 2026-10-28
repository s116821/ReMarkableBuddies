"""Owned Linux protocol fixture; no tablet libraries or native operations."""
import ctypes
import hashlib
import os
import pathlib
import socket
import struct
import sys
import time
import uuid
import zlib

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
listener.bind(str(root / "sdk.pending"))
(root / "sdk.pending").chmod(0o600)
if mode == "delayed-listen":
    # The parent proves it is servicing admission while only staging exists.
    until = time.monotonic() + 3
    while not (root / "permit-listen").exists():
        assert time.monotonic() < until
        time.sleep(0.005)
listener.listen(1)
# Linux fixture: preserve the client's nlink==1 invariant at publication too.
# Missing renameat2 support is a fixture failure, never an overwrite fallback.
libc = ctypes.CDLL(None, use_errno=True)
rename = libc.renameat2
rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
rename.restype = ctypes.c_int
directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
try:
    if rename(directory, b"sdk.pending", directory, b"sdk.sock", 1) != 0:
        raise OSError(ctypes.get_errno(), "atomic fixture socket publication failed")
finally:
    os.close(directory)

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

connections = set()
handshakes = set()
count = 0
while True:
    peer, _ = listener.accept()
    peer.settimeout(3)
    credentials = peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
    assert struct.unpack("3i", credentials)[:2] == (os.getppid(), os.geteuid())
    try:
        hello = bytearray(frame())
        assert len(hello) == 203 and hello[:6] == b"RSDK\x00\x03" and hello[202] == 0
        assert hello[6:118] == admission[8:120]
        assert struct.unpack(">I", hello[118:122])[0] == os.getpid()
        assert hello[130:146] == admission[120:136]
        connection = bytes(hello[146:162])
        handshake = bytes(hello[186:202])
        assert connection not in connections and handshake not in handshakes
        connections.add(connection)
        handshakes.add(handshake)
        count += 1
        (root / "handshakes").write_text(str(count))
        if mode == "late-ready":
            time.sleep(10)
        hello[162:178] = uuid.uuid4().bytes
        hello[202] = 1
        if mode == "bad-ready":
            hello[130:146] = uuid.uuid4().bytes
        send(hello)
        if mode == "reconnect-idle" and count == 1:
            # Immediate owned EOF models expiry without a 30-second test sleep.
            peer.close()
            (root / "idle-retired").touch()
            continue
        while True:
            request = frame()
            if mode == "reconnect-idle":
                with (root / "commands").open("ab") as commands:
                    commands.write(request[202:204])
            digest = hashlib.sha256(struct.pack(">I", len(request)) + request).digest()
            if request[202:204] == b"\x02\x05":
                # Independent owned PNG encoder, not a native Reader model.
                facts = request[204:]
                (root / "capture-received").touch()
                if mode in ("reconnect-capture-cancel", "reconnect-capture-timeout") and count == 1:
                    assert peer.recv(1) == b""
                    break
                width, height = 192, 128
                pixels = b"".join(hashlib.sha256(struct.pack(">I", i)).digest() for i in range(width * height * 3 // 32))
                def chunk(tag, data):
                    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
                rows = b"".join(b"\x00" + pixels[y * width * 3:(y + 1) * width * 3] for y in range(height))
                png = (b"\x89PNG\r\n\x1a\n"
                       + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                       + chunk(b"IDAT", zlib.compress(rows))
                       + chunk(b"IEND", b""))
                assert len(png) > 65536
                image_hash = hashlib.sha256(png).digest()
                if mode == "reconnect-capture-corrupt":
                    png = png[:-1] + bytes([png[-1] ^ 1])
                if mode == "reconnect-capture-source":
                    facts = bytes([facts[0] ^ 1]) + facts[1:]
                capture = facts + facts + struct.pack(">IIdB", width, height, 1.0, 1) + image_hash + struct.pack(">I", len(png)) + png
                response = request[:202] + b"\x03" + digest + b"\x07" + capture
                if mode == "reconnect-capture-eof":
                    peer.sendall(struct.pack(">I", len(response)) + response[:500])
                    break
                send(response)
                continue
            if mode in ("reconnect-pending", "reconnect-observe-timeout") and count == 1:
                assert request[202:204] in (b"\x02\x01", b"\x02\x03") if mode == "reconnect-pending" else request[202:204] == b"\x02\x00"
                (root / "effect-received").touch()
                assert peer.recv(1) == b""  # no reply before caller cancellation
                break
            if mode in ("selection", "reconnect-selection") and request[202:204] == b"\x02\x03":
                # Owned current-target selection fixture: unchanged original facts.
                assert request[220:236] == request[-16:]
                send(request[:202] + b"\x03" + digest + b"\x05" + request[204:-16])
            else:
                assert (len(request) == 204 and request[202:] == b"\x02\x00") or request[202:204] in (b"\x02\x02", b"\x02\x04")
                response = bytearray(request[:202] + b"\x03" + digest + b"\x03\x00")
                if mode == "reconnect-malformed":
                    response[186:202] = uuid.uuid4().bytes
                send(response)
    except (EOFError, ConnectionResetError, BrokenPipeError):
        pass
    finally:
        peer.close()
    if not mode.startswith("reconnect"):
        break
