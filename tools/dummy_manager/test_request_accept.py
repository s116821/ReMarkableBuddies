"""Durable pending/model cooperation only; never command dispatch."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class RequestAcceptTests(unittest.TestCase):
    def test_duplicate_disk_consistency_reserved_capacity_and_independent_close(self):
        with tempfile.TemporaryDirectory(prefix="e0t-accept-") as temp:
            roots = [Path(temp) / name for name in ("first", "capacity", "overrun")]
            for root in roots:
                root.mkdir(mode=0o700)
                (root / "owner").write_text("0123456789abcdef0123456789abcdef")
                (root / "owner").chmod(0o600)
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#define _GNU_SOURCE
#include <stdio.h>
#include "request_accept.h"
static int complete(int root,struct e0t_request_ledger *ledger,const char *frame,unsigned id,
                    enum e0t_request_outcome outcome) {
    return e0t_completion_finish(root,frame,strlen(frame),outcome)==1 && e0t_request_finish(ledger,id,outcome);
}
int main(int argc,char **argv) {
    if(argc!=4) return 1;
    int first=open(argv[1],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    int capacity=open(argv[2],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    int overrun=open(argv[3],O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(first<0 || capacity<0 || overrun<0) return 2;
    struct e0t_request_ledger ledger={.generation=1};
    const char *start=E0T_NONCE " 1 1 B 7\n";
    if(e0t_request_accept(first,&ledger,start,strlen(start))!=E0T_REQUEST_NEW
       || e0t_request_accept(first,&ledger,start,strlen(start))!=E0T_REQUEST_RECONCILE) return 3;
    if(!complete(first,&ledger,start,1,E0T_OUTCOME_OBSERVED)
       || e0t_request_accept(first,&ledger,start,strlen(start))!=E0T_REQUEST_HISTORICAL) return 4;
    int corrupt=openat(first,"request-001",O_WRONLY|O_TRUNC|O_CLOEXEC);
    if(corrupt<0 || write(corrupt,"partial",7)!=7 || close(corrupt)) return 5;
    if(e0t_request_accept(first,&ledger,start,strlen(start))!=E0T_REQUEST_REFUSED
       || !ledger.closed || e0t_fence_observe(first)!=1) return 6;
    struct e0t_request_ledger full={.generation=1}, extra={.generation=1};
    char frame[64];
    for(unsigned id=1;id<=83;++id) {
        snprintf(frame,sizeof(frame),E0T_NONCE " 1 %u J -\n",id);
        if(e0t_request_accept(capacity,&full,frame,strlen(frame))!=E0T_REQUEST_NEW
           || !complete(capacity,&full,frame,id,E0T_OUTCOME_OBSERVED)
           || e0t_request_accept(overrun,&extra,frame,strlen(frame))!=E0T_REQUEST_NEW
           || !complete(overrun,&extra,frame,id,E0T_OUTCOME_OBSERVED)) return 7;
    }
    const char *close_frame=E0T_NONCE " 1 84 X -\n";
    if(e0t_request_accept(capacity,&full,close_frame,strlen(close_frame))!=E0T_REQUEST_NEW
       || !complete(capacity,&full,close_frame,84,E0T_OUTCOME_CLOSED)
       || !full.closed || e0t_fence_observe(capacity)!=1) return 8;
    for(unsigned id=85;id<=96;++id) {
        snprintf(frame,sizeof(frame),E0T_NONCE " 1 %u J -\n",id);
        if(e0t_request_accept(capacity,&full,frame,strlen(frame))!=E0T_REQUEST_REFUSED
           || e0t_cleanup_accept(capacity,&full,frame,strlen(frame))!=E0T_REQUEST_NEW
           || !complete(capacity,&full,frame,id,E0T_OUTCOME_OBSERVED)) return 9;
    }
    if(full.last_id!=96 || e0t_fence_close(capacity)!=1) return 10;
    const char *overflow=E0T_NONCE " 1 84 J -\n";
    if(e0t_request_accept(overrun,&extra,overflow,strlen(overflow))!=E0T_REQUEST_REFUSED
       || extra.last_id!=83 || !extra.closed || e0t_fence_observe(overrun)!=1
       || faccessat(overrun,"request-084",F_OK,0)==0) return 11;
    if(e0t_request_accept(overrun,&extra,overflow,strlen(overflow))!=E0T_REQUEST_REFUSED
       || e0t_cleanup_accept(overrun,&extra,overflow,strlen(overflow))!=E0T_REQUEST_NEW) return 12;
    close(first); close(capacity); close(overrun);
    puts("durable pending acceptance checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary), *(str(root) for root in roots)], capture_output=True, timeout=5, check=True)
            self.assertEqual(result.stdout, b"durable pending acceptance checks passed\n")
            self.assertEqual(len(list(roots[1].glob("request-[0-9][0-9][0-9]"))), 96)
            self.assertEqual((roots[0] / "request-001").read_bytes(), b"partial")


if __name__ == "__main__":
    unittest.main()
