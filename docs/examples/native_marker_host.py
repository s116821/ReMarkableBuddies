"""One bounded diagnostic handshake. SSH/scp only copy evidence and publish ACK.

Usage: native_marker_host.py SSH_ALIAS UNIQUE_REMOTE_NATIVE_DIR NEW_LOCAL_DIR
Requires rmscene==0.8.0 in the Python environment. No tablet input is generated.
Device 30s deadline is authoritative; this host has a 60s total startup budget.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

from native_marker_predicate import bounded, require


def main():
    require(len(sys.argv) == 4, 'expected SSH alias, native directory and new output directory')
    alias, remote, destination = sys.argv[1:]
    require(re.fullmatch(r'[a-zA-Z0-9_-]+', alias), 'unsafe SSH alias')
    require(re.fullmatch(r'/home/root/rem9-validation/[a-zA-Z0-9-]+/native', remote), 'unexpected diagnostic path')
    out = Path(destination); out.mkdir(exist_ok=False)
    started = time.monotonic(); deadline = started + 60

    def budget():
        remaining = deadline-time.monotonic()
        require(remaining > 0, 'host evidence deadline')
        return min(5, remaining)

    def command(args):
        result = subprocess.run(args, check=True, capture_output=True, timeout=budget())
        budget()
        require(len(result.stdout) <= 16_384, 'excessive control output')
        return result.stdout

    def control(name):
        return command(['ssh', alias, f'if test -f {remote}/{name}; then cat {remote}/{name}; fi'])

    def download(name):
        local = out/name
        require(not local.exists(), 'refusing to replace local evidence')
        command(['scp', '-q', f'{alias}:{remote}/{name}', str(local)])
        return local

    def paced():
        time.sleep(min(.5, budget()))

    while True:
        raw = control('expected.json')
        if raw: break
        paced()
    require(len(raw) <= 8192, 'large expected geometry')
    expected = json.loads(raw); require(expected['run'] == remote, 'wrong run')
    (out/'expected.json').write_bytes(raw)
    download('initial.rm')
    budget()
    predicate = Path(__file__).with_name('native_marker_predicate.py')
    (out/'host-source.json').write_text(json.dumps({
        p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [Path(__file__), predicate]}))
    for number in range(16):
        name = f'candidate-{number:02}'
        while True:
            raw = control(name+'.json')
            if raw: break
            paced()
        ready = json.loads(raw)
        require(ready['run'] == remote and ready['number'] == number, 'wrong candidate identity')
        (out/(name+'.json')).write_bytes(raw)
        sample = download(name+'.rm')
        require(hashlib.sha256(bounded(sample)).hexdigest() == ready['sha256'], 'candidate hash mismatch')
        # Separate process gives parser/geometry work a hard wall-clock bound.
        result = subprocess.run([sys.executable, str(predicate), str(out/'initial.rm'),
                                 str(sample), str(out/'expected.json')], capture_output=True,
                                timeout=budget(), env=os.environ.copy())
        budget()
        (out/(name+'-validation.stdout')).write_bytes(result.stdout)
        (out/(name+'-validation.stderr')).write_bytes(result.stderr)
        if result.returncode != 0:
            continue
        report = json.loads(result.stdout)
        require(report['sha256'] == ready['sha256'] and report['run'] == remote, 'validation mismatch')
        ack = out/'ack.json'; ack.write_text(json.dumps(ready))
        command(['scp', '-q', str(ack), f'{alias}:{remote}/ack.json.tmp'])
        # New per-run directory and immutable candidate identity prevent old ACK reuse.
        command(['ssh', alias, f'test ! -e {remote}/ack.json && mv {remote}/ack.json.tmp {remote}/ack.json'])
        print(json.dumps({'acknowledged': ready, 'elapsed_ms': (time.monotonic()-started)*1000}))
        return
    raise ValueError('candidate version cap exhausted without marker proof')


if __name__ == '__main__': main()
