"""Unexecuted offline host-harness candidate; compiled binaries supplied explicitly.
No protected credentials or native loop. Fixture provider is loopback port9 only.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--old-driver', required=True)
    parser.add_argument('--current-binary', required=True)
    parser.add_argument('--old-driver-sha256', required=True)
    parser.add_argument('--current-binary-sha256', required=True)
    parser.add_argument('--build-receipt', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    root = output / 'owned-store'
    conversation = str(uuid.uuid4())
    report = {'status': 'started', 'conversation': conversation, 'calls': [],
              'old_source': '68e42968b4c58b6f8ed1102078d78b40fad33ffc',
              'live_provider_requests': 0, 'tablet_operations': 0,
              'limits': 'Host-compiled old library; idempotent existing-object controls; real SyncEngine/Worker with pure-memory outbound transport; no inbound sync/new-object/native qualification'}
    receipt_raw = Path(args.build_receipt).read_bytes()
    receipt = json.loads(receipt_raw)
    assert receipt['old_source'] == report['old_source']
    assert receipt['old_driver_source_sha256'] == 'e0e1e547740f45cd2cebadbd6d1bcba98426d43633f00212961f7d8e56fb5297'
    for name, path, expected in [('old_driver', args.old_driver, args.old_driver_sha256),
                                 ('current_binary', args.current_binary, args.current_binary_sha256)]:
        actual = hashlib.sha256(Path(path).read_bytes()).hexdigest()
        assert actual == expected == receipt[name + '_sha256'], 'compiled executable identity mismatch'
        report[name + '_sha256'] = actual
    report['build_receipt_sha256'] = hashlib.sha256(receipt_raw).hexdigest()
    report['build_receipt'] = receipt
    private = output / 'private-home'
    private.mkdir()
    # No inherited credentials/proxies/home/config paths. These public Windows
    # loader variables are the only inherited values; hosted execution is Linux.
    environment = {key: os.environ[key] for key in ['SystemRoot', 'WINDIR', 'COMSPEC'] if key in os.environ}
    environment.update(PATH=os.defpath, HOME=str(private), USERPROFILE=str(private),
                       TMPDIR=str(private), TEMP=str(private), TMP=str(private),
                       XDG_CONFIG_HOME=str(private), XDG_CACHE_HOME=str(private), NO_PROXY='*', no_proxy='*')
    environment.update(OPENAI_API_KEY='offline-fixture-key', OPENAI_BASE_URL='http://127.0.0.1:9',
                       OPENAI_MODEL='offline-fixture-model', MEM_API_KEY='')
    config = output / 'config.json'
    config.write_text(json.dumps({'paths': {k: str(root / k) for k in ['data', 'cache', 'credentials']},
                                 'base_url': 'http://127.0.0.1:9', 'model': 'offline-fixture-model'}))
    environment['REMARKABLE_BUDDIES_CONFIG'] = str(config)
    source = output / 'source.json'
    source.write_text(json.dumps({'document': str(uuid.uuid4()), 'format': 'notebook', 'source_sha256': 'a' * 64,
                                 'extractor_revision': 'offline-old-library-fixture-v1',
                                 'units': [{'page': 1, 'section': None, 'text': 'x = 1.'}]}))
    request = {'command': 'generate-notebook', 'conversation': conversation, 'operation': str(uuid.uuid4()),
               'request': 'Keep x = 1.', 'document_file': str(source),
               'document_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
               'selection': {'scope': 'supplied-notebook'}, 'guidance': None,
               'max_model_calls': 2, 'history_range': None}

    def snapshot():
        result = {}
        for area in ['data', 'cache', 'credentials']:
            for file in (root / area).rglob('*'):
                assert not file.is_symlink()
                if file.is_file():
                    result[str(file.relative_to(root))] = hashlib.sha256(file.read_bytes()).hexdigest()
        return result

    def run(label, argv, payload=None, decode=True):
        record = {'label': label, 'argv': argv, 'status': 'attempted'}
        report['calls'].append(record)
        try:
            result = subprocess.run(argv, input=payload, env=environment, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, timeout=120, cwd=private)
        except subprocess.TimeoutExpired as error:
            (output / (label + '.stdout')).write_bytes(error.stdout or b'')
            (output / (label + '.stderr')).write_bytes(error.stderr or b'')
            record['status'] = 'timeout'
            raise
        (output / (label + '.stdout')).write_bytes(result.stdout)
        (output / (label + '.stderr')).write_bytes(result.stderr)
        record.update(status='completed', exit=result.returncode)
        assert result.returncode == 0
        return json.loads(result.stdout) if decode else result.stdout.decode()

    child = None
    reader = None
    reader_errors = []
    lines = queue.Queue()
    child_log = (output / 'old-hold.stderr').open('wb')
    try:
        help_text = run("current-command-help", [args.current_binary, "--help"], decode=False)
        assert "--writer-note" in help_text and "--simulate" in help_text, "normal application command required, not test harness"
        report["current_help_sha256"] = hashlib.sha256(help_text.encode()).hexdigest()
        report["current_version"] = run("current-command-version", [args.current_binary, "--version"], decode=False).strip()
        assert report["current_version"]
        child = subprocess.Popen([args.old_driver, 'positive-controls-and-hold', str(root), conversation],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=child_log, env=environment, cwd=private)
        def preserve_stdout():
            try:
                pending = b''
                with (output / 'old-hold.stdout').open('wb') as stream:
                    while True:
                        chunk = child.stdout.read1(4096)
                        if not chunk:
                            break
                        stream.write(chunk)
                        stream.flush()
                        pending += chunk
                        while b'\n' in pending:
                            line, pending = pending.split(b'\n', 1)
                            lines.put(line + b'\n')
            except BaseException as error:
                reader_errors.append(type(error).__name__)
        reader = threading.Thread(target=preserve_stdout, daemon=True)
        reader.start()
        line = lines.get(timeout=60)
        (output / 'old-ready.stdout').write_bytes(line)
        ready = json.loads(line)
        control = ready['sync_positive_control']
        assert ready['ready'] and child.poll() is None
        assert control['worker_authorize_entered'] and control['uploads'] > 0 and control['manifest_objects'] > 0
        assert 1 <= control['engine_steps'] <= 8 and control['transport_calls'] > 0
        report['old_positive_controls'] = ready
        before = snapshot()
        refused = run('current-while-old-held', [args.current_binary, '--writer-note'], json.dumps(request).encode())
        assert refused['status'] == 'refused' and refused['failure'] == 'storage' and refused['local_invocations'] == 0
        assert snapshot() == before
        child.stdin.write(b'release\n')
        child.stdin.flush()
        child.wait(timeout=30)
        reader.join(timeout=10)
        assert not reader.is_alive() and not reader_errors
        stdout = lines.get(timeout=1)
        (output / 'old-release.stdout').write_bytes(stdout)
        assert child.returncode == 0 and json.loads(stdout)['closed']
        request['operation'] = str(uuid.uuid4())
        adopted = run('current-after-release', [args.current_binary, '--writer-note'], json.dumps(request).encode())
        assert adopted['status'] == 'failed' and adopted['failure'] == 'provider' and adopted['local_invocations'] == 1
        after = snapshot()
        for relative, digest in before.items():
            normalized = relative.replace('\\', '/')
            if '/objects/' in normalized or '/commits/' in normalized or normalized in ['data/CURRENT', 'data/identity.json']:
                assert after[relative] == digest, 'adoption changed original identity/history'
        refused = run('old-format4-reopen', [args.old_driver, 'refuse-reopen', str(root), conversation])
        assert refused['refused'] and refused['observed_format'] == 4 and snapshot() == after
        report.update(status='passed', before=before, after=after)
    except BaseException as error:
        report.update(status='failed', failure_type=type(error).__name__)
        raise
    finally:
        try:
            if child is not None and child.poll() is None:
                child.kill()  # Only the exact child created here; preserve all fixture files.
                child.wait(timeout=10)
            if reader is not None:
                reader.join(timeout=10)
                report['stdout_owner_complete'] = not reader.is_alive()
                if reader.is_alive() or reader_errors:
                    report['cleanup_unverified'] = True
                    report['stdout_reader_errors'] = reader_errors
                    raise RuntimeError('stdout preservation incomplete; owned fixture retained')
            report['owned_child_exit'] = None if child is None else child.returncode
        except BaseException as error:
            report['cleanup_unverified'] = True
            report['cleanup_failure_type'] = type(error).__name__
            raise
        finally:
            child_log.close()
            (output / 'report.json').write_text(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
