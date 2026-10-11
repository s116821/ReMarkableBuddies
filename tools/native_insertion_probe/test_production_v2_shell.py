"""Executed POSIX closure and actor admission faults, no device/services."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import test_actor
import test_open_document_transport as transport_tests


@unittest.skipUnless(os.name == 'posix', 'POSIX exact-source execution')
class ShellV2Tests(unittest.TestCase):
    def test_actual_arm_entry_loop_refuses_every_marker_shape(self):
        source=Path(__file__).with_name('arm.sh.in').read_text()
        start=source.index('for name in deadline host-arm.claim')
        loop=source[start:source.index('done',start)+4]
        for name in ('deadline','host-arm.claim','actor.claim','actor.ready','guard.request','start.request','restore.claim'):
            for shape in ('file','directory','dangling'):
                with self.subTest(name=name,shape=shape),tempfile.TemporaryDirectory() as temporary:
                    root=Path(temporary); path=root/name
                    if shape=='file': path.write_text('partial')
                    elif shape=='directory': path.mkdir()
                    else: path.symlink_to(root/'absent')
                    result=subprocess.run(['sh','-c','set -eu\n'+loop+'\nprintf admitted'],env=dict(os.environ,root=str(root)),capture_output=True)
                    self.assertNotEqual(result.returncode,0); self.assertFalse(result.stdout)

    def test_recovery_hash_failures_cannot_be_masked_by_later_success(self):
        for failure in ('packet.files','runtime.files'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary)
                overrides='sha256sum() { printf "%s\\n" "$2" >> "$root/calls"; test "$2" != "'+ ('packet.files' if failure=='packet.files' else str(root/'runtime.files'))+'"; }'
                test_actor.fixture.shell(root,
                    'if recovery_hashes; then printf masked > "$root/stock.restored"; exit 1; fi',overrides)
                self.assertFalse((root/'stock.restored').exists())
                self.assertNotIn('service.files',(root/'calls').read_text())

    def test_candidate_latest_start_and_late_refusal(self):
        for now, expected in ((145,0),(146,1)):
            with tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary)
                test_actor.fixture.shell(root,
                    'armed_at=100; deadline=460; experiment=open-document-insertion-v2; observation_window',
                    'now() { printf "'+str(now)+'\\n"; }',expected=expected)
                if not expected:
                    self.assertEqual((root/'candidate.window').read_text(),'1'*32+' 100 460 145 275\n')
                else: self.assertFalse((root/'candidate.window').exists())

    def test_conditional_function_context_cannot_mask_missing_process_or_guardian(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            test_actor.fixture.shell(root,
                'if same_process 999999999 42; then exit 1; fi',
                'start_of() { printf "42\\n"; }')
            (root/'tap-start.claim').write_text('1'*32); (root/'tap-start.claim').chmod(0o600)
            test_actor.fixture.shell(root,
                'experiment=open-document-insertion-v2; if recover_owned_input; then exit 1; fi')
            self.assertFalse((root/'tap-recovery.log').exists())

    def test_actual_production_entry_and_closing_loop_refuse_every_closure_shape(self):
        for point in ('entry','closing','guardian-wait'):
            for name in ('entry.closed','restore.claim','stop.request','stop.admitted') if point != 'guardian-wait' else ('tap.unresolved','tap.empty','tap.cleaned','tap.recovered','tap.supervisor-spent'):
                for shape in ('file','directory','dangling'):
                    with self.subTest(point=point,name=name,shape=shape), tempfile.TemporaryDirectory() as temporary:
                        root=Path(temporary); adapter,_=transport_tests.AdapterTests().adapter(root,{})
                        # Extract the actual complete generated loop, not a duplicate assertion.
                        script=adapter.prelude(100,('boot',2,3))
                        start=script.index('for name in entry.closed')
                        end=script.index('done',start)+4
                        loop=script[start:end]
                        if point=='guardian-wait':
                            loop='for name in tap.unresolved tap.empty tap.cleaned tap.recovered tap.supervisor-spent; do test ! -e "$root/$name"; test ! -L "$root/$name"; done'
                            self.assertIn(loop, Path(transport_tests.__file__).with_name('open_document_transport.py').read_text())
                        path=root/(name if point!='closing' else 'pending')
                        if shape=='file': path.write_text('partial')
                        elif shape=='directory': path.mkdir()
                        else: path.symlink_to(root/'absent')
                        prefix='set -eu\n'
                        if point=='closing': prefix+='printf entered > "$root/entered"\nmv "$root/pending" "$root/'+name+'"\n'
                        result=subprocess.run(['sh','-c',prefix+loop+'\nprintf admitted'],env=dict(os.environ,root=str(root)),capture_output=True)
                        self.assertNotEqual(result.returncode,0); self.assertFalse(result.stdout)


if __name__=='__main__': unittest.main()
