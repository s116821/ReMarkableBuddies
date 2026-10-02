"""Pure batch encoder, never invokes cancel/systemctl or establishes ownership."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class CancelEncoderTests(unittest.TestCase):
    def test_empty_maximum_and_invalid_source_tuples(self):
        with tempfile.TemporaryDirectory(prefix="e0t-cancel-args-") as temp:
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#include "cancel_args.h"
int main(void) {
    struct e0t_job rows[13] = {{0}}; char arguments[12][11]; size_t count;
    if (!e0t_jobs_decode("", 0, rows, &count) || count) return 1;
    if (e0t_cancel_encode(NULL, 0, arguments) != 0) return 2;
    for (unsigned i=0; i<12; ++i) rows[i]=(struct e0t_job){i+1,i,0,0};
    rows[11].id=UINT32_MAX;
    if(e0t_cancel_encode(rows,12,arguments)!=1 || strcmp(arguments[0],"1") || strcmp(arguments[11],"4294967295")) return 3;
    if(e0t_cancel_encode(rows,13,arguments)!=-1 || e0t_cancel_encode(NULL,1,arguments)!=-1) return 4;
    rows[1].id=rows[0].id; if(e0t_cancel_encode(rows,12,arguments)!=-1) return 5;
    rows[1].id=2; rows[1].role=0; if(e0t_cancel_encode(rows,12,arguments)!=-1) return 6;
    rows[1].role=12; if(e0t_cancel_encode(rows,12,arguments)!=-1) return 7;
    rows[1].role=1; rows[1].id=0; if(e0t_cancel_encode(rows,12,arguments)!=-1) return 8;
    rows[1].id=2; rows[1].stop=2; if(e0t_cancel_encode(rows,12,arguments)!=-1) return 9;
    rows[1].stop=0; rows[1].running=2; if(e0t_cancel_encode(rows,12,arguments)!=-1) return 10;
    puts("pure encoder checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, timeout=2, check=True)
            self.assertEqual(result.stdout, b"pure encoder checks passed\n")


if __name__ == "__main__":
    unittest.main()
