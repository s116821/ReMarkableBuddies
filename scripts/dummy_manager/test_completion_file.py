"""Owned historical outcome files; no observed manager result or replay."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class CompletionFileTests(unittest.TestCase):
    def test_actual_completion_process_exit_historical_duplicate_and_partial_refusal(self):
        with tempfile.TemporaryDirectory(prefix="e0t-completion-file-") as temp:
            root = Path(temp) / "owned"
            root.mkdir(mode=0o700)
            owner = root / "owner"
            owner.write_text("0123456789abcdef0123456789abcdef")
            owner.chmod(0o600)
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <sys/wait.h>
#include "completion_file.h"
int main(int argc,char **argv) {
    if(argc!=2) return 1;
    int root=open(argv[1],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(root<0) return 2;
    struct e0t_request request;
    struct e0t_request_ledger ledger={.generation=1};
    const char *frame=E0T_NONCE " 1 1 B 7\n";
    if(!e0t_request_parse(frame,strlen(frame),&request)
       || e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_NEW
       || e0t_intent_publish(root,frame,strlen(frame))!=E0T_INTENT_PUBLISHED) return 3;
    if(e0t_completion_finish(root,frame,strlen(frame),E0T_OUTCOME_NO_COMMAND)!=-1) return 4;
    pid_t child=fork();
    if(child<0) return 5;
    if(child==0) _exit(e0t_completion_finish(root,frame,strlen(frame),E0T_OUTCOME_OBSERVED)==1 ? 0:6);
    int status;
    if(waitpid(child,&status,0)!=child || !WIFEXITED(status) || WEXITSTATUS(status)) return 7;
    if(e0t_completion_finish(root,frame,strlen(frame),E0T_OUTCOME_OBSERVED)!=0
       || e0t_completion_finish(root,frame,strlen(frame),E0T_OUTCOME_FAILED_UNKNOWN)!=-1) return 8;
    if(!e0t_request_finish(&ledger,1,E0T_OUTCOME_OBSERVED)
       || e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_HISTORICAL) return 9;
    const char *partial=E0T_NONCE " 1 2 C -\n";
    if(e0t_intent_publish(root,partial,strlen(partial))!=E0T_INTENT_PUBLISHED) return 10;
    int fd=openat(root,"request-002",O_WRONLY|O_APPEND|O_CLOEXEC);
    if(fd<0 || write(fd,"unk",3)!=3 || close(fd)) return 11;
    if(e0t_completion_finish(root,partial,strlen(partial),E0T_OUTCOME_FAILED_UNKNOWN)!=-1) return 12;
    const char *closed=E0T_NONCE " 1 3 X -\n";
    if(e0t_intent_publish(root,closed,strlen(closed))!=E0T_INTENT_PUBLISHED) return 13;
    if(e0t_completion_finish(root,closed,strlen(closed),E0T_OUTCOME_OBSERVED)!=-1
       || e0t_completion_finish(root,closed,strlen(closed),E0T_OUTCOME_CLOSED)!=1) return 14;
    close(root); puts("historical outcome checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary), str(root)], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, b"historical outcome checks passed\n")
            self.assertTrue((root / "request-001").read_bytes().endswith(b"\nobserved\n"))
            self.assertTrue((root / "request-002").read_bytes().endswith(b"\nunk"))
            self.assertLessEqual((root / "request-001").stat().st_size, 64)


if __name__ == "__main__":
    unittest.main()
