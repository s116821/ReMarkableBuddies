"""Actual local subprocess and quota checks; no SSH or tablet calls."""
import asyncio
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import coordinator as c
from source5_navigation import ControlLedger, FUTURE_CONTROL_BYTES

class BoundedTransport(unittest.TestCase):
    def run_client(self, code, payload='', timeout=2):
        processes = []
        original = asyncio.create_subprocess_exec
        async def capture(*args, **kwargs):
            process = await original(*args, **kwargs)
            processes.append(process)
            return process
        with patch('coordinator.asyncio.create_subprocess_exec', capture):
            result = c.bounded_control_transport([sys.executable, '-c', code], payload, timeout)
        self.assertEqual(len(processes), 1)
        self.assertIsNotNone(processes[0].returncode, 'actual client death must be confirmed')
        self.assertLessEqual(len((json.dumps(result)+'\n').encode()), 2048)
        return result

    def test_combined_stream_limit_exact_and_one_over(self):
        for size in (1023, 1024, 1025):
            with self.subTest(size=size):
                result = self.run_client(f'import sys; sys.stdout.write("a"*512); sys.stdout.flush(); sys.stderr.write("b"*{size-512}); sys.stderr.flush()')
                self.assertLessEqual(len(result['stdout'].encode())+len(result['stderr'].encode()),1024)
                self.assertEqual(result['exit'] == 0, size <= 1024)
                if size > 1024:
                    self.assertTrue(result['uncertain_remote_action'])
                    self.assertTrue(result['output_limit_exceeded'])

    def test_full_input_is_delivered_once_before_eof(self):
        payload = 'selected bounded script\n'*800
        result = self.run_client('import sys,hashlib; data=sys.stdin.buffer.read(); print(str(len(data))+":"+hashlib.sha256(data).hexdigest())',payload)
        self.assertEqual(result['exit'],0)
        self.assertEqual(result['stdout'].strip(),str(len(payload.encode()))+':'+hashlib.sha256(payload.encode()).hexdigest())

    def test_timeout_kills_and_reaps_client(self):
        started=time.monotonic()
        result=self.run_client('import sys,time; print("waiting",flush=True); time.sleep(10)',timeout=.2)
        self.assertLess(time.monotonic()-started,2)
        self.assertIsNone(result['exit'])
        self.assertTrue(result['timeout'])
        self.assertTrue(result['uncertain_remote_action'])

    def test_flood_is_refused_without_waiting_for_client_sleep(self):
        started=time.monotonic()
        result=self.run_client('import sys,time; sys.stdout.buffer.write(b"x"*1000000); sys.stdout.flush(); time.sleep(10)')
        self.assertLess(time.monotonic()-started,2)
        self.assertIsNone(result['exit'])
        self.assertTrue(result['output_limit_exceeded'])

    def test_json_expansion_is_unknown_with_prefix_hash_only(self):
        for byte in (b'\x00',b'\xff'):
            with self.subTest(byte=byte):
                result=self.run_client(f'import sys; sys.stdout.buffer.write({byte!r}*1024)')
                self.assertIsNone(result['exit'])
                self.assertEqual(result['stdout'],'')
                self.assertEqual(result['captured_prefix_sha256']['stdout'],hashlib.sha256(byte*1024).hexdigest())
                self.assertTrue(result['output_limit_exceeded'])

    def test_nonzero_exit_is_preserved_and_never_admitted(self):
        result=self.run_client('import sys; print("failure"); sys.exit(7)')
        self.assertEqual(result['exit'],7)
        self.assertFalse(result['timeout'])

    def test_invalid_start_and_input_budget_do_not_start_client(self):
        with patch('coordinator.asyncio.create_subprocess_exec') as create:
            for script,timeout in [('x'*65537,1),('selected',0)]:
                result=c.bounded_control_transport([sys.executable],script,timeout)
                self.assertIsNone(result['exit'])
                self.assertTrue(result['no_remote_process_started'])
            create.assert_not_called()
        result=c.bounded_control_transport(['definitely-missing-source5-fixture-client'],'selected',1)
        self.assertIsNone(result['exit'])
        self.assertTrue(result['no_remote_process_started'])
        self.assertEqual(result['transport_error'],'FileNotFoundError')

    def test_late_process_start_never_feeds_script(self):
        with tempfile.TemporaryDirectory() as directory:
            marker=Path(directory)/'input-issued'
            processes=[];original=asyncio.create_subprocess_exec
            async def slow(*args,**kwargs):
                process=await original(*args,**kwargs);processes.append(process)
                await asyncio.sleep(.2)
                return process
            code=f'import sys; from pathlib import Path; data=sys.stdin.buffer.read(); Path({str(marker)!r}).write_bytes(data) if data else None'
            with patch('coordinator.asyncio.create_subprocess_exec',slow):
                result=c.bounded_control_transport([sys.executable,'-c',code],'must-not-be-fed',.05)
            self.assertTrue(result['timeout'])
            self.assertIsNone(result['exit'])
            self.assertFalse(marker.exists())
            self.assertIsNotNone(processes[0].returncode)

class ControlWrites(unittest.TestCase):
    def test_closed_controller_record_caps_refuse_before_open_or_claim(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);ledger=ControlLedger()
            for name,maximum in list(c.SOURCE5_CONTROL_FILE_CAPS.items())+[('monitor-000.json',2048)]:
                data={'padding':'x'*maximum}
                with self.subTest(name=name),self.assertRaises(ValueError):
                    c.control_write(root,name,data,ledger)
                self.assertFalse((root/name).exists())
            self.assertEqual(ledger.used,0)

    def test_claim_precedes_file_open_and_failed_claim_creates_no_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);ledger=ControlLedger()
            original=ledger.claim
            def claim(size,terminal=False):
                self.assertFalse((root/'record.json').exists())
                original(size,terminal)
            with patch.object(ledger,'claim',claim):
                c.control_write(root,'record.json',{'answer':42},ledger)
            self.assertEqual(ledger.used,(root/'record.json').stat().st_size)
            ledger.claim(FUTURE_CONTROL_BYTES-ledger.terminal_reserve-ledger.used)
            with self.assertRaises(ValueError):c.control_write(root,'refused.json',{'x':1},ledger)
            self.assertFalse((root/'refused.json').exists())

    def test_failed_exclusive_open_does_not_refund_claim(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);ledger=ControlLedger()
            (root/'record.json').write_bytes(b'preserve')
            with self.assertRaises(FileExistsError):c.control_write(root,'record.json',{'x':1},ledger)
            self.assertGreater(ledger.used,0)
            self.assertEqual((root/'record.json').read_bytes(),b'preserve')

    def test_exhaustion_preserves_one_marker_and_terminal_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);ledger=ControlLedger()
            ledger.claim(FUTURE_CONTROL_BYTES-ledger.terminal_reserve)
            self.assertFalse(c.control_write_or_mark(root,'normal.json',{'x':1},ledger))
            self.assertTrue((root/'control-overflow.json').exists())

            self.assertFalse((root/'normal.json').exists())
            before=ledger.used
            self.assertFalse(c.control_write_or_mark(root,'again.json',{'x':1},ledger))
            self.assertEqual(ledger.used,before)
            self.assertTrue(c.control_write_or_mark(root,'terminal.json',{'presence_only':True},ledger,terminal=True))
            self.assertLessEqual(ledger.used,FUTURE_CONTROL_BYTES)

    def test_adapter_overflow_still_gets_explicit_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);ledger=ControlLedger()
            with self.assertRaises(ValueError):ledger.claim(FUTURE_CONTROL_BYTES)
            self.assertFalse(c.control_write_or_mark(root,'normal.json',{},ledger))
            self.assertTrue((root/'control-overflow.json').exists())

@unittest.skipUnless(os.name=='posix' and Path('/bin/bash').exists(), 'Linux Bash fixture required')
class CompactMonitor(unittest.TestCase):
    def fixture(self, root):
        root.chmod(0o700)
        (root/'owner').write_text('1'*32);(root/'owner').chmod(0o600)
        return {'remote_root':str(root),'nonce':'1'*32,'fixture_inventory_variant':c.SOURCE5}

    def read_summary(self, receipt):
        return __import__('subprocess').run(['/bin/bash','--posix','-c',c.monitor_script(receipt)],capture_output=True,text=True,timeout=3)

    def test_complete_small_summary_changes_with_known_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);receipt=self.fixture(root)
            before=self.read_summary(receipt)
            self.assertEqual(before.returncode,0,before.stderr)
            self.assertIn('DIAGNOSTIC_ONLY_SUMMARY',before.stdout)
            self.assertNotIn('EVIDENCE_TERMINAL_PRESENT',before.stdout)
            self.assertLessEqual(len(before.stdout.encode())+len(before.stderr.encode()),1024)
            (root/'stock.restored').write_text('presence only');(root/'stock.restored').chmod(0o600)
            after=self.read_summary(receipt)
            self.assertEqual(after.returncode,0,after.stderr)
            self.assertNotEqual(before.stdout.split('sha256=')[1].strip(),after.stdout.split('sha256=')[1].splitlines()[0])
            self.assertIn('\nEVIDENCE_TERMINAL_PRESENT\n',after.stdout)
            self.assertLessEqual(len(after.stdout.encode())+len(after.stderr.encode()),1024)

    def test_bad_known_member_never_emits_terminal_or_complete_summary(self):
        for damage in ('symlink','hardlink','mode','oversize'):
            with self.subTest(damage=damage),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);receipt=self.fixture(root)
                (root/'stock.restored').write_text('presence only');(root/'stock.restored').chmod(0o600)
                file=root/'query.stdout'
                if damage=='symlink':file.symlink_to(root/'stock.restored')
                else:
                    file.write_bytes(b'x'*(20481 if damage=='oversize' else 1));file.chmod(0o600)
                    if damage=='hardlink':os.link(file,root/'alias')
                    if damage=='mode':file.chmod(0o644)
                result=self.read_summary(receipt)
                self.assertNotEqual(result.returncode,0)
                self.assertNotIn('DIAGNOSTIC_ONLY_SUMMARY',result.stdout)
                self.assertNotIn('EVIDENCE_TERMINAL_PRESENT',result.stdout)

if __name__=='__main__':unittest.main()
