"""Owned loopback TLS/CONNECT controls; no production accounts or global trust changes."""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import socket
import ssl
import subprocess
import threading
import time


def atomic_json(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value), encoding='utf-8')
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, required=True)
    root = parser.parse_args().root
    if not (root / '.owned-provider-transport').is_file():
        raise RuntimeError('missing owned fixture marker')
    os.umask(0o077)
    openssl_env = os.environ.copy()
    openssl_env.pop('OPENSSL_CONF', None)

    def openssl(*args):
        # Capture output: never expose generated keys or request/auth data in logs.
        with subprocess.Popen(['openssl', *args], cwd=root, env=openssl_env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) as child:
            deadline = time.monotonic() + 5
            try:
                while child.poll() is None:
                    if (root / 'stop').exists() or time.monotonic() >= deadline:
                        raise TimeoutError('owned certificate generation stopped')
                    time.sleep(0.02)
                if child.returncode:
                    raise RuntimeError('owned certificate generation failed')
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait()

    for name in ('ca', 'unrelated'):
        openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                '-config', '/dev/null', '-subj', '/CN=owned-fixture-root',
                '-addext', 'basicConstraints=critical,CA:TRUE',
                '-addext', 'keyUsage=critical,keyCertSign,cRLSign',
                '-keyout', name + '.key', '-out', name + '.pem')
    contexts = []
    for name, ip in (('good', '127.0.0.1'), ('wrong', '127.0.0.2')):
        openssl('req', '-newkey', 'rsa:2048', '-nodes', '-config', '/dev/null',
                '-subj', '/CN=owned-fixture-leaf', '-keyout', name + '.key',
                '-out', name + '.csr')
        (root / (name + '.ext')).write_text(
            'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n'
            'extendedKeyUsage=serverAuth\nsubjectAltName=IP:' + ip + '\n', encoding='utf-8')
        openssl('x509', '-req', '-in', name + '.csr', '-CA', 'ca.pem', '-CAkey', 'ca.key',
                '-CAcreateserial', '-days', '1', '-extfile', name + '.ext', '-out', name + '.pem')
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(root / (name + '.pem'), root / (name + '.key'))
        contexts.append(context)
    (root / 'empty-roots').mkdir()
    stop = threading.Event()
    lock = threading.Lock()
    result = dict(tls_accepted=0, tls_rejected=0, requests=0, valid_requests=0,
                  origin_proxy_auth=False, connects=0, valid_proxy_auth=0,
                  plaintext_origin_auth=False, failures=0)
    workers = []

    def record(key, count=1):
        with lock:
            result[key] += count

    def flag(key):
        with lock:
            result[key] = True

    def header(stream):
        data = bytearray()
        while not data.endswith(b'\r\n\r\n'):
            if len(data) >= 16384:
                raise ValueError('header bound')
            byte = stream.recv(1)
            if not byte:
                raise ValueError('incomplete header')
            data.extend(byte)
        lines = data.decode('ascii').split('\r\n')
        fields = {}
        for line in lines[1:]:
            if line:
                name, value = line.split(':', 1)
                key = name.lower()
                if key in fields:
                    raise ValueError('duplicate header')
                fields[key] = value.strip()
        return lines[0], fields

    def origin(raw, context):
        try:
            stream = context.wrap_socket(raw, server_side=True)
        except ssl.SSLError:
            record('tls_rejected')
            return
        with stream:
            record('tls_accepted')
            first, fields = header(stream)
            length = int(fields.get('content-length', '-1'))
            if not 0 < length <= 65536:
                raise ValueError('body bound')
            body = bytearray()
            while len(body) < length:
                part = stream.recv(length - len(body))
                if not part:
                    raise ValueError('incomplete body')
                body.extend(part)
            value = json.loads(body)
            record('requests')
            if 'proxy-authorization' in fields:
                flag('origin_proxy_auth')
            valid = (first == 'POST /v1/chat/completions HTTP/1.1'
                     and fields.get('authorization') == 'Bearer owned-not-a-credential'
                     and value['model'] == 'owned-transport-fixture'
                     and value['messages'][0]['content'] == [
                         {'type': 'text', 'text': 'owned synthetic TLS request'}])
            if not valid:
                raise ValueError('unexpected synthetic request')
            record('valid_requests')
            body = b'{"choices":[{"message":{"content":"owned TLS transport answer"}}]}'
            stream.sendall(b'HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n'
                           + b'Content-Length: ' + str(len(body)).encode()
                           + b'\r\nConnection: close\r\n\r\n' + body)

    listeners = []
    for _ in range(3):
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0))
        listener.listen(8)
        listener.setblocking(False)
        listeners.append(listener)
    good_port, wrong_port, proxy_port = [s.getsockname()[1] for s in listeners]
    allowed = {f'127.0.0.1:{good_port}', f'127.0.0.1:{wrong_port}'}
    expected_proxy = 'Basic ' + base64.b64encode(b'owned-proxy:owned-fake-password').decode()

    def proxy(stream):
        first, fields = header(stream)
        parts = first.split(' ')
        if len(parts) != 3 or parts[0] != 'CONNECT' or parts[1] not in allowed:
            raise ValueError('CONNECT outside owned allowlist')
        record('connects')
        if fields.get('proxy-authorization') == expected_proxy:
            record('valid_proxy_auth')
        if 'authorization' in fields:
            flag('plaintext_origin_auth')
        target = int(parts[1].rsplit(':', 1)[1])
        with socket.create_connection(('127.0.0.1', target), timeout=3) as upstream:
            stream.sendall(b'HTTP/1.1 200 Connection established\r\n\r\n')
            deadline = time.monotonic() + 8
            while not stop.is_set() and time.monotonic() < deadline:
                ready, _, _ = select.select([stream, upstream], [], [], 0.05)
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    (upstream if source is stream else stream).sendall(data)
            raise TimeoutError('owned tunnel deadline')

    def handle(raw, index):
        with raw:
            raw.settimeout(3)
            try:
                if index < 2:
                    origin(raw, contexts[index])
                else:
                    proxy(raw)
            except Exception:
                # Fixed counters only; no headers, payloads, keys or inherited env in logs.
                record('failures')

    atomic_json(root / 'ready.json', dict(good_port=good_port, wrong_port=wrong_port,
                                         proxy_port=proxy_port))
    deadline = time.monotonic() + 25
    try:
        while not (root / 'stop').exists():
            if time.monotonic() >= deadline:
                raise TimeoutError('owned fixture lifetime')
            ready, _, _ = select.select(listeners, [], [], 0.02)
            for listener in ready:
                raw, _ = listener.accept()
                if len(workers) >= 8:
                    raw.close()
                    raise RuntimeError('owned connection bound')
                worker = threading.Thread(target=handle, args=(raw, listeners.index(listener)))
                workers.append(worker)
                worker.start()
    finally:
        for listener in listeners:
            listener.close()
        # Clients must already have completed before the parent requests stop. Give
        # in-flight EOF/close-notify handlers a bounded drain before forced stop.
        drain = time.monotonic() + 1
        for worker in workers:
            worker.join(max(0, drain - time.monotonic()))
        stop.set()
        shutdown = time.monotonic() + 4
        for worker in workers:
            worker.join(max(0, shutdown - time.monotonic()))
        if any(worker.is_alive() for worker in workers):
            record('failures')
        atomic_json(root / 'result.json', result)


if __name__ == '__main__':
    main()
