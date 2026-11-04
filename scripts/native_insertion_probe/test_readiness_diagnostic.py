"""Observation-purpose boundaries and diagnostic measurements; no device access."""
import base64
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import coordinator
from open_document_host import HostSequence, Refused
from open_document_timing import OpenDocumentTiming, diagnostic_cutoff
from owned_frame_capture import OBSERVATION_KIND, PHASE_LABELS, capture_script
from test_open_document_host import Adapter, png
import test_open_document_transport as transport_tests


class DiagnosticAdapter(Adapter):
    def __init__(self, selected, match_at=2):
        super().__init__(selected)
        self.frames = 0
        self.match_at = match_at
        self.fault = None

    def wait_diagnostic_context(self):
        self.calls.append('diagnostic-context')
        return self.raw('ready')

    def snapshot(self, stage, cutoff, idle):
        if stage != 'diagnostic' or cutoff != 167 or idle is not True:
            raise AssertionError('Diagnostic fixed stage and idle required')
        self.frames += 1
        raw = self.raw('ready')
        raw.update(uptime=f'{139+self.frames}.50 0.00\n', diagnostic_cutoff=167,
                   phases=''.join(f'{label} {139+self.frames}.00\n' for label in PHASE_LABELS).encode(),
                   png=png(self.frames >= self.match_at), diagnostics={})
        if self.fault: self.fault(raw)
        return raw


class DiagnosticTests(unittest.TestCase):
    def test_coordinator_dispatch_cannot_fall_through_to_production(self):
        calls=[]
        class Sequence:
            def run(self): calls.append('production')
            def run_diagnostic(self): calls.append('diagnostic'); return {'production_ready':False}
        self.assertFalse(coordinator.run_host_sequence(Sequence(),OBSERVATION_KIND)['production_ready'])
        self.assertEqual(calls,['diagnostic'])
        with self.assertRaises(ValueError): coordinator.run_host_sequence(Sequence(),'unknown')
        self.assertEqual(calls,['diagnostic'])

    def fixture(self, match_at=2):
        selected = {'experiment_kind': OBSERVATION_KIND, 'nonce': '1'*32, 'boot_id': 'boot',
                    'document_id': 'document', 'page_ids': ['source', 'b', 'c', 'd', 'e', 'f', 'g'],
                    'target_id': 'new', 'immutable_hashes': {'ink.rm': 'sha'}}
        adapter = DiagnosticAdapter(selected, match_at)
        return HostSequence(adapter, selected, png(True), png(footer=False), png(body=False)), adapter

    def test_library_or_three_nonmatches_only_report_and_stop(self):
        for match_at in (2, 4):
            host, adapter = self.fixture(match_at)
            report = host.run_diagnostic()
            self.assertEqual(adapter.frames, min(match_at, 3))
            self.assertEqual(report['library_observed'], match_at == 2)
            self.assertFalse(report['production_ready'])
            self.assertFalse(report['publication_permitted'])
            self.assertEqual(adapter.calls.count('stop'), 1)
            self.assertNotIn('tap', adapter.calls)
            self.assertNotIn('arm', adapter.calls)
            self.assertNotIn('result', adapter.calls)

    def test_diagnostic_cannot_enter_normal_sequence_or_publish(self):
        host, unused = self.fixture()
        with self.assertRaises(Refused): host.run()
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = transport_tests.AdapterTests().adapter(root, {})
            adapter.profile['experiment_kind'] = OBSERVATION_KIND
            for action in (lambda: adapter.open_once(('boot',2,3),100),
                           lambda: adapter.arm_once(None,None,{}),
                           lambda: adapter.result_snapshot(('boot',2,3),100),
                           adapter.wait_library):
                with self.assertRaises(Refused): action()
            self.assertFalse(calls)
            self.assertFalse(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)

    def test_faults_are_terminal_and_stop_without_any_next_frame(self):
        faults = [lambda r: r.update(start_ticks=562), lambda r: r.update(closed=True),
                  lambda r: r['hashes'].update({'ink.rm':'changed'}),
                  lambda r: r['idle'].update(pen_contact=1),
                  lambda r: r.update(uptime='167.00 0.00\n'),
                  lambda r: r.update(diagnostics={'terminal_stage':'deadline'}),
                  lambda r: r.update(waiting=('1'*32+' 42 561 waiting 168000\n').encode()),
                  lambda r: r.update(phases=b'partial')]
        for fault in faults:
            host, adapter = self.fixture()
            adapter.fault = fault
            with self.assertRaises(Refused): host.run_diagnostic()
            self.assertEqual(adapter.frames, 1)
            self.assertEqual(adapter.calls.count('stop'),1)

    def test_phase_order_canonical_bounds_and_no_internal_plane_timestamp(self):
        host, adapter = self.fixture()
        host.bind(adapter.wait_diagnostic_context()); host.diagnostic_end=167
        raw = adapter.snapshot('diagnostic',167,True)
        self.assertEqual(len(host.diagnostic_phases(raw)),21)
        invalid = [raw['phases'][:-1], raw['phases']+b'extra\n', b'x'*1025,
                   raw['phases'].replace(b'artifact_begin',b'unknown'),
                   raw['phases'].replace(b'140.00',b'-140.00',1),
                   raw['phases'].replace(b'140.00',b'167.00',1),
                   raw['phases'].replace(b'140.00',b'140.60',1)]
        for data in invalid:
            with self.assertRaises(Refused): host.diagnostic_phases(raw|{'phases':data})
        self.assertNotIn('plane_read',PHASE_LABELS)

    def test_cutoff_floors_original_sdk_expiry_and_keeps_production_ready20(self):
        timing=OpenDocumentTiming(100,460,127)
        self.assertEqual(timing.stage_end('ready'),147)
        self.assertEqual(diagnostic_cutoff(timing,166999),166)
        self.assertEqual(diagnostic_cutoff(timing,200000),167)
        with self.assertRaises(ValueError): diagnostic_cutoff(timing,127000)

    def test_adapter_freezes_context_and_has_three_diagnostic_claims(self):
        with tempfile.TemporaryDirectory() as root:
            adapter,calls=transport_tests.AdapterTests().adapter(root,{})
            adapter.profile.update(experiment_kind=OBSERVATION_KIND,immutable_hashes={},boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
            nonce=adapter.profile['nonce']
            context=f'{nonce} 1 361 50 120\n{nonce} 2 3 waiting 89999\n1 2\n2 3\n51.00 0.00\n'
            raw=transport_tests.AdapterTests().raw_response()
            raw.update(phases='',waiting=base64.b64encode(f'{nonce} 2 3 waiting 89999\n'.encode()).decode())
            def transport(arguments,script,timeout):
                calls.append(script)
                if script=='set -eu\ncat /proc/uptime\n': output='51.00 0.00\n'
                elif 'cat "$root/attempt.identity"' in script: output=context
                else: output=json.dumps(raw)
                return {'timeout':False,'exit':0,'stdout':output}
            adapter.transport=transport
            self.assertEqual(adapter.wait_diagnostic_context()['pid'],2)
            self.assertEqual(adapter.diagnostic_end,89)
            for _ in range(3): adapter.raw_snapshot('diagnostic',89,True)
            count=len(calls)
            with self.assertRaises(Refused): adapter.raw_snapshot('diagnostic',89,True)
            self.assertEqual(len(calls),count)
            captures=[s for s in calls if 'host-diagnostic-' in s]
            self.assertEqual(len(captures),3)
            for index,script in enumerate(captures):
                self.assertIn(f'host-diagnostic-{index:03}.claim',script)
                self.assertNotIn('--publish',script)
                self.assertNotIn('--tap ',script)
                self.assertGreaterEqual(script.count('test ! -e "$root/diagnostics.json"'),2)
                self.assertGreaterEqual(script.count('test ! -L "$root/callback.json"'),2)
                labels=re.findall(r'^diagnostic_phase ([a-z_]+)',script,re.M)
                self.assertEqual(labels,list(PHASE_LABELS))
                self.assertLess(script.index('producer=$?'),script.index('diagnostic_phase producer_end'))
                self.assertLess(script.index('converter=$?'),script.index('diagnostic_phase converter_end'))
            with self.assertRaises(ValueError):
                capture_script('host-diagnostic-003',nonce,adapter.profile['boot_id'],diagnostic=True)
            with self.assertRaises(ValueError):
                capture_script('host-diagnostic-000',nonce,adapter.profile['boot_id'])

    def test_startup_terminal_check_precedes_waiting_markers_and_failure_is_terminal(self):
        with tempfile.TemporaryDirectory() as root:
            adapter,calls=transport_tests.AdapterTests().adapter(root,{'timeout':False,'exit':1,'stdout':''})
            adapter.profile['experiment_kind']=OBSERVATION_KIND
            with self.assertRaises(Refused): adapter.wait_diagnostic_context()
            self.assertEqual(len(calls),1)
            self.assertLess(calls[0].index('test ! -e "$root/diagnostics.json"'),calls[0].index('if test -f "$root/candidate.window"'))
            with self.assertRaises(Refused): adapter.wait_diagnostic_context()
            self.assertEqual(len(calls),1)

    @unittest.skipUnless(os.name=='posix','POSIX bounded record and closure controls')
    def test_executed_phase_record_and_terminal_presence_refusal(self):
        for name in ('entry.closed','restore.claim','stop.request','stop.admitted','diagnostics.json','callback.json','creation-arm','creation-arm.tmp','tap.go','tap-start.claim'):
            for kind in ('file','directory','dangling'):
                for point in ('entry','closing'):
                    with self.subTest(name=name,kind=kind,point=point),tempfile.TemporaryDirectory() as root:
                        adapter,_=transport_tests.AdapterTests().adapter(root,{})
                        path=Path(root)/(name if point=='entry' else 'pending')
                        if kind=='file': path.write_text('partial')
                        elif kind=='directory': path.mkdir()
                        else: path.symlink_to(Path(root)/'absent')
                        script='set -eu\n'+adapter.diagnostic_closure_checks()
                        if point=='closing':
                            script+='printf entered > "$root/entered"\nmv "$root/pending" "$root/'+name+'"\n'+adapter.diagnostic_closure_checks()
                        script+='printf admitted\n'
                        result=subprocess.run(['sh','-c',script],env=dict(os.environ,root=root),capture_output=True)
                        self.assertNotEqual(result.returncode,0)
                        self.assertFalse(result.stdout)
                        self.assertEqual((Path(root)/'entered').exists(),point=='closing')
        with tempfile.TemporaryDirectory() as root:
            adapter,_=transport_tests.AdapterTests().adapter(root,{})
            script='set -eu\numask 077\nprivate() { test -f "$1"; }\n'+adapter.diagnostic_phase_setup('host-diagnostic-000')
            script+=''.join(f'diagnostic_phase {label}\n' for label in PHASE_LABELS[1:])
            result=subprocess.run(['sh','-c',script],env=dict(os.environ,root=root),capture_output=True)
            self.assertEqual(result.returncode,0,result.stderr)
            data=(Path(root)/'host-diagnostic-000.phases').read_bytes()
            self.assertLessEqual(len(data),1024)
            self.assertEqual([line.split()[0].decode() for line in data.splitlines()],list(PHASE_LABELS))
            repeated=subprocess.run(['sh','-c',script],env=dict(os.environ,root=root),capture_output=True)
            self.assertNotEqual(repeated.returncode,0)
            self.assertEqual((Path(root)/'host-diagnostic-000.phases').read_bytes(),data)


if __name__=='__main__': unittest.main()
