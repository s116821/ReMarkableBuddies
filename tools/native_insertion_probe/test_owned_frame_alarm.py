"""Real private-launcher boundary controls; disposable root Linux only.
Supply exact reviewed source paths through REM25_ALARM_SOURCE/REM25_ALARM_R3.
No private source, tablet, Qt, input or service is distributed by this fixture.
"""
from pathlib import Path
import os
import signal
import subprocess
import tempfile
import unittest
import json
import types
import time
from unittest.mock import patch
import test_creation_publisher as publisher_tests

HERE = Path(__file__).resolve().parent

class AlarmTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if os.name != "posix" or os.getuid()!=0 or not os.environ.get("REM25_ALARM_SOURCE"):
            raise unittest.SkipTest("Disposable root Linux and exact private launcher sources required")
        cls.work=tempfile.TemporaryDirectory(prefix="alarm-boundary-")
        cls.r4=Path(cls.work.name)/"r4"
        cls.r3=Path(cls.work.name)/"r3"
        cls.child=Path(cls.work.name)/"child"
        for source,output in ((os.environ["REM25_ALARM_SOURCE"],cls.r4),
                              (os.environ["REM25_ALARM_R3"],cls.r3),
                              (HERE/"alarm_child_fixture.c",cls.child)):
            subprocess.run(["gcc","-std=c11","-Wall","-Wextra","-Werror",str(source),"-o",str(output)],
                           check=True,capture_output=True)
        publisher_tests.PublisherTests.setUpClass()
    @classmethod
    def tearDownClass(cls):
        publisher_tests.PublisherTests.tearDownClass()
        cls.work.cleanup()
    def execute(self,args,env=None,wrapper=None,inherit_alarm=False):
        def inherited_alarm_state():
            signal.signal(signal.SIGALRM,signal.SIG_IGN)
            signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGALRM})
        process=subprocess.Popen([str(wrapper or self.r4),*map(str,args)],stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE,env=os.environ | (env or {}),
                                 preexec_fn=inherited_alarm_state if inherit_alarm else None)
        stdout,stderr=process.communicate(timeout=7)
        return process.pid,process.returncode,stdout,stderr
    def test_same_pid_exact_arguments_alarm_disposition_and_modes(self):
        for mode,args,seconds in (
            ("v2",["--publish",self.child,*["a"]*8,"open-document-insertion-v2"],3),
            ("legacy",["--publish",self.child,*["a"]*8],3),
            ("converter",[self.child,*["a"]*7],5),
            ("observer",["--observe",self.child,"2"],3),
        ):
            with self.subTest(mode=mode):
                pid,exit_code,stdout,stderr=self.execute(args,inherit_alarm=True)
                lines=stdout.decode().splitlines()
                actual_pid,argc,remaining,default,blocked=map(int,lines[0].split())
                self.assertEqual((exit_code,stderr),(0,b""))
                self.assertEqual((actual_pid,default,blocked),(pid,1,0))
                self.assertGreaterEqual(remaining,seconds-1);self.assertLessEqual(remaining,seconds)
                selected=args[1:] if args[0] in ("--publish","--observe") else args
                self.assertEqual(argc,len(selected));self.assertEqual(lines[1:],list(map(str,selected)))
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/"file";path.write_text("fixed")
            _,code,out,err=self.execute(["--hashes","/usr/bin/sha256sum",path])
            self.assertEqual((code,err),(0,b""))
            manifest=Path(root)/"manifest";manifest.write_bytes(out)
            _,code,_,err=self.execute(["--check","/usr/bin/sha256sum","-c",manifest])
            self.assertEqual((code,err),(0,b""))
    def test_invalid_grammar_no_child_and_r3_v2_regression(self):
        v2=["--publish",self.child,*["a"]*8,"open-document-insertion-v2"]
        invalid=(v2[:-1]+["unknown"],v2+["extra"],v2[:-2],
                 ["--publish","relative",*["a"]*8,"open-document-insertion-v2"],
                 ["--other",self.child,*["a"]*8],["--observe",self.child,"3"])
        for args in invalid:
            with self.subTest(args=args):
                _,code,out,err=self.execute(args)
                self.assertEqual((code,out,err),(2,b"",b"Invalid own-alarm launcher arguments\n"))
        _,code,out,err=self.execute(v2,wrapper=self.r3)
        self.assertEqual((code,out,err),(2,b"",b"Invalid own-alarm launcher arguments\n"))
    def test_exec_failure_exit_propagation_and_original_alarm_termination(self):
        _,code,out,err=self.execute(["--publish","/missing-reviewed-child",*["a"]*8])
        self.assertEqual((code,out),(127,b""));self.assertIn(b"Execute selected tool",err)
        _,code,_,_=self.execute(["--publish",self.child,*["a"]*8],{"ALARM_CHILD_EXIT":"17"})
        self.assertEqual(code,17)
        for args in (["--publish",self.child,*["a"]*8,"open-document-insertion-v2"],
                     [self.child,*["a"]*7]):
            _,code,_,_=self.execute(args,{"ALARM_CHILD_WAIT":"1"})
            self.assertEqual(code,-signal.SIGALRM)
    def test_rendered_host_invocation_executes_real_publisher_once(self):
        # Import-only PIL stub enables the host renderer in the isolated C image.
        # No image operations are used in this boundary test.
        import sys
        if "PIL" not in sys.modules:
            pil=types.ModuleType("PIL");pil.Image=None;pil.ImageDraw=None;sys.modules["PIL"]=pil
        from open_document_transport import SSHAdapter
        from open_document_timing import OpenDocumentTiming,PRODUCTION_V2
        for v2 in (False,True):
            for fault in (None,"identity","expiry","closure","uncertain"):
                with self.subTest(v2=v2,fault=fault):
                    f=publisher_tests.PublisherTests();f.setUp()
                    try:
                        if v2:f.select_v2()
                        if fault=="identity":f.args[2]=str(int(f.args[2])+1)
                        if fault=="expiry":
                            f.args[3]="1"
                            f.put("creation-waiting",f"{f.nonce} {f.args[1]} {f.args[2]} waiting 1\n".encode())
                        if fault=="closure":f.put("stop.request",b"closed")
                        # Renderer guards have their own tests; capture its actual
                        # final wrapper invocation and execute it with real files.
                        captured=[]
                        adapter=SSHAdapter([],{"remote_root":str(f.root),"files":{},
                            "library_tap_variant":"primary-1641396"},
                            {"nonce":f.nonce,"boot_id":"boot","document_id":f.doc,
                             "experiment_kind":PRODUCTION_V2 if v2 else "open-document-insertion-v1",
                             "library_tap_variant":"primary-1641396"},f.root,lambda *a:None,b"")
                        adapter.prelude=lambda *a:""
                        adapter.production_closure_checks=lambda:""
                        adapter.action_budget=lambda *a,**k:(12,time.monotonic()+100)
                        adapter.call=lambda script,timeout=3:captured.append(script) or '{"published":true,"retry_permitted":false}'
                        adapter.tap_released=True
                        adapter.tap_release_sha256=adapter.tap_supervisor_sha256="a"*64
                        adapter.receipt["files"]={name:"a"*64 for name in ("runtime.files","document-immutable.files")}
                        waiting=types.SimpleNamespace(nonce=f.nonce,pid=int(f.args[1]),start_ticks=int(f.args[2]),
                                                      expiry_ms=int(f.args[3]))
                        timing=OpenDocumentTiming(*map(int,f.args[4:7]),PRODUCTION_V2 if v2 else "open-document-insertion-v1")
                        adapter.arm_once(waiting,timing,{ext+"_raw_sha256":"a"*64 for ext in ("content","metadata")})
                        line=next(line for line in captured[0].splitlines() if line.startswith('"$root/owned_frame_alarm" --publish'))
                        import shutil
                        shutil.copyfile(self.r4,f.root/"owned_frame_alarm");(f.root/"owned_frame_alarm").chmod(0o700)
                        selected=publisher_tests.PublisherTests.fixture if fault=="uncertain" else publisher_tests.PublisherTests.binary
                        shutil.copyfile(selected,f.root/"publish_creation_arm");(f.root/"publish_creation_arm").chmod(0o700)
                        if v2 and fault is None:
                            shutil.copyfile(self.r3,f.root/"owned_frame_alarm")
                            old=subprocess.run(["sh","-c",'set -eu\nroot='+str(f.root)+'\n'+line],
                                               capture_output=True,timeout=4)
                            self.assertEqual(old.returncode,2)
                            self.assertEqual((f.root/"host-publication.stderr").read_bytes(),
                                             b"Invalid own-alarm launcher arguments\n")
                            self.assertEqual((f.root/"host-publication.json").read_bytes(),b"")
                            self.assertFalse((f.root/"creation-arm").exists())
                            shutil.copyfile(self.r4,f.root/"owned_frame_alarm")
                        env=os.environ | ({"TEST_RECEIPT_FAIL":"1"} if fault=="uncertain" else {})
                        result=subprocess.run(["sh","-c",'set -eu\nroot='+str(f.root)+'\n'+line],env=env,
                                              capture_output=True,timeout=4)
                        arm=f.root/"creation-arm"
                        if fault is None:
                            self.assertEqual(result.returncode,0,result.stderr)
                            self.assertEqual(arm.read_bytes(),f.token)
                            output=json.loads((f.root/"host-publication.json").read_bytes())
                            self.assertTrue(output["published"]);self.assertFalse(output["retry_permitted"])
                            again=subprocess.run(["sh","-c",'set -eu\nroot='+str(f.root)+'\n'+line],
                                                 capture_output=True,timeout=4)
                            self.assertNotEqual(again.returncode,0);self.assertEqual(arm.read_bytes(),f.token)
                        elif fault=="uncertain":
                            self.assertEqual(result.returncode,91);self.assertTrue(arm.exists())
                        else:
                            self.assertEqual(result.returncode,90);self.assertFalse(arm.exists())
                        self.assertFalse((f.root/"creation-arm.tmp").exists())
                    finally:f.tearDown()
