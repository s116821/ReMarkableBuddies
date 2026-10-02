"""Pure prospective argv fixtures. No process dispatch or manager exists."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class CommandArgumentTests(unittest.TestCase):
    def test_fixed_targets_empty_cancel_and_maximum_argv(self):
        with tempfile.TemporaryDirectory(prefix="e0t-command-args-") as temp:
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#include "command_args.h"
int main(void) {
    struct { unsigned before; struct e0t_command_arguments args; unsigned after; } box;
    box.before=123; box.after=456;
    struct e0t_command_arguments *a=&box.args;
    struct e0t_baseline_name baseline[2]={{"fixture-device.device"},{"fixture-fsck.service"}};
    struct e0t_job jobs[12];
    for(unsigned i=0;i<12;++i) jobs[i]=(struct e0t_job){i+1,i,0,0};
    if(e0t_command_encode(E0T_CANCEL_BATCH,0,NULL,0,NULL,0,a)!=0 || a->argc || a->argv[0]) return 1;
    if(e0t_command_encode(E0T_CANCEL_BATCH,0,jobs,12,NULL,0,a)!=1 || a->argc!=16 || strcmp(a->argv[3],"cancel") || strcmp(a->argv[15],"12") || a->argv[16]) return 2;
    if(e0t_command_encode(E0T_STOP_CASES,0,NULL,0,NULL,0,a)!=1 || a->argc!=16 || strcmp(a->argv[4],"stop")) return 3;
    for(size_t i=5;i<a->argc;++i) if(strstr(a->argv[i],"-cleanup.service") || !strstr(a->argv[i],"buddy-e0t-")) return 4;
    if(e0t_command_encode(E0T_STOP_CLEANUP,0,NULL,0,NULL,0,a)!=1 || a->argc!=6 || !strstr(a->argv[5],"-cleanup.service")) return 5;
    if(e0t_command_encode(E0T_START_OWNED,11,NULL,0,NULL,0,a)!=1 || a->argc!=7 || strcmp(a->argv[4],"--job-mode=fail") || !strstr(a->argv[6],"-queued.service")) return 6;
    if(e0t_command_encode(E0T_START_OWNED,12,NULL,0,NULL,0,a)!=-1) return 7;
    if(e0t_command_encode(E0T_UNIT_STATES,0,NULL,0,NULL,0,a)!=1 || a->argc!=28 || strcmp(a->argv[6],"--property=Id") || strcmp(a->argv[25],"xochitl.service") || strcmp(a->argv[27],"rm-sync.service") || a->argv[28]) return 8;
    if(e0t_command_encode(E0T_OBSERVED_JOBS,0,NULL,0,baseline,2,a)!=1 || a->argc!=21 || strcmp(a->argv[19],baseline[0].unit) || strcmp(a->argv[20],baseline[1].unit)) return 9;
    if(e0t_command_encode(E0T_OWNED_JOBS,0,NULL,0,NULL,0,a)!=1 || a->argc!=19) return 10;
    if(e0t_command_encode(E0T_RELOAD,0,NULL,0,NULL,0,a)!=1 || a->argc!=4 || strcmp(a->argv[3],"daemon-reload")) return 11;
    if(e0t_command_encode(E0T_VERSION,0,NULL,0,NULL,0,a)!=1 || a->argc!=2 || strcmp(a->argv[1],"--version")) return 12;
    if(e0t_command_encode(E0T_CANCEL_BATCH,0,jobs,13,NULL,0,a)!=-1) return 13;
    if(e0t_command_encode(E0T_STOP_CASES,0,NULL,0,baseline,2,a)!=-1) return 14;
    if(e0t_command_encode(E0T_OWNED_JOBS,0,jobs,1,NULL,0,a)!=-1) return 15;
    if(e0t_command_encode((enum e0t_command_kind)99,0,NULL,0,NULL,0,a)!=-1) return 16;
    if(box.before!=123 || box.after!=456) return 17;
    puts("pure fixed argument checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, timeout=2, check=True)
            self.assertEqual(result.stdout, b"pure fixed argument checks passed\n")


if __name__ == "__main__":
    unittest.main()
