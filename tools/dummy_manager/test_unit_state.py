"""Pure synthetic property fixtures, not actual systemctl absent-unit behavior."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class UnitStateTests(unittest.TestCase):
    nonce = "0123456789abcdef0123456789abcdef"
    roles = ("cleanup", "controller", "guard", "stock", "fail-a", "fail-b", "claim",
             "norestart", "separate", "notify", "barrier", "queued")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e0t-unit-state-")
        root = Path(self.temp.name)
        source = root / "fixture.c"
        source.write_text('#include "unit_state.h"\nint main(void) { char data[4097]; struct e0t_unit_snapshot result; size_t n=fread(data,1,sizeof(data),stdin); if(!e0t_state_decode(data,n,&result)) return 90; printf("%u %u %u\\n",result.owned[0].main_pid,result.original_readonly[0].main_pid,result.owned[0].job); return 0; }\n')
        self.binary = root / "fixture"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                        '-DE0T_NONCE="' + self.nonce + '"', "-I", str(Path(__file__).parent),
                        str(source), "-o", str(self.binary)], check=True)

    def tearDown(self):
        self.temp.cleanup()

    def blocks(self, longest=False):
        names = [f"buddy-e0t-{self.nonce}-{role}.service" for role in self.roles]
        names += ["xochitl.service", "reader-buddy.service", "rm-sync.service"]
        result = []
        for index, name in enumerate(names):
            result.append({"Id": name, "LoadState": "bad-setting" if longest else "loaded",
                           "ActiveState": "deactivating" if longest else "active",
                           "SubState": "failed-before-auto-restart" if longest else "running",
                           "MainPID": "4294967295" if longest else str(index + 1),
                           "ControlPID": "4294967295" if longest else "0",
                           "Job": "4294967295" if longest else ""})
        return result

    def text(self, blocks):
        return "\n".join("".join(f"{key}={value}\n" for key, value in block.items()) for block in blocks).encode()

    def decode(self, data, expected):
        result = subprocess.run([str(self.binary)], input=data, capture_output=True, timeout=2)
        self.assertEqual(result.returncode, expected, data)
        return result.stdout

    def test_complete_original_and_owned_rows_are_distinct_and_order_independent(self):
        blocks = self.blocks()
        self.assertEqual(self.decode(self.text(blocks), 0), b"1 13 0\n")
        blocks = [dict(reversed(list(block.items()))) for block in reversed(blocks)]
        self.assertEqual(self.decode(self.text(blocks), 0), b"1 13 0\n")
        self.decode(self.text(blocks) + b"\n", 0)

    def test_schema_worst_case_fits_bound_without_claiming_actual_cli_output(self):
        data = self.text(self.blocks(longest=True))
        # 15 * (Id=3+61+LF, Load=10+11+LF, Active=12+12+LF,
        # Sub=9+26+LF, Main=8+10+LF, Control=11+10+LF, Job=4+10+LF, delimiter1).
        self.assertLessEqual(len(data), 15 * 205)
        self.assertLess(15 * 205, 4096)
        self.assertEqual(self.decode(data, 0), b"4294967295 4294967295 4294967295\n")

    def test_missing_duplicate_unknown_and_foreign_rows_refuse(self):
        original = self.blocks()
        self.decode(self.text(original[:-1]), 90)
        self.decode(self.text(original[:-1] + [original[0]]), 90)
        for key, value in (("Id", "foreign.service"), ("LoadState", "unknown"),
                           ("ActiveState", "refreshing"), ("SubState", "unknown"),
                           ("MainPID", ""), ("ControlPID", "01"), ("Job", "4294967296")):
            blocks = self.blocks()
            blocks[0][key] = value
            self.decode(self.text(blocks), 90)
        blocks = self.blocks()
        del blocks[0]["Job"]
        self.decode(self.text(blocks), 90)
        self.decode(self.text(original).replace(b"Job=\n", b"Job=\nJob=\n", 1), 90)

    def test_truncation_controls_extra_properties_and_byte_overflow_refuse(self):
        data = self.text(self.blocks())
        for invalid in (data[:-1], b"\n" + data, data.replace(b"loaded", b"loaded\x00", 1),
                        data.replace(b"Job=\n", b"Job=\nOther=1\n", 1), data.replace(b"\n", b"\r\n"),
                        b"a" * 4097, data + b"diagnostic\n"):
            self.decode(invalid, 90)


if __name__ == "__main__":
    unittest.main()
