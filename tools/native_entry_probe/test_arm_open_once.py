"""Owned Linux source-exact arming mocks. Never invokes SSH or a tablet."""
import os
from pathlib import Path
import subprocess
import tempfile

SOURCE = Path(__file__).with_name("arm-open-once.sh").read_text(encoding="utf-8")
NONCE = "604d9d8e17c046fe84fea4e44e608165"
STOCK_HASH = "071d85beef3ef2d4cc0e11002140b27b82a2cc04a2ed740a5669f591069b77df"
SHIM = r'''#!/usr/bin/python3
import os, pathlib, subprocess, sys
name=pathlib.Path(sys.argv[0]).name; mode=os.environ['ARM_CASE']; root=pathlib.Path(os.environ['ARM_ROOT']); p=os.environ['ARM_PID']; started=os.environ['ARM_START']
if name=='systemctl':
    if '--property=MainPID' in sys.argv:
        counter=root/'.mainpid-reads'; n=int(counter.read_text())+1 if counter.exists() else 1; counter.write_text(str(n))
        if mode=='closed-afterproof' and n==2: (root/'entry.closed').write_text('closed')
        print(str(int(p)+1) if mode=='wrongpid' else p)
    elif '--property=Job' in sys.argv: print('42' if mode=='job' else '')
    else: sys.exit(90)
elif name=='sha256sum': print(('bad' if mode=='wrong-exe' else os.environ['ARM_STOCK_HASH'])+'  '+sys.argv[1])
elif name=='awk':
    if sys.argv[1]=='-v': sys.exit(1 if mode=='wrong-env' else 0)
    if sys.argv[1]=='{print $22}': print(str(int(started)+1) if mode=='wrong-start' else started)
    elif sys.argv[1]=='{print $1}': print(sys.stdin.read().split()[0])
    else: sys.exit(90)
elif name=='chmod':
    os.chmod(sys.argv[2], int(sys.argv[1],8))
    if mode=='close-beforepublish': (root/'entry.closed').write_text('closed')
elif name=='ln':
    if mode=='ln-unsupported': sys.exit(90)
    if mode=='ln-race': pathlib.Path(sys.argv[-1]).write_bytes(b'original')
    if mode=='ln-race-directory': pathlib.Path(sys.argv[-1]).mkdir()
    sys.exit(subprocess.call(['/usr/bin/ln']+sys.argv[1:]))
else: sys.exit(90)
'''

cases = ["good", "no-waiting", "wrong-waiting", "partial-waiting", "waiting-mode",
         "waiting-symlink", "root-mode", "no-identity", "extra-identity", "identity-mode",
         "stale-arm", "stale-temp", "closed", "restoring", "wrongpid", "wrong-start",
         "wrong-exe", "wrong-env", "job", "closed-afterproof", "close-beforepublish",
         "ln-unsupported", "ln-race", "ln-race-directory"]
for mode in cases:
    with tempfile.TemporaryDirectory(prefix="owned-open-arm-") as temporary:
        workspace=Path(temporary); root=workspace/"packet"; root.mkdir(mode=0o700)
        shims=workspace/"bin"; shims.mkdir()
        for name in ["systemctl", "sha256sum", "awk", "chmod", "ln"]:
            script=shims/name; script.write_text(SHIM, encoding="utf-8"); script.chmod(0o700)
        p=str(os.getpid()); start=Path(f"/proc/{p}/stat").read_text().rsplit(")",1)[1].split()[19]
        def put(name, value, permissions=0o600):
            path=root/name; path.write_bytes(value); path.chmod(permissions)
        put("owner", NONCE.encode()); put("attempt.identity", f"{p} {start}\n".encode())
        waiting=f"{NONCE} {p} {start} waiting\n".encode()
        put("open-waiting", waiting)
        if mode=="no-waiting": (root/"open-waiting").unlink()
        if mode=="wrong-waiting": put("open-waiting", b"wrong\n")
        if mode=="partial-waiting": put("open-waiting", waiting[:-1])
        if mode=="waiting-mode": (root/"open-waiting").chmod(0o644)
        if mode=="waiting-symlink":
            (root/"open-waiting").unlink(); put("other",waiting); (root/"open-waiting").symlink_to(root/"other")
        if mode=="root-mode": root.chmod(0o755)
        if mode=="no-identity": (root/"attempt.identity").unlink()
        if mode=="extra-identity": put("attempt.identity",f"{p} {start} extra\n".encode())
        if mode=="identity-mode": (root/"attempt.identity").chmod(0o644)
        if mode=="stale-arm": put("open-arm",b"original")
        if mode=="stale-temp": put("open-arm.tmp",b"original")
        if mode=="closed": put("entry.closed",b"closed")
        if mode=="restoring": put("restore.claim",b"closed")
        operator=workspace/"arming.sh"
        original=f"root=/run/rmb-qt-probe-{NONCE}"
        assert SOURCE.count(original)==1
        operator.write_text(SOURCE.replace(original,f"root={root}"),encoding="utf-8")
        env=os.environ.copy(); env.update(ARM_CASE=mode, ARM_ROOT=str(root), ARM_PID=p,
            ARM_START=start, ARM_STOCK_HASH=STOCK_HASH, PATH=str(shims)+":/usr/bin:/bin")
        result=subprocess.run(["/bin/sh",str(operator)],env=env,capture_output=True,text=True,timeout=5)
        arm=root/"open-arm"
        if mode=="good":
            assert result.returncode==0, result.stderr
            assert arm.read_bytes()==f"{NONCE} {p} {start} open\n".encode()
            assert not (root/"open-arm.tmp").exists()
            old=arm.stat(); duplicate=subprocess.run(["/bin/sh",str(operator)],env=env,capture_output=True,timeout=5)
            assert duplicate.returncode!=0 and arm.stat().st_ino==old.st_ino
        else:
            assert result.returncode!=0, mode
            if mode in ("stale-arm","ln-race"): assert arm.read_bytes()==b"original"
            elif mode=="ln-race-directory": assert arm.is_dir() and not list(arm.iterdir())
            else: assert not arm.exists(), mode
        print(f"{mode}: PASS")
print(f"PASS {len(cases)} arming mock cases plus duplicate rejection; no transport/device")
