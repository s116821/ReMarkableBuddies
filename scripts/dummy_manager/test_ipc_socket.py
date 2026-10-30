"""Owned fixed Unix socket binding only; no server, lease or device."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class IpcSocketTests(unittest.TestCase):
    def test_exact_root_endpoint_credentials_permissions_and_stale_path_refusal(self):
        with tempfile.TemporaryDirectory(prefix="e0t-bind-") as temp:
            parent = Path(temp)
            nonce = "0123456789abcdef0123456789abcdef"
            root = parent / ("buddy-e0t-" + nonce)
            other = parent / "other"
            for directory in (root, other):
                directory.mkdir(mode=0o700)
                (directory / "owner").write_text(nonce)
                (directory / "owner").chmod(0o600)
            source = parent / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <stdio.h>
#include "ipc_socket.h"
#include "ipc_receive.h"
static int live(pid_t pid,unsigned long long start) { return pid==getpid() && start==123; }
int main(int argc,char **argv) {
    if(argc!=3) return 1;
    int root=open(argv[1],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    int other=open(argv[2],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(root<0 || other<0) return 2;
    struct stat endpoint={.st_ino=123}, retained;
    if(e0t_ipc_bind(other,&endpoint)!=-1 || endpoint.st_ino!=123) return 3;
    mode_t initial=umask(0022);
    int receiver=e0t_ipc_bind(root,&endpoint);
    mode_t after=umask(0022);
    if(receiver<0 || after!=0022 || (endpoint.st_mode&07777)!=0600
       || !(fcntl(receiver,F_GETFL)&O_NONBLOCK) || !(fcntl(receiver,F_GETFD)&FD_CLOEXEC)) return 4;
    if(e0t_ipc_bind(root,&retained)!=-1 || fstatat(root,"request.sock",&retained,AT_SYMLINK_NOFOLLOW)
       || retained.st_ino!=endpoint.st_ino) return 5;
    int sender=socket(AF_UNIX,SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC,0);
    struct sockaddr_un address={.sun_family=AF_UNIX};
    memcpy(address.sun_path,E0T_REQUEST_SOCKET,sizeof(E0T_REQUEST_SOCKET));
    const char *frame=E0T_NONCE " 1 1 J -\n";
    if(sender<0 || sendto(sender,frame,strlen(frame),MSG_DONTWAIT,(struct sockaddr *)&address,sizeof(address))!=(ssize_t)strlen(frame)) return 6;
    struct e0t_request request; struct e0t_request_ledger ledger={.generation=1};
    if(e0t_ipc_receive(receiver,getpid(),123,live,&request)!=1
       || e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_NEW
       || e0t_intent_publish(root,frame,strlen(frame))!=E0T_INTENT_PUBLISHED
       || !e0t_request_finish(&ledger,1,E0T_OUTCOME_OBSERVED)) return 7;
    close(sender); close(receiver);
    if(e0t_ipc_bind(root,&retained)!=-1) return 8; /* Closing FD is not endpoint removal. */
    if(unlinkat(root,"request.sock",0) || symlinkat("owner",root,"request.sock")) return 9;
    if(e0t_ipc_bind(root,&retained)!=-1) return 10;
    close(root); close(other); umask(initial);
    puts("fixed owned socket checks passed"); return 0;
}
''')
            binary = parent / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="' + nonce + '"', '-DE0T_RUNTIME_PARENT="' + temp + '"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary), str(root), str(other)], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, b"fixed owned socket checks passed\n")
            self.assertTrue((root / "request.sock").is_symlink())
            self.assertFalse((other / "request.sock").exists())


if __name__ == "__main__":
    unittest.main()
