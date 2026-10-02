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
        handle = kernel.OpenProcess(0x1000 | 0x100000, False, pid)
        if not handle:
            return None
        try:
            if kernel.WaitForSingleObject(wintypes.HANDLE(handle), 0) != 258:
                return None
            values = [wintypes.FILETIME() for _ in range(4)]
            if not kernel.GetProcessTimes(wintypes.HANDLE(handle), *map(ctypes.byref, values)):
                return None
            return (values[0].dwHighDateTime << 32) | values[0].dwLowDateTime
        finally:
            kernel.CloseHandle(wintypes.HANDLE(handle))
    try:
        raw = Path(f"/proc/{pid}/stat").read_text()
        fields = raw[raw.rfind(")") + 2:].split()
        return None if fields[0] == "Z" else int(fields[19])
    except (FileNotFoundError, ProcessLookupError):
        return None


def atomic(path, data):
    temporary = path.with_name(path.name + ".partial." + str(os.getpid()))
    temporary.write_bytes(data)
    os.replace(temporary, path)


def emit(value):
    print(json.dumps(value), flush=True)


def owned(root, token):
    root = Path(root).resolve(strict=True)
    if root.name != "buddy-e0-" + token or (root / "owner").read_text() != token:
        raise ValueError("not an owned harness directory")
    return root


def read_line(pipe, timeout):
    result = queue.Queue()
    threading.Thread(target=lambda: result.put(pipe.readline()), daemon=True).start()
    return result.get(timeout=max(0.001, timeout))


def child(root, token, role, deadline=None):
    p = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), role,
                          str(root), token], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        bound = min(2, deadline - time.monotonic()) if deadline is not None else 2
        hello = json.loads(read_line(p.stdout, bound))
        if hello != {"role": role, "pid": p.pid, "start": token + ":" + role}:
            raise RuntimeError("owned child start challenge failed")
        hello["os_start"] = start_identity(p.pid)
        if hello["os_start"] is None:
            raise RuntimeError("missing OS process start identity")
        return p, hello
    except BaseException:
        stop(p)
        for pipe in (p.stdin, p.stdout, p.stderr):
            pipe.close()
        raise


def stop(p):
    if p and p.poll() is None:
        p.kill()
    if p:
        p.wait(timeout=2)


def supervisor(root, token):
    emit({"role": "supervisor", "pid": os.getpid(), "start": token + ":supervisor"})
    guard_pid = os.getppid()
    guard_start = start_identity(guard_pid)
    armed = False
    disabled = False
    injected = None
    injected_identity = None
    scope = None
    unrelated = digest(root / "user-config")
    stock = None
    completion = None
    restore_runs = 0
    def restore_owned():
        nonlocal stock, completion, disabled, restore_runs
        if completion is not None:
            return completion
        disabled = True  # Fence this serialized activation writer before effects.
        restore_runs += 1
        deadline = time.monotonic() + 5
        try:
            stop(injected)
            atomic(root / "config", (root / "stock-baseline").read_bytes())
            for name in ("session", "config.partial", "session.partial"):
                (root / name).unlink(missing_ok=True)
            stock, identity = child(root, token + "-supervisor-stock", "runtime", deadline)
            if time.monotonic() >= deadline:
                raise TimeoutError("late recovery")
            completion = {"state": "DisabledForSession", "stock": identity,
                          "transaction": token, "scope": scope, "restore_runs": restore_runs,
                          "config_hash": digest(root / "config"),
                          "baseline_hash": digest(root / "stock-baseline"),
                          "unrelated_preserved": digest(root / "user-config") == unrelated,
                          "injected_identity": injected_identity,
                          "injected_gone": injected is None or injected.poll() is not None}
        except (TimeoutError, queue.Empty, OSError, ValueError, RuntimeError):
            completion = {"state": "RecoveryFailed", "transaction": token,
                          "scope": scope, "restore_runs": restore_runs}
        atomic(root / "supervisor-recovery", json.dumps(completion).encode())
        return completion
    try:
        for line in sys.stdin:
            command = json.loads(line)
            op = command["op"]
            if op == "arm":
                if disabled:
                    emit({"ack": op, "refused": True})
                    continue
                armed = True
            elif op == "prepare":
                if not armed or disabled or injected is not None or start_identity(guard_pid) != guard_start:
                    emit({"ack": op, "refused": True})
                    continue
                # Supervisor holds the Popen before any enabling configuration is written.
                injected, injected_identity = child(root, token + "-injected", "runtime")
                scope = dict(command["scope"], process={"pid": injected.pid, "os_start": injected_identity["os_start"]})
                emit({"ack": op, "identity": injected_identity, "scope": scope})
                continue
            elif op == "apply":
                if disabled or not armed or injected is None or command["scope"] != scope or start_identity(guard_pid) != guard_start:
                    emit({"ack": op, "refused": True})
                    continue
                atomic(root / "session", json.dumps(scope).encode())
                atomic(root / "config", b"injected\n")
            elif op == "partial":
                if not armed or disabled:
                    emit({"ack": op, "refused": True})
                    continue
                (root / "config.partial").write_bytes(b"incom")
            elif op == "disable":
                disabled = True
                stop(injected)
            elif op == "restore":
                if not armed or command.get("scope") != scope:
                    emit({"ack": op, "refused": True})
                else:
                    emit({"ack": op, "receipt": restore_owned()})
                continue
            elif op == "kill-target":
                stop(injected)
            elif op == "runtime-event":
                if disabled or injected is None or injected.poll() is not None or command["scope"] != scope:
                    emit({"ack": op, "refused": True})
                    continue
                injected.stdin.write(json.dumps({"op": command["event"], "scope": scope}) + "\n")
                injected.stdin.flush()
                response = json.loads(read_line(injected.stdout, 2))
                emit({"ack": op, "response": response})
                continue
            emit({"ack": op})
    except Exception:
        # Any failed private command/acknowledgment channel withdraws protection.
        # Redirect the broken stdout descriptor so interpreter flush cannot bypass
        # the armed restoration path or produce a misleading late process exit.
        with open(os.devnull, 'w') as sink:
            os.dup2(sink.fileno(), sys.stdout.fileno())
    # Loss of the private guard pipe is an event, not a timer-based success claim.
    # Surviving Supervisor restores stock independently of the runner.
    if armed:
        restore_owned()
    while not (root / "stop-owned-children").exists():
        time.sleep(0.01)
    stop(stock)
    stop(injected)


class ObservedChild:
    """Identity observation only; no PID-name killing or native process control."""
    def __init__(self, identity):
        self.identity = identity

    def poll(self):
        return None if start_identity(self.identity["pid"]) == self.identity["os_start"] else 0

    def wait(self, timeout):
        deadline = time.monotonic() + timeout
        while self.poll() is None:
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired("owned child observation", timeout)
            time.sleep(0.01)


def runtime(root, token):
    emit({"role": "runtime", "pid": os.getpid(), "start": token + ":runtime"})
    # This fake process has no device calls. It responds to explicit stdin events.
    for line in sys.stdin:
        command = json.loads(line)
        if command["op"] == "exit":
            return
        emit({"event": command["op"], "scope": command["scope"]})
    while not (root / "stop-owned-children").exists() and (root / "config").read_bytes() != b"stock\n":
        time.sleep(0.01)


def guard(root, token):
    emit({"role": "guard", "pid": os.getpid(), "start": token + ":guard"})
    baseline = (root / "stock-baseline").read_bytes()
    # Simulated reboot reconstructs runtime-only state, with payload still inert.
    atomic(root / "config", baseline)
    for name in ("session", "config.partial", "session.partial", "stop-owned-children", "supervisor-recovery"):
        (root / name).unlink(missing_ok=True)
    unrelated = digest(root / "user-config")
    supervisor_process, supervisor_identity = child(root, token, "supervisor")
    try:
        stock, stock_identity = child(root, token + "-stock", "runtime")
    except BaseException:
        stop(supervisor_process)
        raise
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
    def rpc(op, **fields):
        supervisor_process.stdin.write(json.dumps({"op": op, **fields}) + "\n")
        supervisor_process.stdin.flush()
        return json.loads(read_line(supervisor_process.stdout, 2))
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
        nonlocal state, stock, armed, deadline
        state = "RestoringStock"
        deadline = time.monotonic() + 5
        if fail_rollback:
            report("rollback-boundary", reason=reason)
            return
        try:
            if supervisor_process.poll() is None:
                if isinstance(stock, subprocess.Popen):
                    stop(stock)
                result = rpc("restore", scope=scope)
                receipt = result.get("receipt", {})
                if receipt.get("state") != "DisabledForSession":
                    raise RuntimeError("Supervisor restoration refused or failed")
                identity = receipt["stock"]
                stock = ObservedChild(identity)
                identities.append(identity)
            else:
                atomic(root / "config", baseline)
                if injected:
                    injected.wait(max(0.001, deadline - time.monotonic()))
                for name in ("session", "config.partial", "session.partial"):
                    (root / name).unlink(missing_ok=True)
                if stock.poll() is not None:
                    stock, identity = child(root, token + "-restored", "runtime", deadline)
                    identities.append(identity)
            if time.monotonic() >= deadline:
                raise TimeoutError("rollback completed too late")
        except (TimeoutError, queue.Empty, subprocess.TimeoutExpired, OSError, ValueError, RuntimeError):
            state = "RecoveryFailed"
            armed = False
            report("recovery-failed", reason=reason)
            return
        state = "DisabledForSession"
        armed = False
        report("restored", reason=reason)
    report("boot")
    try:
        while True:
            now = time.monotonic()
            if state == "RestoringStock" and now >= deadline:
                state = "RecoveryFailed"
                armed = False
                report("recovery-failed", reason="rollback-deadline")
            elif armed and supervisor_process.poll() is not None and not (fail_rollback and state == "RestoringStock"):
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
            if op == "kill-both":
                stop(supervisor_process)
                os._exit(70)
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
                rpc("arm")
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
            elif op in ("lose-prepare-ack", "lose-apply-ack"):
                if op == "lose-prepare-ack" and state == "RecoveryArmed":
                    attempts += 1
                    scope = {"boot": token, "generation": attempts,
                             "nonce": token + "-challenge", "process": token + ":injected"}
                    request = {"op": "prepare", "scope": scope}
                elif op == "lose-apply-ack" and state == "Prepared":
                    request = {"op": "apply", "scope": scope}
                else:
                    report("refused", op=op)
                    continue
                supervisor_process.stdout.close()
                supervisor_process.stdin.write(json.dumps(request) + "\n")
                supervisor_process.stdin.flush()
                armed = False
                state = "ChannelLost"
                report("ack-pipe-lost")
            elif op == "lose-restore-ack" and state == "Ready":
                stop(stock)
                supervisor_process.stdout.close()
                supervisor_process.stdin.write(json.dumps({"op": "restore", "scope": scope}) + "\n")
                supervisor_process.stdin.flush()
                armed = False
                state = "ChannelLost"
                report("ack-pipe-lost")
            elif op == "restore-race" and state == "Ready":
                stop(stock)
                request = json.dumps({"op": "restore", "scope": scope}) + "\n"
                supervisor_process.stdin.write(request + request)
                supervisor_process.stdin.flush()
                receipts = [json.loads(read_line(supervisor_process.stdout, 2))["receipt"] for _ in range(2)]
                stock = ObservedChild(receipts[0]["stock"])
                identities.append(receipts[0]["stock"])
                armed = False
                state = "DisabledForSession"
                report("restored", receipts=receipts)
            elif op == "foreign-restore" and state == "Ready":
                foreign = dict(scope, nonce="foreign")
                if command.get("field") == "process":
                    foreign = dict(scope, process={"pid": scope["process"]["pid"], "os_start": 0})
                result = rpc("restore", scope=foreign)
                report("restore-refused", refused=result.get("refused", False))
            elif op == "late-apply" and state == "DisabledForSession":
                result = rpc("apply", scope=scope)
                partial = rpc("partial")
                report("late-refused", refused=result.get("refused", False), partial_refused=partial.get("refused", False))
            elif op in ("prepare", "activate") and state == "RecoveryArmed" and attempts == 0:
                attempts += 1
                scope = {"boot": token, "generation": attempts,
                         "nonce": token + "-challenge", "process": token + ":injected"}
                result = rpc("prepare", scope=scope)
                if result.get("refused"):
                    restore("prepare-refused")
                    continue
                identity = result["identity"]
                scope = result["scope"]
                injected = ObservedChild(identity)
                identities.append(identity)
                if op == "prepare":
                    state = "Prepared"
                    report("prepared", scope=scope)
                    continue
                result = rpc("apply", scope=scope)
                if result.get("refused"):
                    restore("apply-refused")
                    continue
                stop(stock)
                state = "Activating"
                deadline = time.monotonic() + 5
                report("activated", scope=scope)
            elif op == "apply" and state == "Prepared":
                result = rpc("apply", scope=scope)
                if result.get("refused"):
                    restore("apply-refused")
                else:
                    state = "Applied"
                    report("applied", scope=scope)
            elif op == "stop-stock" and state == "Applied":
                stop(stock)
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
                    result = rpc("runtime-event", event=op, scope=scope)
                    response = result.get("response")
                    valid = response == {"event": op, "scope": scope}
                if valid:
                    state = "Ready"
                    heartbeat = time.monotonic()
                report(op, accepted=valid)
            elif op == "kill-runtime" and injected:
                rpc("kill-target")
                report("runtime-stopped")
            elif op == "begin-rollback":
                state = "RestoringStock"
                deadline = time.monotonic() + 5
                report("rollback-boundary")
            elif op == "block-rollback":
                fail_rollback = True
                report("rollback-blocked")
            elif op in ("disable", "update", "uninstall") and armed:
                restore(op)
            else:
                report("refused", op=op)
    finally:
        if injected and supervisor_process.poll() is None:
            try:
                rpc("disable")
            except (OSError, ValueError, queue.Empty):
                pass
        if isinstance(stock, subprocess.Popen):
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
