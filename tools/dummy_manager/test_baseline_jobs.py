"""Pure combined observer fixtures; no baseline tuple enters cancellation."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class BaselineJobTests(unittest.TestCase):
    nonce = "0123456789abcdef0123456789abcdef"

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e0t-baseline-jobs-")
        root = Path(self.temp.name)
        source = root / "fixture.c"
        source.write_text(r'''
#include "baseline_jobs.h"
int main(void) {
    char data[4097]; size_t n=fread(data,1,sizeof(data),stdin);
    struct e0t_baseline_name names[2]={{"fixture-device.device"},{"fixture-fsck.service"}};
    struct e0t_combined_jobs observed;
    if(!e0t_combined_decode(data,n,names,2,&observed)) return 90;
    printf("%zu %zu\n",observed.owned_count,observed.baseline_count); return 0;
}
''')
        self.binary = root / "fixture"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                        '-DE0T_NONCE="' + self.nonce + '"', "-I", str(Path(__file__).parent),
                        str(source), "-o", str(self.binary)], check=True)

    def tearDown(self):
        self.temp.cleanup()

    def owned(self, number=10):
        return f"{number} buddy-e0t-{self.nonce}-queued.service start waiting\n".encode()

    def decode(self, data, expected):
        result = subprocess.run([str(self.binary)], input=data, capture_output=True, timeout=2)
        self.assertEqual(result.returncode, expected, data)
        return result.stdout

    def test_empty_and_combined_rows_are_distinct_observations(self):
        self.assertEqual(self.decode(b"", 0), b"0 0\n")
        baseline = b"90 fixture-device.device start running\n91 fixture-fsck.service start waiting\n"
        self.assertEqual(self.decode(self.owned() + baseline, 0), b"1 2\n")
        self.assertEqual(self.decode(baseline + self.owned(), 0), b"1 2\n")
        self.assertEqual(self.decode(b"4294967295 fixture-device.device stop waiting\n", 0), b"0 1\n")

    def test_duplicate_ids_across_classes_or_names_refuse(self):
        for data in (self.owned(90) + b"90 fixture-device.device start running\n",
                     b"90 fixture-device.device start running\n" + self.owned(90),
                     b"90 fixture-device.device start running\n91 fixture-device.device start waiting\n",
                     b"90 fixture-device.device start running\n90 fixture-fsck.service start waiting\n"):
            self.decode(data, 90)

    def test_unknown_control_truncated_or_malformed_baseline_rows_refuse(self):
        valid = b"90 fixture-device.device start running\n"
        for data in (valid.replace(b"fixture-device", b"foreign"), valid[:-1], valid + b"\n",
                     valid.replace(b"90", b"0"), valid.replace(b"90", b"090"),
                     valid.replace(b"90", b"4294967296"), valid.replace(b"start", b"restart"),
                     valid.replace(b"running", b"running extra"), valid.replace(b"start", b"st\x00art"),
                     b"a" * 4097, b"\x1b[0m" + valid):
            self.decode(data, 90)


if __name__ == "__main__":
    unittest.main()
