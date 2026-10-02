"""Fake E0 children, never a real tablet/service/payload controller.

Only a runner-created temporary directory bearing its unpredictable ownership token
is accepted. The guard owns its children; recovery does not need Supervisor replies.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def start_identity(pid):
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.restype = wintypes.HANDLE
        handle = kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            return None
        try:
            values = [wintypes.FILETIME() for _ in range(4)]
            if not kernel.GetProcessTimes(wintypes.HANDLE(handle), *map(ctypes.byref, values)):
                return None
            return (values[0].dwHighDateTime << 32) | values[0].dwLowDateTime
        finally:
            kernel.CloseHandle(wintypes.HANDLE(handle))
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        return int(raw[raw.rfind(")") + 2:].split()[19])
    except (FileNotFoundError, ProcessLookupError):
        return None


def atomic(path, data):
    temporary = path.with_suffix(path.suffix + ".partial")
    temporary.write_bytes(data)
    os.replace(temporary, path)


def emit(value):
    print(json.dumps(value), flush=True)


def owned(root, token):
    root = Path(root).resolve(strict=True)
    if root.name != "buddy-e0-" + token or (root / "owner").read_text() != token:
        raise ValueError("not an owned harness directory")
    return root


def child(root, token, role):
    p = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), role,
                          str(root), token], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    hello = json.loads(p.stdout.readline())
    if hello != {"role": role, "pid": p.pid, "start": token + ":" + role}:
        p.kill()
        p.wait()
        raise RuntimeError("owned child start challenge failed")
    hello["os_start"] = start_identity(p.pid)
    if hello["os_start"] is None:
        stop(p)
        raise RuntimeError("missing OS process start identity")
    return p, hello


def stop(p):
    if p and p.poll() is None:
        p.kill()
    if p:
        p.wait(timeout=2)


def supervisor(root, token):
    emit({"role": "supervisor", "pid": os.getpid(), "start": token + ":supervisor"})
    for line in sys.stdin:
        command = json.loads(line)
        if command["op"] == "apply":
            atomic(root / "session", json.dumps(command["scope"]).encode())
            atomic(root / "config", b"injected\n")
        elif command["op"] == "partial":
            (root / "config.partial").write_bytes(b"incom")
        emit({"ack": command["op"]})
    while not (root / "stop-owned-children").exists():
        time.sleep(0.01)


def runtime(root, token):
    emit({"role": "runtime", "pid": os.getpid(), "start": token + ":runtime"})
    # This fake process has no device calls. It responds to explicit stdin events.
    for line in sys.stdin:
        command = json.loads(line)
        if command["op"] == "exit":
            return
        emit({"event": command["op"], "scope": command["scope"]})
    while not (root / "stop-owned-children").exists():
        time.sleep(0.01)


def guard(root, token):
    emit({"role": "guard", "pid": os.getpid(), "start": token + ":guard"})
    baseline = (root / "stock-baseline").read_bytes()
    # Simulated reboot reconstructs runtime-only state, with payload still inert.
    atomic(root / "config", baseline)
    for name in ("session", "config.partial", "session.partial", "stop-owned-children"):
        (root / name).unlink(missing_ok=True)
    unrelated = digest(root / "user-config")
    supervisor_process, supervisor_identity = child(root, token, "supervisor")
    stock, stock_identity = child(root, token + "-stock", "runtime")
    injected = None
    identities = [supervisor_identity, stock_identity]
    commands = queue.Queue()
    def receive():
        for line in sys.stdin:
            commands.put(json.loads(line))
        commands.put({"op": "quit"})
    threading.Thread(target=receive, daemon=True).start()
    state = "Stock"
    scope = None
    attempts = 0
    deadline = 0
    heartbeat = 0
    armed = False
    fail_rollback = False
    def report(event, **extra):
        emit({"event": event, "state": state, "attempts": attempts,
              "config_hash": digest(root / "config"),
              "baseline_hash": hashlib.sha256(baseline).hexdigest(),
              "unrelated_preserved": digest(root / "user-config") == unrelated,
              "supervisor_alive": supervisor_process.poll() is None,
              "injected_alive": injected is not None and injected.poll() is None,
              "stock_alive": stock.poll() is None, "identities": identities,
              "owned_remaining": [x for x in ("session", "config.partial", "session.partial")
                                  if (root / x).exists()], **extra})
    def restore(reason):
        nonlocal state, stock, armed
        state = "RestoringStock"
        if fail_rollback:
            state = "RecoveryFailed"
            report("recovery-failed", reason=reason)
            armed = False
            return
        stop(injected)
        atomic(root / "config", baseline)
        for name in ("session", "config.partial", "session.partial"):
            (root / name).unlink(missing_ok=True)
        if stock.poll() is not None:
            stock, identity = child(root, token + "-restored", "runtime")
            identities.append(identity)
        state = "DisabledForSession"
        armed = False
        report("restored", reason=reason)
    report("boot")
    try:
        while True:
            now = time.monotonic()
            if armed and supervisor_process.poll() is not None:
                restore("supervisor-dead")
            elif state == "Activating" and injected and injected.poll() is not None:
                restore("constructor-exit")
            elif state == "Activating" and now >= deadline:
                restore("readiness-deadline")
            elif state == "Ready" and (injected.poll() is not None or now - heartbeat >= 3):
                restore("runtime-or-heartbeat-lost")
            try:
                command = commands.get(timeout=0.01)
            except queue.Empty:
                continue
            op = command["op"]
            if op == "quit":
                return
            if op == "kill-supervisor":
                stop(supervisor_process)
                if not armed:
                    state = "DisabledForSession"
                    report("restored", reason="no-activation-before-guard")
                continue
            if op == "inspect":
                report("inspect")
            elif op == "preflight" and state == "Stock":
                state = "Preflight" if command.get("compatible", True) else "DisabledForSession"
                report("preflight")
            elif op == "arm" and state == "Preflight":
                armed = True
                state = "RecoveryArmed"
                report("armed")
            elif op == "partial" and state == "RecoveryArmed":
                supervisor_process.stdin.write(json.dumps({"op": "partial"}) + "\n")
                supervisor_process.stdin.flush()
                if not supervisor_process.stdout.readline():
                    restore("supervisor-died-during-partial")
                else:
                    report("partial-applied")
            elif op == "activate" and state == "RecoveryArmed" and attempts == 0:
                attempts += 1
                scope = {"boot": token, "generation": attempts,
                         "nonce": token + "-challenge", "process": token + ":injected"}
                supervisor_process.stdin.write(json.dumps({"op": "apply", "scope": scope}) + "\n")
                supervisor_process.stdin.flush()
                if not supervisor_process.stdout.readline():
                    restore("supervisor-died-during-apply")
                    continue
                stop(stock)
                injected, identity = child(root, token + "-injected", "runtime")
                identities.append(identity)
                state = "Activating"
                deadline = time.monotonic() + 5
                report("activated", scope=scope)
            elif op in ("ready", "heartbeat"):
                valid = (command.get("scope") == scope and armed
                         and supervisor_process.poll() is None
                         and injected is not None and injected.poll() is None
                         and (state == "Ready" or op == "ready" and state == "Activating"
                              and time.monotonic() < deadline))
                if valid:
                    # Challenge goes through actual owned child's pipe, not log inference.
                    injected.stdin.write(json.dumps({"op": op, "scope": scope}) + "\n")
                    injected.stdin.flush()
                    response = json.loads(injected.stdout.readline())
                    valid = response == {"event": op, "scope": scope}
                if valid:
                    state = "Ready"
                    heartbeat = time.monotonic()
                report(op, accepted=valid)
            elif op == "kill-runtime" and injected:
                stop(injected)
                report("runtime-stopped")
            elif op == "begin-rollback":
                state = "RestoringStock"
                report("rollback-boundary")
            elif op == "block-rollback":
                fail_rollback = True
                report("rollback-blocked")
            elif op in ("disable", "update", "uninstall") and armed:
                restore(op)
            else:
                report("refused", op=op)
    finally:
        stop(injected)
        stop(stock)
        stop(supervisor_process)


if __name__ == "__main__":
    role, directory, ownership_token = sys.argv[1:]
    # Child start challenge varies while directory ownership remains runner token.
    directory_token = Path(directory).name.removeprefix("buddy-e0-")
    base = owned(directory, directory_token)
    if not ownership_token.startswith(directory_token):
        raise ValueError("foreign child token")
    {"guard": guard, "supervisor": supervisor, "runtime": runtime}[role](base, ownership_token)
