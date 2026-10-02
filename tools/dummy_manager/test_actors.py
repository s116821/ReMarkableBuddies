"""Real host processes with explicitly FAKE cgroup files; no manager/device."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
import uuid
import errno


class ActorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e0t-actors-")
        self.parent = Path(self.temp.name)
        self.nonce = uuid.uuid4().hex
        self.root = self.parent / ("buddy-e0t-" + self.nonce)
        self.root.mkdir(mode=0o700)
        (self.root / "owner").write_text(self.nonce)
        self.cgroups = self.parent / "FAKE-cgroups"
        self.cgroups.mkdir()
        for role in ("controller", "guard"):
            group = self.cgroups / ("buddy-e0t-" + self.nonce + "-" + role + ".service")
            group.mkdir()
            (group / "cgroup.procs").write_bytes(b"")
        for role in ("controller", "guard", "stock"):
            os.mkfifo(self.root / ("control-" + role), 0o600)
        self.binary = self.root / "helper"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-DE0T_ACTORS",
                        '-DE0T_NONCE="' + self.nonce + '"',
                        '-DE0T_RUNTIME_PARENT="' + str(self.parent) + '"',
                        '-DE0T_CGROUP_PARENT="' + str(self.cgroups) + '"',
                        str(Path(__file__).with_name("helper.c")), "-o", str(self.binary)], check=True)
        self.processes = {}

    def tearDown(self):
        for role, process in self.processes.items():
            if process.poll() is None:
                try:
                    self.send(role, "X")
                    process.wait(timeout=3)
                except (OSError, subprocess.TimeoutExpired):
                    process.kill()
                    process.wait(timeout=3)
            process.stderr.close()
        # Stock is an actor-owned child, never terminated by arbitrary PID here.
        identities = list(self.root.glob("identity-T*-stock"))
        for identity in identities:
            pid, start = map(int, identity.read_text().split())
            deadline = time.monotonic() + 3
            while self.live(pid, start):
                if time.monotonic() >= deadline:
                    self.fail("actor stock child survived cleanup; retaining owned evidence")
                time.sleep(0.01)
        self.temp.cleanup()

    @staticmethod
    def live(pid, expected):
        try:
            data = Path(f"/proc/{pid}/stat").read_text()
        except FileNotFoundError:
            return False
        fields = data[data.rfind(")") + 2:].split()
        return fields[0] != "Z" and int(fields[19]) == expected

    def event(self, role, text):
        path = self.root / f"events-T{self.case}-{role}"
        deadline = time.monotonic() + 3
        while True:
            data = path.read_text() if path.exists() else ""
            if " " + text + "\n" in data:
                return data
            if time.monotonic() >= deadline:
                self.fail((text, data, {r: p.poll() for r, p in self.processes.items()}))
            time.sleep(0.01)  # Fresh event polling, never elapsed-time success.

    def send(self, role, message):
        deadline = time.monotonic() + 3
        while True:
            try:
                fd = os.open(self.root / ("control-" + role), os.O_WRONLY | os.O_NONBLOCK)
                break
            except OSError as error:
                if error.errno != errno.ENXIO or time.monotonic() >= deadline:
                    raise
                time.sleep(0.01)
        try:
            self.assertEqual(os.write(fd, message.encode()), 1)
        finally:
            os.close(fd)

    def start(self, case):
        self.case = case
        state = self.root / f"state-T{case}"
        state.write_text("injected\n")
        state.chmod(0o600)
        for role in ("controller", "guard"):
            self.processes[role] = subprocess.Popen([str(self.binary), role, str(case)], stderr=subprocess.PIPE)
        for role in self.processes:
            self.event(role, "protection-observed")

    def lose(self, role):
        self.send(role, "B")
        self.event(role, "restoration-begun")
        self.processes[role].kill()
        self.processes[role].wait(timeout=3)

    def restored(self, role):
        self.event(role, "restored")
        self.assertEqual((self.root / f"state-T{self.case}").read_bytes(), b"stock\n")
        pid, start = map(int, (self.root / f"receipt-T{self.case}").read_text().split())
        self.assertTrue(self.live(pid, start))
        return pid, start

    def test_controller_loss_recovers_owned_state_and_live_child(self):
        self.start(6)
        self.lose("controller")
        self.restored("guard")

    def test_guard_loss_fences_actual_late_publication(self):
        self.start(7)
        self.lose("guard")
        self.restored("controller")
        self.send("controller", "L")
        self.event("controller", "late-publication-refused")
        self.assertEqual((self.root / "state-T7").read_bytes(), b"stock\n")

    def test_cached_child_loss_query_fails_without_second_claim(self):
        self.start(8)
        self.lose("guard")
        pid, start = self.restored("controller")
        claim = (self.root / "restore-claim-T8").read_bytes()
        self.send("controller", "K")
        self.event("controller", "owned-stock-stopped")
        self.assertFalse(self.live(pid, start))
        self.send("controller", "Q")
        self.event("controller", "fresh-restoration-unknown")
        self.assertEqual((self.root / "restore-claim-T8").read_bytes(), claim)
        self.assertEqual((self.root / "events-T8-stock").read_text().count(" started\n"), 1)

    def test_nonempty_fake_old_cgroup_refuses_fork(self):
        self.start(6)
        group = self.cgroups / ("buddy-e0t-" + self.nonce + "-controller.service")
        (group / "cgroup.procs").write_text("99999\n")
        self.lose("controller")
        self.event("guard", "restoration-unknown")
        self.assertFalse((self.root / "identity-T6-stock").exists())
        self.assertFalse((self.root / "restore-claim-T6").exists())

    def test_spent_claim_without_identity_is_unknown_without_fork(self):
        self.start(6)
        claim = self.root / "restore-claim-T6"
        claim.write_text("simulated interrupted claim/fork publication\n")
        claim.chmod(0o600)
        self.lose("controller")
        self.event("guard", "restoration-unknown")
        self.assertFalse((self.root / "identity-T6-stock").exists())
        self.assertEqual((self.root / "state-T6").read_bytes(), b"injected\n")


if __name__ == "__main__":
    unittest.main()
