"""Extracted exact preflight, owned temporary Linux filesystem, no device."""
import os
from pathlib import Path
import subprocess
import tempfile

source = Path(__file__).with_name('run-qt-page-open-one-shot.ps1').read_text()
start = source.index("set -eu\nfor name in open-waiting")
end = source.index("\n'@))", start)
preflight = source[start:end]
for mode in ('success', 'unsupported', 'duplicate-overwrite', 'directory-fallback', 'existing', 'dangling'):
    with tempfile.TemporaryDirectory(prefix='open-link-test-') as directory:
        base = Path(directory)
        root = base / 'owned'
        root.mkdir(mode=0o700)
        bindir = base / 'bin'
        bindir.mkdir()
        env = dict(os.environ, PATH=str(bindir)+':'+os.environ['PATH'])
        if mode in ('existing','dangling'):
            target = root / 'open-link-target'
            if mode == 'existing': target.write_text('preserve')
            else: target.symlink_to(root / 'absent')
        if mode == 'unsupported': body = 'exit 2\n'
        elif mode == 'duplicate-overwrite': body = 'exec /bin/ln -f "$2" "$3"\n'
        elif mode == 'directory-fallback': body = 'exec /bin/ln "$2" "$3"\n'
        else: body = 'exec /bin/ln "$@"\n'
        wrapper = bindir / 'ln'
        wrapper.write_text('#!/bin/sh\n'+body)
        wrapper.chmod(0o700)
        script = base / 'preflight.sh'
        script.write_text(preflight.replace('@ROOT@', str(root)))
        result = subprocess.run(['/bin/sh', str(script)], env=env, capture_output=True, timeout=5)
        assert (result.returncode == 0) == (mode == 'success'), (mode, result.stderr)
        if mode == 'existing': assert (root/'open-link-target').read_text() == 'preserve'
        elif mode == 'dangling': assert (root/'open-link-target').is_symlink()
        elif mode != 'directory-fallback': assert not list(root.iterdir()), (mode, list(root.iterdir()))
        else:
            # Unsupported directory behavior refuses and retains the unexpected entry.
            assert (root/'open-link-directory/open-link-source').exists()
print('PASS 6 source-exact link preflight cases; no transport/device')
