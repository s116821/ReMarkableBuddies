"""Owned publication fence only; no lease, manager command or queued effects."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class FenceFileTests(unittest.TestCase):
    def test_slot_loss_close_without_request_capacity_and_late_publication(self):
        with tempfile.TemporaryDirectory(prefix="e0t-fence-file-") as temp:
            root = Path(temp) / "owned"
            root.mkdir(mode=0o700)
            owner = root / "owner"
            owner.write_text("0123456789abcdef0123456789abcdef")
            owner.chmod(0o600)
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <sys/wait.h>
#include "fence_file.h"
int main(int argc,char **argv) {
    if(argc!=2) return 1;
    int root=open(argv[1],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(root<0) return 2;
    struct e0t_request request;
    struct e0t_request_ledger ledger={.generation=1};
    const char *start=E0T_NONCE " 1 1 B 7\n";
    if(!e0t_request_parse(start,strlen(start),&request)
       || e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_NEW
       || !e0t_request_finish(&ledger,1,E0T_OUTCOME_FAILED_UNKNOWN)) return 3;
    int slot=e0t_request_slot(root);
    if(slot<0 || e0t_fence_close(root)!=-1 || e0t_fence_observe(root)!=0) return 4;
    if(close(slot)) return 5;
    if(e0t_fenced_intent(root,start,strlen(start))!=E0T_INTENT_PUBLISHED) return 6;
    ledger.last_id=96; /* File close is independent of a full model ledger. */
    pid_t child=fork();
    if(child<0) return 7;
    if(child==0) _exit(e0t_fence_close(root)==1 ? 0:8);
    int status;
    if(waitpid(child,&status,0)!=child || !WIFEXITED(status) || WEXITSTATUS(status)) return 9;
    if(e0t_fence_close(root)!=1 || e0t_fence_observe(root)!=1) return 10;
    const char *late=E0T_NONCE " 1 2 B 7\n";
    const char *advance=E0T_NONCE " 1 2 A 2\n";
    if(e0t_fenced_intent(root,late,strlen(late))!=E0T_INTENT_UNKNOWN
       || e0t_fenced_intent(root,advance,strlen(advance))!=E0T_INTENT_UNKNOWN) return 11;
    const char *query=E0T_NONCE " 1 2 J -\n";
    if(e0t_fenced_intent(root,query,strlen(query))!=E0T_INTENT_PUBLISHED) return 12;
    const char *close_frame=E0T_NONCE " 1 3 X -\n";
    if(e0t_fenced_intent(root,close_frame,strlen(close_frame))!=E0T_INTENT_PUBLISHED) return 13;
    if(fchmodat(root,"requests-closed",0644,0)) return 14;
    if(e0t_fence_close(root)!=-1 || e0t_fenced_intent(root,query,strlen(query))!=E0T_INTENT_UNKNOWN) return 15;
    close(root); puts("owned publication fence checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary), str(root)], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, b"owned publication fence checks passed\n")
            self.assertFalse((root / "request-002").read_bytes().endswith(b" B 7\n"))
            self.assertEqual((root / "requests-closed").read_bytes(), owner.read_bytes())


if __name__ == "__main__":
    unittest.main()
