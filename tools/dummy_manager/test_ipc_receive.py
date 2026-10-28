"""Real owned Unix datagrams/credentials only; no server, ACK or effects."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class IpcReceiveTests(unittest.TestCase):
    def test_nonblocking_credentials_bounds_and_rejected_descriptor_cleanup(self):
        with tempfile.TemporaryDirectory(prefix="e0t-ipc-") as temp:
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <stdio.h>
#include "ipc_receive.h"
static int checks=0, fail_after=0;
/* Host-only identity callback; no /proc or device identity qualification. */
static int live(pid_t pid,unsigned long long start) {
    ++checks; return pid>1 && start==123 && (!fail_after || checks<fail_after);
}
static int fd_count(void) { int total=0; for(int fd=0;fd<128;++fd) if(fcntl(fd,F_GETFD)>=0) ++total; return total; }
int main(void) {
    int sockets[2], one=1;
    if(socketpair(AF_UNIX,SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC,0,sockets)
       || setsockopt(sockets[0],SOL_SOCKET,SO_PASSCRED,&one,sizeof(one))) return 1;
    struct e0t_request output={8,96,E0T_REQ_CLOSE,-1};
    if(e0t_ipc_receive(sockets[0],getpid(),123,live,&output)!=0 || output.id!=96) return 2;
    const char *frame=E0T_NONCE " 1 1 J -\n";
    if(send(sockets[1],frame,strlen(frame),MSG_DONTWAIT)!=(ssize_t)strlen(frame)
       || e0t_ipc_receive(sockets[0],getpid(),123,live,&output)!=1 || output.id!=1) return 3;
    struct e0t_request_ledger ledger={.generation=1};
    if(e0t_request_prepare(&ledger,&output)!=E0T_REQUEST_NEW
       || !e0t_request_finish(&ledger,1,E0T_OUTCOME_OBSERVED)) return 4;
    output.id=96;
    if(send(sockets[1],frame,strlen(frame),MSG_DONTWAIT)!=(ssize_t)strlen(frame)
       || e0t_ipc_receive(sockets[0],getpid()+1,123,live,&output)!=-1 || output.id!=96) return 5;
    char oversized[4096]; memset(oversized,'x',sizeof(oversized));
    if(send(sockets[1],oversized,sizeof(oversized),MSG_DONTWAIT)!=(ssize_t)sizeof(oversized)
       || e0t_ipc_receive(sockets[0],getpid(),123,live,&output)!=-1) return 6;
    checks=0; fail_after=2;
    if(send(sockets[1],frame,strlen(frame),MSG_DONTWAIT)!=(ssize_t)strlen(frame)
       || e0t_ipc_receive(sockets[0],getpid(),123,live,&output)!=-1 || output.id!=96) return 7;
    fail_after=0;
    int passed=open("/dev/null",O_RDONLY|O_CLOEXEC); if(passed<0) return 8;
    union { struct cmsghdr alignment; unsigned char bytes[CMSG_SPACE(20*sizeof(int))]; } ancillary;
    memset(&ancillary,0,sizeof(ancillary));
    struct iovec payload={(void *)frame,strlen(frame)};
    struct msghdr message={0}; message.msg_iov=&payload; message.msg_iovlen=1;
    message.msg_control=ancillary.bytes; message.msg_controllen=sizeof(ancillary.bytes);
    struct cmsghdr *rights=CMSG_FIRSTHDR(&message);
    rights->cmsg_level=SOL_SOCKET; rights->cmsg_type=SCM_RIGHTS; rights->cmsg_len=CMSG_LEN(3*sizeof(int));
    int descriptors[20]; for(unsigned i=0;i<20;++i) descriptors[i]=passed;
    int before=fd_count();
    for(unsigned i=0;i<32;++i) {
        unsigned number=i<16 ? 3:20;
        rights->cmsg_len=CMSG_LEN(number*sizeof(int));
        message.msg_controllen=CMSG_SPACE(number*sizeof(int));
        memcpy(CMSG_DATA(rights),descriptors,number*sizeof(int));
        if(sendmsg(sockets[1],&message,MSG_DONTWAIT)!=(ssize_t)strlen(frame)
           || e0t_ipc_receive(sockets[0],getpid(),123,live,&output)!=-1
           || fd_count()!=before || output.id!=96) return 9;
    }
    int flags=fcntl(sockets[0],F_GETFL);
    if(flags<0 || fcntl(sockets[0],F_SETFL,flags&~O_NONBLOCK)
       || e0t_ipc_receive(sockets[0],getpid(),123,live,&output)!=-1) return 10;
    close(passed); close(sockets[0]); close(sockets[1]);
    puts("bounded credential datagram checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, timeout=3, check=True)
            self.assertEqual(result.stdout, b"bounded credential datagram checks passed\n")


if __name__ == "__main__":
    unittest.main()
