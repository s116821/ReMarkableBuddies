"""Exact shell cap and recovery fixtures; synthetic clients, no service manager."""
import os
import hashlib
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent

@unittest.skipUnless(os.name == 'posix' and Path('/bin/bash').exists(), 'Linux Bash fixture required')
class Source5Control(unittest.TestCase):
    def source(self, name, root):
        boundary = '# Main-only execution boundary' if name == 'actor.sh.in' else '# Fixture source boundary'
        return (HERE/name).read_text().split(boundary, 1)[0].replace('@ROOT@',str(root)).replace('@NONCE@','1'*32).replace('@CONTROL_POLICY@','source5-child-fsize4096-v1').replace('@CONTROL_SHELL_SHA256@','0'*64).replace('@CAPTURESPACE@',':')

    def run_shell(self, name, root, body, overrides=''):
        # The actual tablet shell identity is a separate hardware pin. This
        # fixture tests its assessed POSIX Bash limit behavior, not that pin.
        return subprocess.run(['/bin/bash','--posix','-c',self.source(name,root)+'\ncontrol_shell() { :; }\n'+overrides+'\n'+body],capture_output=True,text=True,timeout=8)

    def test_both_stream_boundaries_and_same_pid_exec(self):
        for name,prefix in [('actor.sh.in','query'),('arm.sh.in','host-query')]:
            for stream in ('stdout','stderr'):
                for size in (4095,4096,4097):
                    with self.subTest(name=name,stream=stream,size=size), tempfile.TemporaryDirectory() as d:
                        root=Path(d)
                        producer=root/'producer.py'
                        producer.write_text('import os,sys\nfrom pathlib import Path\nPath(sys.argv[1]).write_text(str(os.getpid()))\ngetattr(sys,"'+stream+'").write("x"*'+str(size)+')\n')
                        body=f'''control_outputs {prefix}
status=0
(control_child {prefix} || exit 90; exec /usr/bin/python3 {shlex.quote(str(producer))} {shlex.quote(str(root/'pid'))}) > "$root/{prefix}.stdout" 2> "$root/{prefix}.stderr" &
expected=$!
wait "$expected" || status=$?
test "$(cat "$root/pid")" = "$expected"
printf '%s' "$status" > "$root/status"
test "$status" = 0 || exit 90
control_complete {prefix}
'''
                        result=self.run_shell(name,root,body)
                        self.assertEqual(result.returncode == 0,size < 4096,result.stderr)
                        self.assertLessEqual((root/(prefix+'.'+stream)).stat().st_size,4096)
                        if size == 4096:
                            self.assertEqual((root/'status').read_text(),'0')

    def test_child_limits_do_not_change_parent(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            result=self.run_shell('actor.sh.in',root,'''cat /proc/self/limits > "$root/before"
(control_child query || exit 90; cat /proc/self/limits) > "$root/query.stdout" 2> "$root/query.stderr"
cat /proc/self/limits > "$root/after"
control_complete query
''')
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual((root/'before').read_bytes(),(root/'after').read_bytes())
            lines=(root/'query.stdout').read_text().splitlines()
            self.assertTrue(any(line.split()==['Max','file','size','4096','4096','bytes'] for line in lines))
            self.assertTrue(any(line.split()==['Max','core','file','size','0','0','bytes'] for line in lines))

    def test_setup_failure_prevents_exec(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            result=self.run_shell('actor.sh.in',root,'(control_child query || exit 90; printf issued > "$root/issued") > "$root/query.stdout" 2> "$root/query.stderr"','control_shell() { return 90; }')
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((root/'issued').exists())

    def test_held_fd_mismatch_and_linked_output_refuse(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            for name in ('query.stdout','query.stderr'):
                (root/name).write_bytes(b''); (root/name).chmod(0o600)
            result=self.run_shell('actor.sh.in',root,'(control_child query || exit 90) > "$root/other" 2> "$root/query.stderr"')
            self.assertNotEqual(result.returncode,0)
            os.link(root/'query.stdout',root/'alias')
            result=self.run_shell('actor.sh.in',root,'control_outputs query')
            self.assertNotEqual(result.returncode,0)

    def test_diagnostic_failure_does_not_skip_guarded_stop_or_stock_restore(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            overrides='''reserve() { return 0; }
policy() { :; }
cgroup_gone() { :; }
process_gone() { :; }
recover_owned_input() { :; }
recovery_hashes() { :; }
record_document_preservation() { :; }
value() { VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'; }
query() { case "$1" in show) return 90;; *) :;; esac; }
stop_unit() { printf stopped > "$root/stopped"; }
verify_stock() { printf restored > "$root/stock.restored"; }
'''
            result=self.run_shell('actor.sh.in',root,'stock_stopped=yes; candidate_started=yes; restore',overrides)
            self.assertNotEqual(result.returncode,0)
            self.assertTrue((root/'stopped').exists())
            self.assertTrue((root/'stock-start.claim').exists())
            self.assertTrue((root/'stock.restored').exists())
            self.assertTrue((root/'insertion.failed').exists())

    def test_critical_policy_failure_remains_unresolved(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            result=self.run_shell('actor.sh.in',root,'stock_stopped=yes; candidate_started=yes; restore','recover_owned_input() { :; }; policy() { return 90; }; stop_unit() { printf stopped > "$root/stopped"; }')
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((root/'stopped').exists())
            self.assertFalse((root/'stock-start.claim').exists())

    def test_property_response_is_complete_unique_and_bounded(self):
        for text,accepted in [('MainPID=4253\nJob=\n',True),('MainPID=4253\n',False),
            ('MainPID=4253\nMainPID=4253\nJob=\n',False),('MainPID=4253\nJob=\nExtra=1\n',False),
            ('MainPID='+'9'*65+'\nJob=\n',False),('MainPID=4253\r\nJob=\n',False)]:
            with self.subTest(text=text),tempfile.TemporaryDirectory() as d:
                root=Path(d);(root/'query.stdout').write_text(text)
                result=self.run_shell('actor.sh.in',root,'control_grammar query show --property=MainPID,Job xochitl.service')
                self.assertEqual(result.returncode==0,accepted,result.stderr)

    def test_empty_scalar_requires_complete_single_line(self):
        for text,accepted in [('\n',True),('',False),('\nsecond\n',False)]:
            with self.subTest(text=text),tempfile.TemporaryDirectory() as d:
                root=Path(d);(root/'query.stdout').write_text(text)
                result=self.run_shell('actor.sh.in',root,'control_grammar query show --property=Job --value xochitl.service')
                self.assertEqual(result.returncode==0,accepted,result.stderr)

    def test_closed_policy_maximum_fits_below_file_limit(self):
        properties='OnFailure,OnFailureJobMode,FailureAction,StartLimitAction,Restart,RestartMode,TimeoutStopUSec,WatchdogUSec,KillMode,KillSignal,FinalKillSignal,SendSIGKILL,FragmentPath,DropInPaths,ExecStart'
        maxima={'ExecStart':2048,'DropInPaths':512,'OnFailure':256,'FragmentPath':192}
        text=''.join(key+'='+'x'*maxima.get(key,64)+'\n' for key in properties.split(','))
        self.assertLess(len(text.encode()),4096)
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);(root/'query.stdout').write_text(text)
            result=self.run_shell('actor.sh.in',root,'control_grammar query show --property='+properties+' xochitl.service')
            self.assertEqual(result.returncode,0,result.stderr)

    def test_issued_action_overflow_or_signal_enters_restore_without_replay(self):
        for signal_case in (False,True):
            with self.subTest(signal_case=signal_case),tempfile.TemporaryDirectory() as d:
                root=Path(d)
                producer=root/'client.py'
                code='from pathlib import Path\nimport os,signal,sys\np=Path('+repr(str(root/'issued'))+')\np.write_text(str(int(p.read_text())+1) if p.exists() else "1")\n'
                code+=('signal.signal(signal.SIGXFSZ,signal.SIG_DFL)\nos.write(1,b"x"*4096)\nos.write(1,b"x")\n' if signal_case else 'sys.stderr.write("x"*4097)\n')
                producer.write_text(code)
                (root/'actor.sh').write_text('#!/bin/sh\nprintf restored > '+shlex.quote(str(root/'restored'))+'\n')
                source=self.source('actor.sh.in',root).replace('exec /usr/bin/systemctl --no-pager "$@"','exec /usr/bin/python3 '+shlex.quote(str(producer))+' "$@"')
                result=subprocess.run(['/bin/bash','--posix','-c',source+'\ncontrol_shell() { :; }\ndeadline=$(($(now)+20)); set +e; query stop --no-block xochitl.service; finish'],capture_output=True,text=True,timeout=8)
                self.assertNotEqual(result.returncode,0)
                self.assertEqual((root/'issued').read_text(),'1')
                self.assertEqual((root/'restored').read_text(),'restored')
                self.assertTrue((root/'query.output-unknown').exists())
                self.assertTrue((root/'insertion.failed').exists())
                self.assertFalse(list(root.glob('core*')))

    def test_fixed_hash_pairs_complete_success_and_failure(self):
        pairs=[('packet-check','packet.files'),('hash-check','baseline.files'),('service-check','service.files'),
            ('recovery-packet-check','packet.files'),('recovery-runtime-check','runtime.files'),('recovery-service-check','service.files'),
            ('document-immutable-check','document-immutable.files'),('document-mutable-check','document-mutable.files')]
        for output,manifest in pairs:
            with self.subTest(output=output),tempfile.TemporaryDirectory() as d:
                root=Path(d);data=root/'protected';data.write_bytes(b'protected bytes')
                path=root/manifest;path.write_text(hashlib.sha256(data.read_bytes()).hexdigest()+'  '+str(data)+'\n');path.chmod(0o600)
                result=self.run_shell('actor.sh.in',root,'control_hash_check '+output+' '+manifest)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn(': OK',(root/output).read_text())
                data.write_bytes(b'changed')
                result=self.run_shell('actor.sh.in',root,'control_hash_check '+output+' '+manifest)
                self.assertNotEqual(result.returncode,0)
                self.assertLessEqual((root/output).stat().st_size,4096)
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            result=self.run_shell('actor.sh.in',root,'control_hash_check arbitrary arbitrary')
            self.assertNotEqual(result.returncode,0)
            self.assertFalse(list(root.iterdir()))

    def test_hash_cap_boundary_and_setup_failure_prevent_prefix_success(self):
        for size in (4095,4096,4097):
            with self.subTest(size=size),tempfile.TemporaryDirectory() as d:
                root=Path(d);(root/'packet.files').write_text('fixture');(root/'packet.files').chmod(0o600)
                producer=root/'hash-client.py';producer.write_text('import sys\nsys.stdout.write("x"*'+str(size)+')\n')
                source=self.source('actor.sh.in',root).replace('exec sha256sum -c "$manifest_name"','exec /usr/bin/python3 '+shlex.quote(str(producer)))
                result=subprocess.run(['/bin/bash','--posix','-c',source+'\ncontrol_shell() { :; }\ncontrol_hash_check packet-check packet.files'],capture_output=True,text=True,timeout=8)
                self.assertEqual(result.returncode==0,size<4096,result.stderr)
                self.assertLessEqual((root/'packet-check').stat().st_size,4096)
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);(root/'packet.files').write_text('fixture');(root/'packet.files').chmod(0o600)
            result=self.run_shell('actor.sh.in',root,'control_hash_check packet-check packet.files','control_shell() { return 90; }')
            self.assertNotEqual(result.returncode,0)
            self.assertEqual((root/'packet-check').stat().st_size,0)

if __name__ == '__main__': unittest.main()
