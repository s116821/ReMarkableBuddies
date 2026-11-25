# The Rust test supplies the shebang, FAULT and independently normalized UNITS.
import hashlib
import json
import os
import struct
import sys


def receive(kind):
    version, received, size = struct.unpack("<BBI", sys.stdin.buffer.read(6))
    assert (version, received) == (1, kind)
    payload = sys.stdin.buffer.read(size)
    assert len(payload) == size
    (header_size,) = struct.unpack("<I", payload[:4])
    return json.loads(payload[4 : 4 + header_size]), payload[4 + header_size :]


def send(kind, payload):
    sys.stdout.buffer.write(struct.pack("<BBI", 1, kind, len(payload)))
    sys.stdout.buffer.write(payload)
    sys.stdout.buffer.flush()


assert sys.argv[1:] == ["--internal-reader-pdf-worker-v1"]
original, pdf = receive(1)
assert len(pdf) == original["bytes"]
assert hashlib.sha256(pdf).hexdigest() == original["sha256"]
units = UNITS.encode("utf-8")
key = {
    "document": original["document"],
    "format": "pdf",
    "source_sha256": original["sha256"],
    "extractor_revision": original["extractor_revision"],
    "normalized_sha256": hashlib.sha256(units).hexdigest(),
}
ready = json.dumps(
    {"request": original["request"], "key": key, "pages": original["pages"]}
).encode("utf-8")
send(2, struct.pack("<I", len(ready)) + ready + units)

query, question = receive(3)
assert query["request"] == original["request"] and query["key"] == key
assert question == b"acceleration" and sys.stdin.buffer.read(1) == b""

if FAULT == "abrupt":
    os._exit(19)
if FAULT == "key":
    key["source_sha256"] = "0" * 64
result = json.dumps(
    {
        "request": query["request"],
        "result": {"source": key, "passages": [], "budget_limited": False},
    }
).encode("utf-8")

if FAULT == "truncated":
    sys.stdout.buffer.write(struct.pack("<BBI", 1, 4, len(result)))
    sys.stdout.buffer.write(result[:-1])
    sys.stdout.buffer.flush()
else:
    send(3 if FAULT == "frame" else 4, result)
    if FAULT == "extra":
        sys.stdout.buffer.write(b"!")
        sys.stdout.buffer.flush()
    if FAULT == "exit":
        os._exit(19)
