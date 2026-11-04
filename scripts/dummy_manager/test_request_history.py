"""Fixed owned historical records, never current lease or effect replay."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class RequestHistoryTests(unittest.TestCase):
    def test_writer_exit_historical_generation_pending_intent_and_gap_refusal(self):
        with tempfile.TemporaryDirectory(prefix="e0t-history-") as temp:
            root = Path(temp) / "owned"
            root.mkdir(mode=0o700)
            owner = root / "owner"
            owner.write_text("0123456789abcdef0123456789abcdef")
            owner.chmod(0o600)
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <sys/wait.h>
#include "request_history.h"
int main(int argc,char **argv) {
    if(argc!=2) return 1;
    int root=open(argv[1],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(root<0) return 2;
    int slot=openat(root,"request-slot",O_RDWR|O_CREAT|O_EXCL|O_CLOEXEC,0600);
    if(slot<0 || close(slot)) return 3;
    pid_t child=fork();
    if(child<0) return 4;
    if(child==0) {
        const char *frames[]={E0T_NONCE " 1 1 B 7\n",E0T_NONCE " 1 2 A 2\n",
                             E0T_NONCE " 2 3 J -\n",E0T_NONCE " 2 4 B 7\n"};
        for(unsigned i=0;i<4;++i) {
            if(e0t_intent_publish(root,frames[i],strlen(frames[i]))!=E0T_INTENT_PUBLISHED) _exit(5);
            if(i<2 && e0t_completion_finish(root,frames[i],strlen(frames[i]),E0T_OUTCOME_OBSERVED)!=1) _exit(6);
        }
        _exit(0);
    }
    int status;
    if(waitpid(child,&status,0)!=child || !WIFEXITED(status) || WEXITSTATUS(status)) return 7;
    struct e0t_request_ledger history;
    if(!e0t_history_read(root,&history) || history.generation!=2 || history.last_id!=4 || !history.closed
       || history.entries[3].outcome!=E0T_OUTCOME_PENDING) return 8;
    struct e0t_request start={2,4,E0T_REQ_START,7}, next={2,5,E0T_REQ_START,7};
    if(e0t_request_prepare(&history,&start)!=E0T_REQUEST_RECONCILE
       || e0t_request_prepare(&history,&next)!=E0T_REQUEST_REFUSED) return 9;
    slot=openat(root,"request-slot",O_RDWR|O_CLOEXEC);
    if(slot<0 || flock(slot,LOCK_EX|LOCK_NB)) return 10;
    if(e0t_history_read(root,&history) || !history.closed || history.last_id) return 11;
    if(close(slot)) return 12;
    const char *gap=E0T_NONCE " 2 6 J -\n";
    if(e0t_intent_publish(root,gap,strlen(gap))!=E0T_INTENT_PUBLISHED) return 13;
    if(e0t_history_read(root,&history) || !history.closed || history.last_id) return 14;
    if(unlinkat(root,"request-006",0)) return 15;
    int corrupt=openat(root,"request-005",O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC,0600);
    if(corrupt<0 || write(corrupt,"partial",7)!=7 || close(corrupt)) return 16;
    if(e0t_history_read(root,&history) || !history.closed || history.last_id) return 17;
    close(root); puts("fenced historical reconstruction checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary), str(root)], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, b"fenced historical reconstruction checks passed\n")
            self.assertTrue((root / "request-004").read_bytes().endswith(b" B 7\n"))
            self.assertEqual((root / "request-005").read_bytes(), b"partial")


if __name__ == "__main__":
    unittest.main()
