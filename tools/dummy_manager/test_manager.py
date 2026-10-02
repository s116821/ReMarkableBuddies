"""Host C stand-in only: never invokes systemctl or a device manager."""
import fcntl
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid


class ManagerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e0t-manager-")
        self.parent = Path(self.temp.name)
        self.nonce = uuid.uuid4().hex
        self.root = self.parent / ("buddy-e0t-" + self.nonce)
        self.root.mkdir(mode=0o700)
        (self.root / "owner").write_text(self.nonce)
        self.binary = self.parent / "helper"

    def tearDown(self):
        self.temp.cleanup()

    def build(self, body):
        fake = self.parent / "fake-cli"
        source = self.parent / "fake.c"
        source.write_text('#include <sys/resource.h>\n#include <unistd.h>\n#include <stdio.h>\n#include <string.h>\nint main(int argc, char **argv) {' + body + '}\n')
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(source), "-o", str(fake)], check=True)
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-DE0T_ACTORS", "-DE0T_MANAGER",
                        '-DE0T_NONCE="' + self.nonce + '"',
                        '-DE0T_RUNTIME_PARENT="' + str(self.parent) + '"',
                        '-DE0T_SYSTEMCTL_PATH="' + str(fake) + '"',
                        str(Path(__file__).with_name("helper.c")), "-o", str(self.binary)], check=True)

    def run_command(self, expected):
        result = subprocess.run([str(self.binary), "--manager-version"], capture_output=True, timeout=4)
        self.assertEqual(result.returncode, expected, result.stderr)
        return result

    def test_fixed_command_limits_and_serial_reuse_after_actual_wait(self):
        self.build('struct rlimit r; if(argc != 2 || strcmp(argv[1], "--version") || getrlimit(RLIMIT_AS, &r) || r.rlim_cur != 20971520 || r.rlim_max != 20971520) return 7; puts("host-fake-version"); return 0;')
        for _ in range(2):
            self.assertEqual(self.run_command(0).stdout, b"host-fake-version\n")
            self.assertFalse((self.root / "manager-child").exists())
            self.assertFalse((self.root / "manager-claim").exists())

    def test_retained_claim_refuses_even_without_live_lock_owner(self):
        self.build('(void)argc; (void)argv; return 0;')
        claim = self.root / "manager-claim"
        claim.write_text("version 123 456\n")
        claim.chmod(0o600)
        self.assertIn(b"unresolved previous", self.run_command(90).stderr)
        self.assertEqual(claim.read_text(), "version 123 456\n")
        self.assertFalse((self.root / "manager-child").exists())

    def test_locked_slot_refuses_before_claim_or_child(self):
        self.build('(void)argc; (void)argv; return 0;')
        with open(self.root / "manager-slot", "w") as slot:
            os.chmod(slot.name, 0o600)
            fcntl.flock(slot, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertIn(b"slot unavailable", self.run_command(90).stderr)
        self.assertFalse((self.root / "manager-claim").exists())

    def test_output_overflow_is_failed_and_owned_child_reaped(self):
        self.build('(void)argc; (void)argv; char x[512]; memset(x, 65, sizeof(x)); for (;;) if(write(1, x, sizeof(x)) < 0) return 1;')
        self.run_command(90)
        self.assertFalse((self.root / "manager-child").exists())
        self.assertFalse((self.root / "manager-claim").exists())

    def test_wall_deadline_is_failed_and_owned_child_reaped(self):
        self.build('(void)argc; (void)argv; for (;;) pause();')
        self.run_command(90)
        self.assertFalse((self.root / "manager-child").exists())
        self.assertFalse((self.root / "manager-claim").exists())


if __name__ == "__main__":
    unittest.main()
