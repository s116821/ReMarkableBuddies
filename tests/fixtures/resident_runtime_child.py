"""Owned Linux protocol fixture; no tablet libraries or native operations."""
import ctypes
import hashlib
import json
import os
import pathlib
import socket
import signal
import struct
import sys
import time
import uuid
import zlib

root = pathlib.Path(os.environ["RSDK_ACTIVATION_DIRECTORY"])
mode = sys.argv[1]
assert sys.argv[2:] == ["--system", "two words", "", "$(not-a-command); *", "λ"]
assert "LD_PRELOAD" not in os.environ  # fixture-only launch, explicitly removed
if mode == "reconnect-companion-term":
    def terminate(_signal, _frame):
        companion = json.loads((root.parent / "companion-evidence").read_text())
        result = {"companion_absent": not pathlib.Path(f"/proc/{companion['pid']}").exists(),
                  "companion_term": (root.parent / "companion-evidence.term").read_text() == "TERM"}
        (root / "reader-term").write_text(json.dumps(result))
        sys.exit(0)
    signal.signal(signal.SIGTERM, terminate)
elif mode == "reconnect-companion-ignore-term":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
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
# Explicit owned source-handoff model, never native current-page authority.
declared_source = root.parent / "declared-source.json"
if declared_source.is_file():
    source_document, source_page = [uuid.UUID(value).bytes for value in json.loads(declared_source.read_text())]
else:
    source_document, source_page = uuid.uuid4().bytes, uuid.uuid4().bytes
source_order = [source_page]
declared_order = root.parent / "declared-source-order.json"
if declared_order.is_file():
    source_order = [uuid.UUID(value).bytes for value in json.loads(declared_order.read_text())]
elif mode in ("reconnect-source-handoff-indeterminate-successor", "reconnect-source-handoff-text-refusal"):
    source_order.append(uuid.uuid4().bytes)
elif mode == "reconnect-source-handoff-indeterminate-last-page":
    source_order.insert(0, uuid.uuid4().bytes)
elif mode == "reconnect-source-handoff-research-pages":
    source_order.append(uuid.UUID((root.parent / "research-successor").read_text()).bytes)
source_facts = source_document + source_page + struct.pack(">H", len(source_order)) + b"".join(source_order)
inserted_target = None

def current_source_facts():
    if mode == "reconnect-source-handoff-open-first" and (root.parent / "source-switch").is_file():
        if inserted_target is not None:
            successor_index = source_order.index(source_page) + 1
            order = source_order[:successor_index] + [inserted_target] + source_order[successor_index:]
            return source_document + source_page + struct.pack(">H", len(order)) + b"".join(order)
        return source_facts
    if inserted_target is not None:
        successor_index = source_order.index(source_page) + 1
        order = source_order[:successor_index] + [inserted_target] + source_order[successor_index:]
        return source_document + inserted_target + struct.pack(">H", len(order)) + b"".join(order)
    if mode == "reconnect-source-handoff-open-first" and (root.parent / "successor-position").is_file():
        target = source_order[source_order.index(source_page) + 1]
        return source_document + target + struct.pack(">H", len(source_order)) + b"".join(source_order)
    if mode in ("reconnect-source-handoff-text-refusal", "reconnect-source-handoff-research-pages") and (root.parent / "successor-position").is_file():
        return source_document + source_order[1] + struct.pack(">H", 2) + b"".join(source_order)
    order_change = root.parent / "ordinary-pdf-order-change"
    if mode == "reconnect-source-handoff-ordinary-pdf" and order_change.is_file():
        extra = uuid.UUID(order_change.read_text()).bytes
        return source_document + source_page + struct.pack(">H", 2) + source_page + extra
    return source_facts

while True:
    peer, _ = listener.accept()
    # The companion-startup fixture must outlive its five-second readiness
    # deadline; an idle fixture exit must not masquerade as absent-broker refusal.
    # Only the command-count probe deliberately spends up to its fixed 90s
    # request budget in pre-request admission; normal fixture timing is intact.
    peer.settimeout(95 if mode == "reconnect-source-handoff-count-probe" else
                    10 if mode.startswith("reconnect-companion-") else 3)
    credentials = peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
    assert struct.unpack("3i", credentials)[:2] == (os.getppid(), os.geteuid())
    try:
        hello = bytearray(frame())
        assert len(hello) == 203 and hello[:6] == b"RSDK\x00\x04" and hello[202] == 0
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
            if mode.startswith("reconnect-pre-enter") and request[202:204] == b"\x02\x0a":
                # Wire fixture only. No native handle, arming marker or input model.
                assert len(request) == 210
                offset, units_count = struct.unpack(">IH", request[204:])
                assert units_count <= 4096 and offset + units_count < 2147483647
                with (root / "pre-enter-requests").open("ab") as log:
                    log.write(request[186:202] + request[204:])
                if mode.endswith(("-wait", "-progress")):
                    assert peer.recv(1) == b""  # original deadline/cancellation retires client
                    break
                if mode.endswith("-refused"):
                    payload = b"\x0c\x01"
                elif mode.endswith("-unsupported"):
                    payload = b"\x03\x00"
                elif mode.endswith("-capacity"):
                    payload = b"\x03\x04"
                elif mode.endswith("-wrong-type"):
                    payload = b"\x03\x01"  # rejection not allowed for this command
                else:
                    raw = [0x0041, 0x0000, 0x2029, 0xD800, 0xDC00, 0xDFFF]
                    copied = units_count + (1 if mode.endswith("-wrong-count") else 0)
                    payload = b"\x0c\x00" + struct.pack(">H", copied)
                    payload += b"".join(struct.pack(">H", raw[i % len(raw)]) for i in range(copied))
                response = bytearray(request[:202] + b"\x03" + digest + payload)
                if mode.endswith("-wrong-digest"):
                    response[203] ^= 1
                elif mode.endswith("-wrong-operation"):
                    response[186] ^= 1
                elif mode.endswith("-wrong-sequence"):
                    response[185] ^= 1
                elif mode.endswith("-wrong-scope"):
                    response[146] ^= 1
                send(response)
                continue
            if mode in ("reconnect-source-handoff-writer-insert", "reconnect-source-handoff-open-first") and request[202:204] == b"\x02\x01":
                assert inserted_target is None and request[204:-16] == source_facts
                inserted_target = request[-16:]
                assert inserted_target != source_page
                send(request[:202] + b"\x03" + digest + b"\x01" + current_source_facts())
                continue
            if mode in ("reconnect-source-handoff-current-rejected", "reconnect-source-handoff-current-indeterminate") and request[202:204] == b"\x02\x01":
                assert request[204:-16] == source_facts and request[-16:] != source_page
                reply = b"\x03\x02" if mode.endswith("-rejected") else b"\x04"
                send(request[:202] + b"\x03" + digest + reply)
                continue
            if mode in ("reconnect-source-handoff-indeterminate", "reconnect-source-handoff-indeterminate-last-page",
                        "reconnect-source-handoff-indeterminate-successor",
                        "reconnect-source-handoff-rejected") and request[202:204] == b"\x02\x01":
                assert count == 1 and request[204:220] != source_facts[:16]
                reply = b"\x03\x00" if mode.endswith("-rejected") else b"\x04"
                send(request[:202] + b"\x03" + digest + reply)
                continue
            if mode.startswith("reconnect-source-handoff") and request[202:204] == b"\x02\x00":
                facts = current_source_facts()
                if mode == "reconnect-source-handoff-open-first" and facts[16:32] != source_page:
                    (root.parent / "observed-open-page").write_text(str(uuid.UUID(bytes=facts[16:32])))
                send(request[:202] + b"\x03" + digest + b"\x00" + facts)
                continue
            if request[202:204] in (b"\x02\x05", b"\x02\x06"):
                # Independent owned PNG encoder, not a native Reader model.
                visual = request[203] == 6
                facts = request[204:]
                assert not visual or not facts
                if mode.startswith("reconnect-source-handoff"):
                    assert facts == current_source_facts()
                (root / "capture-received").touch()
                if mode in ("reconnect-capture-cancel", "reconnect-capture-timeout") and count == 1:
                    assert peer.recv(1) == b""
                    break
                width, height = 192, 128
                ordinary_pdf = mode == "reconnect-source-handoff-ordinary-pdf"
                seed = facts[16:32] if mode in ("reconnect-source-handoff-research-pages", "reconnect-source-handoff-open-first") else b""
                pixels = (b"\xcc" * (width * height * 3) if ordinary_pdf else
                          b"".join(hashlib.sha256(seed + struct.pack(">I", i)).digest() for i in range(width * height * 3 // 32)))
                if (root.parent / "edited-selection").is_file():
                    # Represent fresh ink on the same native page and inside the left selection.
                    pixels = bytearray(pixels)
                    for y in range(30, 37):
                        for x in range(30, 37):
                            offset = (y * width + x) * 3
                            pixels[offset:offset + 3] = b"\xff\x00\xf0"
                def chunk(tag, data):
                    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
                rows = b"".join(b"\x00" + pixels[y * width * 3:(y + 1) * width * 3] for y in range(height))
                png = (b"\x89PNG\r\n\x1a\n"
                       + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                       + chunk(b"IDAT", zlib.compress(rows, 0 if ordinary_pdf else -1))
                       + chunk(b"IEND", b""))
                assert len(png) > 65536
                image_hash = hashlib.sha256(png).digest()
                if mode in ("reconnect-capture-corrupt", "reconnect-source-handoff-corrupt"):
                    png = png[:-1] + bytes([png[-1] ^ 1])
                if mode == "reconnect-capture-source" and not visual:
                    facts = bytes([facts[0] ^ 1]) + facts[1:]
                capture = facts + facts + struct.pack(">IIdB", width, height, 1.0, 2 if visual else 1) + image_hash + struct.pack(">I", len(png)) + png
                response = request[:202] + b"\x03" + digest + (b"\x08" if visual else b"\x07") + capture
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
