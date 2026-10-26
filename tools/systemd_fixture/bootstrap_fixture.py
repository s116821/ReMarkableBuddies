"""Run one explicit fake-service bootstrap case on an authorized Linux target.

SSH prefix JSON contains only the already authorized ssh executable/options/host,
without a remote command. No real service is started/stopped. Every invocation
uses a new nonce; failures retain all remote/local files and must not be replayed.
Cleanup is a separate, evidence-reviewed operation, not part of this runner.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import shlex
import subprocess
import tarfile
import time
import uuid


CASES = ("none", "actor_kill", "actor_hang", "after_buddy_stop",
         "recovery_exit", "recovery_after_intent", "recovery_after_start", "source_mismatch")
PROPERTIES = ("Id", "ActiveState", "SubState", "MainPID", "ControlPID", "Job",
              "InvocationID", "ExecMainStartTimestampMonotonic", "Result")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def strict_json(data):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON field")
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=unique)


def states(data):
    result = {}
    for block in data.decode().strip().split("\n\n"):
        values = {}
        for line in block.splitlines():
            key, value = line.split("=", 1)
            if key in values:
                raise ValueError("duplicate service property")
            values[key] = value
        if set(values) != set(PROPERTIES) or values["Id"] in result:
            raise ValueError("ambiguous service snapshot")
        result[values["Id"]] = values
    return result


def terminal(state):
    return (state["ActiveState"] in ("inactive", "failed")
            and state["MainPID"] == state["ControlPID"] == "0"
            and not state["Job"])


def verify_states(case, unit, original, first, second):
    expected_actor = {"actor_kill": "signal", "actor_hang": "timeout",
                      "after_buddy_stop": "exit-code", "source_mismatch": "exit-code"}.get(case, "success")
    if first[unit("actor")]["Result"] != expected_actor:
        raise AssertionError("actor did not reach the selected completion/fault")
    expected_first = {"recovery_after_intent": "signal", "recovery_after_start": "signal",
                      "recovery_exit": "exit-code", "source_mismatch": "exit-code"}.get(case, "success")
    if first[unit("recovery")]["Result"] != expected_first:
        raise AssertionError("first helper did not reach the selected completion/fault")
    if (first[unit("recovery")]["ExecMainStartTimestampMonotonic"]
            == second[unit("recovery")]["ExecMainStartTimestampMonotonic"]):
        raise AssertionError("second helper invocation not observed")
    expected_helper = "exit-code" if case in ("recovery_after_intent", "source_mismatch") else "success"
    if second[unit("recovery")]["Result"] != expected_helper:
        raise AssertionError("unexpected second helper result")
    for role in ("stock", "buddy"):
        final = second[unit(role)]
        if case == "source_mismatch" and role == "buddy":
            if not all(terminal(sample[unit(role)]) for sample in (first, second)):
                raise AssertionError("source mismatch admitted Buddy")
        elif case == "recovery_after_intent":
            if not terminal(final):
                raise AssertionError("uncertain start was replayed")
        elif final["ActiveState"] != "active" or final["Job"]:
            raise AssertionError("stand-in restoration incomplete")
        elif case == "after_buddy_stop" and role == "stock":
            if final["InvocationID"] != original[unit(role)]["InvocationID"]:
                raise AssertionError("unadmitted stock was restarted")
        elif final["InvocationID"] == original[unit(role)]["InvocationID"]:
            raise AssertionError("selected stop/restoration was not exercised")
        if first[unit(role)]["ActiveState"] == "active":
            if final["InvocationID"] != first[unit(role)]["InvocationID"]:
                raise AssertionError("second helper restarted an active stand-in")
    if case == "source_mismatch":
        stock = first[unit("stock")]
        if (stock["ActiveState"] != "active" or stock["SubState"] != "running"
                or stock["MainPID"] == "0" or stock["ControlPID"] != "0" or stock["Job"]
                or stock["InvocationID"] == original[unit("stock")]["InvocationID"]):
            raise AssertionError("source mismatch delayed original stock restoration")


def verify_archive(data, nonce, case):
    prefix = "rsdk-control-" + nonce + "/"
    records = {}
    total = 0
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            if member.isdir() and member.name.rstrip("/") == prefix.rstrip("/"):
                continue
            leaf = member.name.removeprefix(prefix)
            total += member.size
            if (not member.name.startswith(prefix) or not member.isfile() or "/" in leaf
                    or leaf in ("", ".", "..") or leaf in records
                    or member.size > 256 * 1024 or total > 4 * 1024 * 1024
                    or len(records) >= 128):
                raise ValueError("unexpected control archive entry")
            records[leaf] = archive.extractfile(member).read()
    if records.get("buddy.stop-intent") != nonce.encode() or "ownership.json" not in records:
        raise AssertionError("original ownership/first stop not exercised")
    ownership = strict_json(records["ownership.json"])
    if ownership.get("version") != 1:
        raise ValueError("invalid ownership version")
    if case == "after_buddy_stop":
        if "stock.stop-intent" in records or "observer.launch-intent" in records:
            raise AssertionError("partial-stop fault did not prevent later effects")
    else:
        for leaf in ("stock.stop-intent", "observer.launch-intent", "stock.start-intent"):
            if records.get(leaf) != nonce.encode():
                raise AssertionError("missing selected effect intent: " + leaf)
        if case == "recovery_after_intent" and "buddy.start-intent" in records:
            raise AssertionError("uncertain stock restoration admitted Buddy")
    if case in ("actor_kill", "actor_hang"):
        ready = strict_json(records["fixture-child-ready"])
        observed = strict_json(records["fixture-child-observed"])
        owner = strict_json(ownership["actor"])["group"]
        if (set(ready) != {"pid", "start_ticks"}
                or any(type(ready[key]) is not int or ready[key] <= 0 for key in ready)
                or ready["pid"] <= 1 or observed["parent"] != ready
                or ready["pid"] == owner["parent"]["pid"]
                or {key: value for key, value in observed.items() if key != "parent"}
                != {key: value for key, value in owner.items() if key != "parent"}
                or owner["unit"] != f"rsdk-bootstrap-{nonce}-actor.service"
                or records["fixture-child-ready-complete"] != b""):
            raise AssertionError("child identity/group witness differs from the original actor")
    if case.startswith("recovery_"):
        leaf = "fixture-" + case.replace("_", "-")
        if records.get(leaf) != nonce.encode():
            raise AssertionError("selected recovery fault phase was not recorded")
    if case == "source_mismatch":
        if records.get("fixture-source-mismatch") != nonce.encode():
            raise AssertionError("source mismatch injection was not recorded")
        if records.get("observer.exit") != b"exit status: 0":
            raise AssertionError("source mismatch observer did not complete successfully")
        if any(name == "buddy.start-intent" or name.endswith((".buddy.start-intent", ".buddy.restored"))
               for name in records):
            raise AssertionError("source mismatch admitted Buddy")
        for prefix, suffix, count, expected in (
            ("actor-result-", "", 1, b"Err(source manifest changed)\n"),
            ("recovery-result-", "", 2, b"Err(source manifest changed)\n"),
            ("recovery-", ".source-after", 2, b"Err(source manifest changed)"),
        ):
            found = [value for name, value in records.items() if name.startswith(prefix) and name.endswith(suffix)]
            if len(found) != count or any(value != expected for value in found):
                raise AssertionError("source mismatch lacks exact preservation refusals")
        restored = [value for name, value in records.items()
                    if name.startswith("recovery-") and name.endswith(".stock.restored")]
        if len(restored) != 2 or restored[0] != restored[1]:
            raise AssertionError("source mismatch lacks both stock restoration receipts")
        stock = strict_json(restored[0])
        if stock.get("version") != 1 or stock.get("group", {}).get("unit") != f"rsdk-bootstrap-{nonce}-stock.service":
            raise AssertionError("source mismatch stock receipt identity differs")
        fields = {}
        for line in stock["snapshot"].splitlines():
            key, value = line.split("=", 1)
            if key in fields:
                raise ValueError("duplicate restored stock property")
            fields[key] = value
        if (fields.get("Id") != stock["group"]["unit"] or fields.get("ActiveState") != "active"
                or fields.get("SubState") != "running" or int(fields.get("MainPID", "0")) <= 1
                or fields.get("ControlPID") != "0" or fields.get("Job") != ""
                or not fields.get("InvocationID")):
            raise AssertionError("source mismatch stock receipt is not running")
    return {name: digest(value) for name, value in records.items()}


class Run:
    def __init__(self, args):
        self.args = args
        self.nonce = uuid.uuid4().hex
        self.stage = "/home/root/rsdk-bootstrap-" + self.nonce
        self.control = "/run/rsdk-control-" + self.nonce
        self.binary = self.stage + "/resident_bootstrap_control"
        self.output = args.output / ("bootstrap-fixture-" + self.nonce)
        self.output.mkdir(parents=True, exist_ok=False)
        self.prefix = json.loads(args.ssh_prefix_file.read_text())
        if not self.prefix or not all(isinstance(x, str) for x in self.prefix):
            raise ValueError("SSH prefix must be a nonempty string array")
        self.sequence = 0
        self.receipt = {"nonce": self.nonce, "case": args.case,
                        "source_sha": args.source_sha, "native_authority": False,
                        "calls": [], "result": "incomplete"}
        self.save()

    def save(self):
        (self.output / "receipt.json").write_text(json.dumps(self.receipt, indent=2) + "\n")

    def remote(self, label, command, data=None, timeout=20, required=True):
        self.sequence += 1
        leaf = f"{self.sequence:03d}-{label}"
        call = {"label": leaf, "command": command, "timeout_seconds": timeout}
        if data is not None:
            call["stdin_sha256"] = digest(data)
        self.receipt["calls"].append(call)
        self.save()  # Persist selection before executing; never retry on timeout.
        try:
            result = subprocess.run(self.prefix + [command], input=data,
                                    capture_output=True, timeout=timeout)
            call["returncode"] = result.returncode
            stdout, stderr = result.stdout, result.stderr
        except subprocess.TimeoutExpired as error:
            call["timeout"] = True
            stdout, stderr = error.stdout or b"", error.stderr or b""
            result = None
        (self.output / (leaf + ".stdout")).write_bytes(stdout)
        (self.output / (leaf + ".stderr")).write_bytes(stderr)
        self.save()
        if required and (result is None or result.returncode):
            raise RuntimeError(f"{leaf} failed; inspect preserved evidence")
        return stdout

    def unit(self, role):
        return f"rsdk-bootstrap-{self.nonce}-{role}.service"

    def show(self, roles):
        return self.remote("states", "LC_ALL=C systemctl show "
                           + " ".join(shlex.quote(self.unit(r)) for r in roles)
                           + " " + " ".join("-p " + p for p in PROPERTIES))

    def real(self):
        return self.remote("real-services", "LC_ALL=C systemctl show "
                           "xochitl.service reader-buddy.service "
                           + " ".join("-p " + p for p in PROPERTIES)
                           + " && sha256sum /usr/bin/xochitl "
                           "/usr/lib/systemd/system/xochitl.service "
                           "/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf "
                           "/etc/systemd/system/reader-buddy.service")

    def upload(self, label, path, data, executable=False):
        self.remote(label, "umask 077; set -euC; cat > " + shlex.quote(path)
                    + ("; chmod 700 " + shlex.quote(path) if executable else ""),
                    data, timeout=40)

    def wait(self, initial=False, previous=None):
        deadline = time.monotonic() + (20 if initial else 145)
        while time.monotonic() < deadline:
            current = states(self.show(("actor", "recovery", "stock", "buddy")))
            if initial:
                if all(current[self.unit(r)]["ActiveState"] == "active"
                       and not current[self.unit(r)]["Job"] for r in ("stock", "buddy")):
                    return current
            else:
                helper = current[self.unit("recovery")]
                if (terminal(current[self.unit("actor")]) and terminal(helper)
                        and helper["ExecMainStartTimestampMonotonic"] not in ("0", previous)):
                    return current
            time.sleep(0.5)  # State-driven polling, never elapsed-time success.
        raise TimeoutError("original fixture wait deadline expired; no replay")

    def execute(self):
        before = self.real()
        original = None
        try:
            roles = ("actor", "recovery", "stock", "buddy")
            paths = ["/run/systemd/system/" + self.unit(r) for r in roles]
            check = "set -eu; test $(id -u) = 0; test -d /sys/fs/cgroup/unified; "
            for path in [self.stage, self.control, *paths]:
                check += f"test ! -e {shlex.quote(path)}; test ! -L {shlex.quote(path)}; "
            for role in roles:
                check += (f"test \"$(systemctl show {self.unit(role)} -p LoadState --value)\" "
                          "= not-found; ")
            check += f"umask 077; mkdir -m 700 {self.stage}; mkdir -m 700 {self.stage}/fixture-data"
            self.remote("exclusive-stage", check)
            binary = self.args.binary.read_bytes()
            self.receipt["controller_sha256"] = digest(binary)
            self.save()
            self.upload("controller", self.binary, binary, executable=True)
            actual = self.remote("binary-hash", "sha256sum " + self.binary).decode().split()[0]
            if actual != digest(binary):
                raise ValueError("transferred controller hash mismatch")
            fixture = b"Disposable bootstrap source fixture.\n"
            self.upload("fixture-source", self.stage + "/fixture-data/source.txt", fixture)
            rendered = self.remote("render-units", f"{self.binary} units {self.nonce} fixture")
            units = json.loads(rendered)
            if set(units) != {self.unit(role) for role in roles}:
                raise ValueError("unexpected unit inventory")
            for role, path in zip(roles, paths):
                text = units[self.unit(role)].encode()
                (self.output / self.unit(role)).write_bytes(text)
                self.upload("unit-" + role, path, text)
            self.remote("reload", "systemctl daemon-reload")
            self.remote("start-standins", "systemctl --no-block --job-mode=fail start "
                        + self.unit("stock") + " " + self.unit("buddy"))
            original = self.wait(initial=True)
            # Retain the never-started helper's effective policy, including the
            # systemd 255 WatchdogUSec=infinity sentinel before its first start.
            self.remote("diagnostic-policy", "LC_ALL=C systemctl show "
                        + self.unit("actor") + " " + self.unit("recovery")
                        + " -p Id -p Type -p ActiveState -p SubState -p MainPID -p ControlPID"
                        + " -p ExecMainPID -p ExecMainStartTimestampMonotonic -p InvocationID"
                        + " -p ControlGroup -p Job -p WatchdogUSec -p FragmentPath"
                        + " -p DropInPaths -p NeedDaemonReload")
            sample = json.loads(self.remote("inspect", f"{self.binary} inspect {self.nonce} fixture"))
            candidate = {"version": 1, "nonce": self.nonce, "kind": "fixture",
                         "fault": self.args.case, "boot": sample["boot"],
                         "controller_sha256": digest(binary), "observer_sha256": digest(binary),
                         "backup_sha256": digest(fixture), "stock": sample["services"]["stock"],
                         "buddy": sample["services"]["buddy"], "source": sample["source"],
                         "unit_files": sample["unit_files"]}
            frozen = json.dumps(candidate, separators=(",", ":")).encode()
            (self.output / "candidate.json").write_bytes(frozen)
            self.upload("candidate", self.stage + "/candidate.json", frozen)
            if self.real() != before:
                raise RuntimeError("real service baseline changed before fixture actor")
            self.remote("start-actor", "systemctl --no-block --job-mode=fail start " + self.unit("actor"))
            first = self.wait()
            self.receipt["first"] = first
            self.save()
            # Exactly one explicitly selected second helper attempt tests durable
            # admission. It may verify active stand-ins, never replay a start claim.
            self.remote("second-helper", "systemctl --no-block --job-mode=fail start " + self.unit("recovery"))
            second = self.wait(previous=first[self.unit("recovery")]["ExecMainStartTimestampMonotonic"])
            self.receipt["second"] = second
            self.receipt["original"] = original
            self.save()
            verify_states(self.args.case, self.unit, original, first, second)
            self.receipt["result"] = "state-assertions-passed"
        finally:
            # Read-only collection also runs after failures; it never retries actors.
            try:
                self.receipt["real_services_unchanged"] = self.real() == before
            except (RuntimeError, OSError) as error:
                self.receipt["real_services_unchanged"] = False
                self.receipt["final_observation_error"] = str(error)
            self.remote("journal", "journalctl --no-pager -n 200 -o short-monotonic "
                        + " ".join("-u " + self.unit(r) for r in ("actor", "recovery", "stock", "buddy")),
                        required=False)
            archive = self.remote("control-archive", "tar -C /run -czf - " + "rsdk-control-" + self.nonce,
                                  required=False)
            self.save()
        if not self.receipt["real_services_unchanged"]:
            raise AssertionError("real service baseline changed")
        self.receipt["records"] = verify_archive(archive, self.nonce, self.args.case)
        self.receipt["result"] = "passed"
        self.save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssh-prefix-file", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--case", choices=CASES, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.source_sha):
        parser.error("source SHA must be the exact artifact build revision")
    run = Run(args)
    print(run.output, flush=True)
    run.execute()
    print(json.dumps(run.receipt, indent=2), flush=True)


if __name__ == "__main__":
    main()
