"""Owned host intent files/process loss only; no CLI, lease or device."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest


class IntentFileTests(unittest.TestCase):
    def test_actual_process_loss_retains_intent_and_refuses_replay_or_bad_ownership(self):
        with tempfile.TemporaryDirectory(prefix="e0t-intent-file-") as temp:
            root = Path(temp) / "owned"
            root.mkdir(mode=0o700)
            owner = root / "owner"
            owner.write_text("0123456789abcdef0123456789abcdef")
            owner.chmod(0o600)
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <sys/wait.h>
#include <sys/syscall.h>
#include <unistd.h>
static int fail_sync_fd=-1;
static int fixture_fsync(int fd) { return fd==fail_sync_fd ? -1 : (int)syscall(SYS_fsync,fd); }
#define fsync fixture_fsync
#include "intent_file.h"
int main(int argc,char **argv) {
    if(argc!=2) return 1;
    int root=open(argv[1],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(root<0) return 2;
    const char *frame=E0T_NONCE " 1 1 B 7\n";
    struct e0t_request request;
    struct e0t_request_ledger ledger={.generation=1};
    if(!e0t_request_parse(frame,strlen(frame),&request)
       || e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_NEW) return 3;
    pid_t child=fork();
    if(child<0) return 4;
    if(child==0) _exit(e0t_intent_publish(root,frame,strlen(frame))==E0T_INTENT_PUBLISHED ? 0:5);
    int status;
    if(waitpid(child,&status,0)!=child || !WIFEXITED(status) || WEXITSTATUS(status)) return 6;
    if(e0t_intent_publish(root,frame,strlen(frame))!=E0T_INTENT_RECONCILE) return 7;
    const char *changed=E0T_NONCE " 1 1 B 6\n";
    if(e0t_intent_publish(root,changed,strlen(changed))!=E0T_INTENT_UNKNOWN) return 8;
    if(!e0t_request_finish(&ledger,1,E0T_OUTCOME_FAILED_UNKNOWN)) return 9;
    const char *second=E0T_NONCE " 1 2 J -\n";
    if(symlinkat("owner",root,"request-002")) return 10;
    if(e0t_intent_publish(root,second,strlen(second))!=E0T_INTENT_UNKNOWN) return 11;
    const char *third=E0T_NONCE " 1 3 J -\n";
    if(linkat(root,"request-001",root,"request-003",0)) return 12;
    if(e0t_intent_publish(root,third,strlen(third))!=E0T_INTENT_UNKNOWN) return 13;
    if(unlinkat(root,"request-003",0)) return 14;
    int fd=openat(root,"request-003",O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC,0600);
    if(fd<0 || write(fd,"partial",7)!=7 || close(fd)) return 15;
    if(e0t_intent_publish(root,third,strlen(third))!=E0T_INTENT_UNKNOWN) return 16;
    if(fchmodat(root,"request-001",0644,0)) return 17;
    if(e0t_intent_publish(root,frame,strlen(frame))!=E0T_INTENT_UNKNOWN) return 18;
    if(fchmodat(root,"owner",0644,0)) return 19;
    const char *fourth=E0T_NONCE " 1 4 J -\n";
    if(e0t_intent_publish(root,fourth,strlen(fourth))!=E0T_INTENT_UNKNOWN) return 20;
    if(faccessat(root,"request-004",F_OK,0)==0) return 21;
    if(fchmodat(root,"owner",0600,0)) return 22;
    fail_sync_fd=root;
    if(e0t_intent_publish(root,fourth,strlen(fourth))!=E0T_INTENT_UNKNOWN) return 23;
    fail_sync_fd=-1;
    if(e0t_intent_publish(root,fourth,strlen(fourth))!=E0T_INTENT_RECONCILE) return 24;
    close(root); puts("owned process-loss intent checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary), str(root)], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, b"owned process-loss intent checks passed\n")
            self.assertEqual((root / "request-001").read_bytes(),
                             b"0123456789abcdef0123456789abcdef 1 1 B 7\n")
            self.assertEqual((root / "request-003").read_bytes(), b"partial")
            self.assertTrue((root / "request-002").is_symlink())
            self.assertEqual(os.stat(owner).st_nlink, 1)


if __name__ == "__main__":
    unittest.main()
