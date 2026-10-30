"""Pure source decoder fixtures: no manager calls or inferred job outcomes."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class JobDecoderTests(unittest.TestCase):
    nonce = "0123456789abcdef0123456789abcdef"

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="e0t-jobs-")
        root = Path(self.temp.name)
        source = root / "decoder.c"
        source.write_text('#include "jobs.h"\nint main(void) { char data[4097]; struct e0t_job rows[12]; size_t n, count; n=fread(data,1,sizeof(data),stdin); if(!e0t_jobs_decode(data,n,rows,&count)) return 90; printf("%zu\\n",count); return 0; }\n')
        self.binary = root / "decoder"
        subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                        '-DE0T_NONCE="' + self.nonce + '"', "-I", str(Path(__file__).parent),
                        str(source), "-o", str(self.binary)], check=True)

    def tearDown(self):
        self.temp.cleanup()

    def row(self, number=123, role="queued", kind="start", state="waiting"):
        return f" {number} buddy-e0t-{self.nonce}-{role}.service {kind} {state}  \n".encode()

    def decode(self, data, expected):
        result = subprocess.run([str(self.binary)], input=data, capture_output=True, timeout=2)
        self.assertEqual(result.returncode, expected, data)
        return result.stdout

    def test_empty_and_exact_owned_rows_are_observations(self):
        self.assertEqual(self.decode(b"", 0), b"0\n")
        self.assertEqual(self.decode(self.row() + self.row(4294967295, "barrier", "stop", "running"), 0), b"2\n")

    def test_ids_and_duplicate_unit_or_id_refuse(self):
        for number in (0, -1, 4294967296, "001", "1x", "12345678901"):
            self.decode(self.row(number), 90)
        self.decode(self.row() + self.row(124), 90)
        self.decode(self.row() + self.row(123, "barrier"), 90)

    def test_foreign_unit_types_states_and_truncation_refuse(self):
        for data in (self.row(role="unknown"), self.row(kind="restart"), self.row(state="done"),
                     self.row().replace(self.nonce.encode(), b"f" * 32), self.row()[:-1],
                     self.row().replace(b".service", b".serv..."), b"\n", self.row() + b"extra\n",
                     self.row().replace(b"waiting", b"waiting extra")):
            self.decode(data, 90)

    def test_controls_colors_and_byte_overflow_refuse(self):
        for data in (b"\x1b[0m" + self.row(), self.row().replace(b"start", b"st\x00art"),
                     self.row().replace(b"start", b"\tstart"), b"a" * 4097):
            self.decode(data, 90)


if __name__ == "__main__":
    unittest.main()
