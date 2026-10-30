"""Pure canonical IPC/intent model; no durable files, lease, dispatcher or CLI."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class RequestProtocolTests(unittest.TestCase):
    def test_canonical_frames_unknown_duplicates_and_closed_generation_fence(self):
        with tempfile.TemporaryDirectory(prefix="e0t-request-protocol-") as temp:
            source = Path(temp) / "fixture.c"
            source.write_text(r'''
#include <stdio.h>
#include "request_protocol.h"
int main(void) {
    struct e0t_request request, changed;
    struct e0t_request_ledger ledger={.generation=1};
    const char *valid=E0T_NONCE " 1 1 B 7\n";
    if(!e0t_request_parse(valid,strlen(valid),&request)) return 1;
    if(e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_NEW) return 2;
    if(e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_RECONCILE) return 3;
    struct e0t_request next={1,2,E0T_REQ_ADVANCE,2};
    if(e0t_request_prepare(&ledger,&next)!=E0T_REQUEST_REFUSED) return 4;
    next=(struct e0t_request){1,2,E0T_REQ_JOBS,-1};
    if(e0t_request_prepare(&ledger,&next)!=E0T_REQUEST_NEW) return 5;
    if(!e0t_request_finish(&ledger,2,E0T_OUTCOME_OBSERVED)) return 6;
    if(!e0t_request_finish(&ledger,1,E0T_OUTCOME_FAILED_UNKNOWN)) return 7;
    if(e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_HISTORICAL) return 8;
    changed=request; changed.operand=6;
    if(e0t_request_prepare(&ledger,&changed)!=E0T_REQUEST_REFUSED || !ledger.closed) return 9;
    next=(struct e0t_request){1,3,E0T_REQ_START,7};
    if(e0t_request_prepare(&ledger,&next)!=E0T_REQUEST_REFUSED) return 10;
    next=(struct e0t_request){1,3,E0T_REQ_UNITS,-1};
    if(e0t_request_prepare(&ledger,&next)!=E0T_REQUEST_NEW) return 11;
    ledger=(struct e0t_request_ledger){.generation=1};
    next=(struct e0t_request){1,1,E0T_REQ_ADVANCE,2};
    if(e0t_request_prepare(&ledger,&next)!=E0T_REQUEST_NEW) return 12;
    request=(struct e0t_request){1,2,E0T_REQ_START,7};
    if(e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_REFUSED) return 17;
    request=(struct e0t_request){1,2,E0T_REQ_ADVANCE,2};
    if(e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_REFUSED) return 18;
    if(e0t_request_finish(&ledger,1,E0T_OUTCOME_NO_COMMAND)
       || e0t_request_finish(&ledger,1,E0T_OUTCOME_CLOSED)) return 19;
    request=(struct e0t_request){1,2,E0T_REQ_CLOSE,-1};
    if(e0t_request_prepare(&ledger,&request)!=E0T_REQUEST_NEW || !ledger.closed) return 13;
    if(e0t_request_finish(&ledger,2,E0T_OUTCOME_OBSERVED)
       || !e0t_request_finish(&ledger,2,E0T_OUTCOME_CLOSED)) return 20;
    if(e0t_request_finish(&ledger,1,E0T_OUTCOME_OBSERVED) || ledger.generation!=1) return 14;
    const char *bad[]={E0T_NONCE " 0 1 J -\n", E0T_NONCE " 1 97 J -\n", E0T_NONCE " 1 01 J -\n",
        E0T_NONCE " 1 1 C 4294967295\n", E0T_NONCE " 1 1 B -\n", E0T_NONCE " 1 1 R -\n",
        E0T_NONCE " 1 1 J - \n", E0T_NONCE " 1 1 J 0\n", E0T_NONCE " 1 1 A 9\n",
        E0T_NONCE " 1  1 J -\n", E0T_NONCE " 1 1 J -", E0T_NONCE " 1 1 J -\r\n"};
    for(unsigned i=0;i<sizeof(bad)/sizeof(bad[0]);++i)
        if(e0t_request_parse(bad[i],strlen(bad[i]),&request)) return 15;
    const char *cancel=E0T_NONCE " 8 96 C -\n";
    if(!e0t_request_parse(cancel,strlen(cancel),&request) || request.operand!=-1) return 16;
    puts("pure protocol checks passed"); return 0;
}
''')
            binary = Path(temp) / "fixture"
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            '-DE0T_NONCE="0123456789abcdef0123456789abcdef"',
                            "-I", str(Path(__file__).parent), str(source), "-o", str(binary)], check=True)
            result = subprocess.run([str(binary)], capture_output=True, timeout=2, check=True)
            self.assertEqual(result.stdout, b"pure protocol checks passed\n")


if __name__ == "__main__":
    unittest.main()
