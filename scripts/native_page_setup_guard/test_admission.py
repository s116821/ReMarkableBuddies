"""Source-exact shared admission/consumers on an owned Linux fixture only."""
import ast
import fcntl
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

HERE = Path(__file__).parent
OLD = HERE.parent / 'native_entry_probe/test_arm_open_once.py'
tree = ast.parse(OLD.read_text())
SHIM = next(ast.literal_eval(node.value) for node in tree.body
            if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'SHIM' for t in node.targets))
SHIM = SHIM.replace("if mode=='closed-afterproof' and n==2:",
                    "if mode=='closed-afterproof' and n==2:")
SHIM = SHIM.replace("print(str(int(p)+1) if mode=='wrongpid' else p)",
                    "print(str(int(p)+1) if mode=='wrongpid' or (mode=='stale-recheck' and n>=3) else p)")
NONCE = '604d9d8e17c046fe84fea4e44e608165'
HASH = '071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df'
common = ['good','missing-stage','deleted-stage','no-owner','owner-symlink','owner-mode',
          'wrong-owner','root-mode','no-lock','lock-symlink','busy-lock','no-waiting',
          'wrong-waiting','partial-waiting','waiting-mode','waiting-symlink','no-identity',
          'extra-identity','identity-mode','stale-arm','stale-temp','closed','restoring',
          'restored','callback','wrongpid','wrong-start','wrong-exe','wrong-env','job',
          'closed-afterproof','stale-recheck']
cases = 0
for consumer in ['guard-one-command.sh','arm-after-source.sh']:
    modes = common + (['command-failure','command-timeout','command-cancel','wrapper-killed'] if consumer.startswith('guard') else
                      ['close-beforepublish','ln-unsupported','ln-race','ln-race-directory'])
    for mode in modes:
        with tempfile.TemporaryDirectory(prefix='owned-setup-') as directory:
            base=Path(directory); root=base/'packet'; root.mkdir(mode=0o700)
            scripts=base/'scripts'; scripts.mkdir(); bindir=base/'bin'; bindir.mkdir()
            for name in ['systemctl','sha256sum','awk','chmod','ln']:
                path=bindir/name; path.write_text(SHIM); path.chmod(0o700)
            for name in ['admission.sh',consumer]:
                text=(HERE/name).read_text()
                if name=='admission.sh':
                    assert text.count('root=/run/rmb-qt-probe-$nonce')==1
                    text=text.replace('root=/run/rmb-qt-probe-$nonce',f'root={root}')
                (scripts/name).write_text(text)
            pid=str(os.getpid()); start=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19]
            def put(name,value,permissions=0o600):
                path=root/name; path.write_bytes(value); path.chmod(permissions)
            put('owner',NONCE.encode()); put('admission.lock',b'')
            put('attempt.identity',f'{pid} {start}\n'.encode())
            waiting=f'{NONCE} {pid} {start} waiting\n'.encode(); put('open-waiting',waiting)
            if mode=='no-owner': (root/'owner').unlink()
            if mode=='wrong-owner': put('owner',b'0'*32)
            for name,case in [('owner','owner-mode'),('open-waiting','waiting-mode'),('attempt.identity','identity-mode')]:
                if mode==case: (root/name).chmod(0o644)
            for name,case in [('owner','owner-symlink'),('open-waiting','waiting-symlink'),('admission.lock','lock-symlink')]:
                if mode==case:
                    (root/name).rename(root/(name+'.original')); (root/name).symlink_to(root/(name+'.original'))
            for name,case in [('admission.lock','no-lock'),('open-waiting','no-waiting'),('attempt.identity','no-identity')]:
                if mode==case: (root/name).unlink()
            if mode=='root-mode': root.chmod(0o755)
            if mode=='wrong-waiting': put('open-waiting',b'wrong\n')
            if mode=='partial-waiting': put('open-waiting',waiting[:-1])
            if mode=='extra-identity': put('attempt.identity',f'{pid} {start} extra\n'.encode())
            for name,case in [('open-arm','stale-arm'),('open-arm.tmp','stale-temp'),('entry.closed','closed'),
                              ('restore.claim','restoring'),('restored','restored'),('callback.json','callback')]:
                if mode==case: put(name,b'original')
            held=None
            if mode=='busy-lock':
                held=(root/'admission.lock').open('r+'); fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
            if mode in ['missing-stage','deleted-stage']: shutil.rmtree(root)
            effect=base/'effect.sh'
            effect.write_text('''#!/bin/sh
set -eu
# Opening a different descriptor must observe the held admission lock.
if flock -n "$ARM_ROOT/admission.lock" true; then exit 91; fi
printf 'one-selected-command\n' > "$EFFECT_MARKER"
case "$ARM_CASE" in command-failure) exit 17;; command-timeout|command-cancel|wrapper-killed) sleep 3;; esac
'''); effect.chmod(0o700)
            marker=base/'effect-marker'
            env=dict(os.environ,ARM_CASE=mode,ARM_ROOT=str(root),ARM_PID=pid,ARM_START=start,
                     ARM_STOCK_HASH=HASH,EFFECT_MARKER=str(marker),PATH=str(bindir)+':/usr/bin:/bin')
            args=['/bin/sh',str(scripts/consumer),NONCE]
            if consumer.startswith('guard'): args+=['1',str(effect)]
            if mode in ['command-cancel','wrapper-killed']:
                process=subprocess.Popen(args,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                deadline=time.monotonic()+2
                while not marker.exists() and time.monotonic()<deadline: time.sleep(0.01)
                assert marker.exists(), 'selected effect did not start'
                if mode=='command-cancel': process.terminate()
                else: process.kill()
                with (root/'admission.lock').open('r+') as check:
                    try: fcntl.flock(check,fcntl.LOCK_EX|fcntl.LOCK_NB)
                    except BlockingIOError: pass
                    else: raise AssertionError('lock released while bounded child outstanding')
                stdout,stderr=process.communicate(timeout=6)
                result=subprocess.CompletedProcess(args,process.returncode,stdout,stderr)
            else: result=subprocess.run(args,env=env,capture_output=True,timeout=6)
            assert (result.returncode==0)==(mode=='good'), (consumer,mode,result.stderr)
            if consumer.startswith('guard'):
                assert marker.exists()==(mode in ['good','command-failure','command-timeout','command-cancel','wrapper-killed']), mode
                assert not (root/'open-arm').exists() or mode=='stale-arm'
            elif mode=='good':
                assert (root/'open-arm').read_bytes()==f'{NONCE} {pid} {start} open\n'.encode()
            elif mode in ['stale-arm','ln-race']: assert (root/'open-arm').read_bytes()==b'original'
            elif mode=='ln-race-directory': assert (root/'open-arm').is_dir() and not list((root/'open-arm').iterdir())
            else: assert not (root/'open-arm').exists(), mode
            if mode in ['missing-stage','deleted-stage']: assert not root.exists()
            if held: held.close()
            if root.exists() and (root/'admission.lock').exists():
                with (root/'admission.lock').open('r+') as check: fcntl.flock(check,fcntl.LOCK_EX|fcntl.LOCK_NB)
            cases+=1
print(f'PASS {cases} shared admission/consumer cases; no transport/device')
