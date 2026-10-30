"""Host C stand-in only: never invokes systemctl or a device manager."""
import fcntl
import ctypes
import os
from pathlib import Path
import subprocess
import tempfile
import time
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

    def build(self, body, flags=()):
        fake = self.parent / "fake-cli"
        source = self.parent / "fake.c"
        source.write_text('#include <stdlib.h>\n#include <sys/resource.h>\n#include <unistd.h>\n#include <stdio.h>\n#include <string.h>\nint main(int argc, char **argv) {' + body + '}\n')
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(source), "-o", str(fake)], check=True)
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-DE0T_ACTORS", "-DE0T_MANAGER", *flags,
                        '-DE0T_NONCE="' + self.nonce + '"',
                        '-DE0T_RUNTIME_PARENT="' + str(self.parent) + '"',
                        '-DE0T_SYSTEMCTL_PATH="' + str(fake) + '"',
                        str(Path(__file__).with_name("helper.c")), "-o", str(self.binary)], check=True)

    def run_command(self, expected, command="--manager-version"):
        result = subprocess.run([str(self.binary), command], capture_output=True, timeout=4,
                                env={**os.environ, "SYSTEMD_COLORS": "1", "SYSTEMD_LOG_TARGET": "journal", "E0T_UNTRUSTED": "present"})
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
        began = time.monotonic()
        self.run_command(90)
        self.assertLess(time.monotonic() - began, 2.5)
        self.assertFalse((self.root / "manager-child").exists())
        self.assertFalse((self.root / "manager-claim").exists())

    def test_output_eof_with_live_child_is_not_completion(self):
        self.build('(void)argc; (void)argv; close(1); close(2); for (;;) pause();')
        self.run_command(90)
        self.assertFalse((self.root / "manager-child").exists())
        self.assertFalse((self.root / "manager-claim").exists())

    def test_missing_exit_evidence_retains_intent_and_refuses_another_fork(self):
        libc = ctypes.CDLL(None, use_errno=True)
        previous = ctypes.c_int()
        self.assertEqual(libc.prctl(37, ctypes.byref(previous), 0, 0, 0), 0)
        self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)  # Adopt only this fixture's orphan for cleanup.
        try:
            self.build('(void)argc; (void)argv; for (;;) pause();', ("-DE0T_TEST_WAIT_FAULT",))
            self.assertIn(b"exit or bookkeeping deadline unknown", self.run_command(90).stderr)
            child = int((self.root / "manager-child").read_text().split()[0])
            claim = (self.root / "manager-claim").read_bytes()
            self.assertIn(b"unresolved previous", self.run_command(90).stderr)
            self.assertEqual((self.root / "manager-claim").read_bytes(), claim)
            deadline = time.monotonic() + 2
            while True:
                observed, _ = os.waitpid(child, os.WNOHANG)
                if observed == child:
                    break
                if time.monotonic() >= deadline:
                    self.fail("owned fault-fixture child did not exit")
                time.sleep(0.01)
        finally:
            self.assertEqual(libc.prctl(36, previous.value, 0, 0, 0), 0)

    def test_fixed_jobs_query_and_clean_child_environment(self):
        self.build('if(argc != 20 || strcmp(argv[1], "--no-pager") || strcmp(argv[2], "--no-ask-password") || strcmp(argv[3], "--no-legend") || strcmp(argv[4], "--plain") || strcmp(argv[5], "--full") || strcmp(argv[6], "list-jobs") || strcmp(argv[7], "--") || !getenv("LC_ALL") || strcmp(getenv("LC_ALL"), "C") || !getenv("SYSTEMD_COLORS") || strcmp(getenv("SYSTEMD_COLORS"), "0") || !getenv("SYSTEMD_LOG_TARGET") || strcmp(getenv("SYSTEMD_LOG_TARGET"), "console") || getenv("E0T_UNTRUSTED")) return 7; printf("123 %s start waiting\\n", argv[19]); return 0;')
        output = self.run_command(0, "--manager-jobs").stdout
        self.assertIn(self.nonce.encode(), output)
        self.assertTrue(output.endswith(b" start waiting\n"))

    def test_jobs_diagnostic_or_truncated_reply_refuses(self):
        self.build('(void)argc; (void)argv; puts("diagnostic error"); return 0;')
        self.assertIn(b"invalid owned job observation", self.run_command(90, "--manager-jobs").stderr)

    def test_complete_unit_snapshot_and_exact_readonly_targets(self):
        self.build('if(argc != 29 || strcmp(argv[5], "show") || strcmp(argv[13], "--") || strcmp(argv[26], "xochitl.service") || strcmp(argv[27], "reader-buddy.service") || strcmp(argv[28], "rm-sync.service")) return 7; for(int i=14;i<29;++i) printf("Id=%s\\nLoadState=loaded\\nActiveState=inactive\\nSubState=dead\\nMainPID=0\\nControlPID=0\\nJob=\\n\\n",argv[i]); return 0;')
        result = self.run_command(0, "--manager-units")
        self.assertEqual(result.stdout.count(b"Id="), 15)
        self.assertFalse((self.root / "manager-claim").exists())

    def test_incomplete_or_diagnostic_unit_snapshot_has_no_publication(self):
        for body in ('(void)argc; (void)argv; puts("diagnostic"); return 0;',
                     'printf("Id=%s\\nLoadState=loaded\\nActiveState=inactive\\nSubState=dead\\nMainPID=0\\nControlPID=0\\nJob=\\n",argv[14]); (void)argc; return 0;'):
            self.build(body)
            result = self.run_command(90, "--manager-units")
            self.assertEqual(result.stdout, b"")
            self.assertIn(b"invalid unit state", result.stderr)

    def test_writable_verbs_have_no_cli_entry(self):
        self.build('(void)argc; (void)argv; puts("unexpected-dispatch"); return 0;')
        for verb in ("--manager-start", "--manager-cancel", "--manager-stop", "--manager-reload"):
            self.assertEqual(self.run_command(90, verb).stdout, b"")
            self.assertFalse((self.root / "manager-claim").exists())


if __name__ == "__main__":
    unittest.main()
