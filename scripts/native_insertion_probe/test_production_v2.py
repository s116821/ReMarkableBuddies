"""Fixed-clock refusal and pixel counterexamples; no device or input."""
import io
import base64
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from PIL import Image
from open_document_host import library_comparison, Refused
from open_document_timing import OpenDocumentTiming, PRODUCTION_V2, V2_STAGES, V2_CONFIG_LINE, validate_v2_config
import test_open_document_transport as transport_tests
import test_open_document_host as host_tests


class FixedV2Tests(unittest.TestCase):
    def test_known_stage_clamps_query_and_refresh_never_renews(self):
        with tempfile.TemporaryDirectory() as root:
            adapter,_=transport_tests.AdapterTests().adapter(root,{})
            adapter.profile['experiment_kind']=PRODUCTION_V2
            adapter.candidate_remote_at=100; adapter.candidate_local_anchor=0
            calls=[]
            adapter.call=lambda script,timeout:(calls.append(timeout) or '114.00 0.00\n')
            with patch('open_document_transport.time.monotonic',return_value=14.9):
                _,_,deadline=adapter.clock_query(115)
            self.assertAlmostEqual(calls[0],0.1)
            self.assertEqual(deadline,15)
            with patch('open_document_transport.time.monotonic',return_value=15):
                with self.assertRaises(Refused): adapter.action_budget(115,3)
                with self.assertRaises(Refused): adapter.snapshot_timeout(115)
            self.assertEqual(len(calls),1)
            # STOP125 derives from the original candidate anchor, not a new
            # 125-second allowance or the previous result remaining interval.
            self.assertEqual(adapter.known_deadline(125),25)

    def test_configuration_comments_literals_duplicates_and_other_selector_refuse(self):
        validate_v2_config('void configure() {\n'+V2_CONFIG_LINE+'\n}\n')
        for text in ('// '+V2_CONFIG_LINE, '/*\n'+V2_CONFIG_LINE+'\n*/',
                     'const char *s="'+V2_CONFIG_LINE+'";', V2_CONFIG_LINE+'\n'+V2_CONFIG_LINE,
                     V2_CONFIG_LINE+'\nconfig.setupVariant = qml_access::CreationConfig::SetupVariant::Legacy40s;'):
            with self.subTest(text=text), self.assertRaises(ValueError): validate_v2_config(text)

    def test_original_deadline_and_latest_start(self):
        timing = OpenDocumentTiming(100, 460, 145, PRODUCTION_V2)
        self.assertEqual(timing.observation_end, 275)
        self.assertEqual({s: timing.stage_end(s) for s in V2_STAGES},
                         {s: 145 + v for s, v in V2_STAGES.items()})
        self.assertTrue(timing.admit('trigger', 234))
        self.assertFalse(timing.admit('trigger', 235))
        with self.assertRaises(ValueError): OpenDocumentTiming(100, 460, 146, PRODUCTION_V2)
        self.assertEqual(OpenDocumentTiming(100, 460, 205).observation_end, 275)

    def test_query_cost_and_full_publication_reservation(self):
        for cutoff, elapsed, allowed in ((65, 1, True), (65, 1.001, False), (64, 1, False)):
            with self.subTest(cutoff=cutoff, elapsed=elapsed), tempfile.TemporaryDirectory() as root:
                adapter, calls = transport_tests.AdapterTests().adapter(root, {})
                adapter.profile['experiment_kind'] = PRODUCTION_V2
                adapter.call = lambda script, timeout: '51.99 0.00\n'
                with patch('open_document_transport.time.monotonic', side_effect=[10, 10 + elapsed]):
                    if allowed:
                        cap, deadline = adapter.action_budget(cutoff, 12, 100000, full=True)
                        self.assertEqual(cap, 12)
                        self.assertAlmostEqual(deadline, 23)
                    else:
                        with self.assertRaises(Refused): adapter.action_budget(cutoff, 12, 100000, full=True)
                self.assertFalse(calls)

    def test_sdk_original_expiry_and_late_processing_refuse(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, _ = transport_tests.AdapterTests().adapter(root, {})
            adapter.profile['experiment_kind'] = PRODUCTION_V2
            adapter.call = lambda script, timeout: '51.00 0.00\n'
            with patch('open_document_transport.time.monotonic', side_effect=[10, 10]):
                with self.assertRaises(Refused): adapter.action_budget(100, 12, 63000, full=True)
            adapter.action_deadline = 20
            with patch('open_document_transport.time.monotonic', return_value=20):
                with self.assertRaises(Refused): adapter.action_guard()

    def test_publication_entry_reservation_and_uncertain_response_never_replay(self):
        for remaining, published in ((11.99,True),(12.01,False),(12.01,True)):
            with self.subTest(remaining=remaining,published=published), tempfile.TemporaryDirectory() as root:
                adapter,calls=transport_tests.AdapterTests().adapter(root,{'timeout':False,'exit':0,'stdout':json.dumps({'published':published,'retry_permitted':False})})
                adapter.profile['experiment_kind']=PRODUCTION_V2
                adapter.receipt['files']={name:'a'*64 for name in ('runtime.files','document-immutable.files')}
                adapter.tap_released=True; adapter.tap_release_sha256=adapter.tap_supervisor_sha256='a'*64
                adapter.action_budget=lambda *args,**kwargs:(12,100+remaining)
                timing=OpenDocumentTiming(100,460,127,PRODUCTION_V2)
                waiting=SimpleNamespace(nonce='1'*32,pid=2,start_ticks=3,expiry_ms=222000)
                ready={ext+'_raw_sha256':'a'*64 for ext in ('content','metadata')}
                with patch('open_document_transport.time.monotonic',return_value=100):
                    if remaining<12 or not published:
                        with self.assertRaises(Refused): adapter.arm_once(waiting,timing,ready)
                    else: adapter.arm_once(waiting,timing,ready)
                    with self.assertRaises(Refused): adapter.arm_once(waiting,timing,ready)
                self.assertEqual(len(calls),0 if remaining<12 else 1)
                if calls: self.assertIn('open-document-insertion-v2',calls[0])

    def test_ready_retention_expiry_prevents_go_and_replay(self):
        with tempfile.TemporaryDirectory() as root:
            adapter,calls=transport_tests.AdapterTests().adapter(root,{})
            adapter.profile['experiment_kind']=PRODUCTION_V2
            guardian=transport_tests.AdapterTests().guardian(adapter,'guardian-ready-no-down')
            encode=lambda v:base64.b64encode(v).decode()
            response=json.dumps({'ready':encode((json.dumps(guardian)+'\n').encode()),
                'setup':encode(b'setup'),'selected_cutoff_ms':100000,'uptime':encode(b'98.00 0.00\n')})
            def call(script,timeout=3): calls.append(script); return response
            adapter.call=call; adapter.action_budget=lambda *a,**k:(3,100)
            clock=[10]
            adapter.retain=lambda stage,evidence:clock.__setitem__(0,13)
            with patch('open_document_transport.time.monotonic',side_effect=lambda:clock[0]):
                with self.assertRaisesRegex(Refused,'setup lease expired'): adapter.open_once(('boot',2,3),100)
                with self.assertRaises(Refused): adapter.open_once(('boot',2,3),100)
            self.assertEqual(len(calls),1)
            self.assertNotIn('--publish-go',calls[0]); self.assertFalse(adapter.tap_released)

    def test_result_partial_json_poll_wait_and_stop_charge_complete_cost(self):
        with tempfile.TemporaryDirectory() as root:
            adapter,calls=transport_tests.AdapterTests().adapter(root,{})
            adapter.profile['experiment_kind']=PRODUCTION_V2
            adapter.action_budget=lambda *a,**k:(3,10.05)
            adapter.call=lambda *a,**k:'{'
            clock=[10]
            def sleep(duration):
                self.assertLessEqual(duration,0.051); clock[0]+=duration
            with patch('open_document_transport.time.monotonic',side_effect=lambda:clock[0]), patch('open_document_transport.time.sleep',side_effect=sleep):
                with self.assertRaises(Refused): adapter.result_snapshot(('boot',2,3),100)
            self.assertAlmostEqual(clock[0],10.05)
            adapter.call=lambda *a,**k:clock.__setitem__(0,11)
            with patch('open_document_transport.time.monotonic',side_effect=lambda:clock[0]):
                with self.assertRaises(Refused): adapter.stop_once(('boot',2,3),100)
                adapter.stop_once(('boot',2,3),100)


class LibraryV2Tests(unittest.TestCase):
    def test_selected_v2_complete_sequence_keeps_result_authority_separate(self):
        fixture=host_tests.HostTests(); fixture.setUp()
        fixture.selection.update(experiment_kind=PRODUCTION_V2,production_contract=__import__('open_document_timing').V2_CONTRACT)
        def context(stage,raw):
            raw['window'][4]=257
            raw['waiting']=('1'*32+' 42 561 waiting 222000\n').encode()
        fixture.adapter.change=context
        report=fixture.host.run()
        self.assertFalse(report['observed_target_presence'])
        self.assertIn('not inferred',report['native_success'])
        for action in ('tap','arm','stop'): self.assertEqual(fixture.adapter.calls.count(action),1)
        self.assertIn('retain:library-comparison-000',fixture.adapter.calls)

    @staticmethod
    def png(points):
        im = Image.new('RGB', (768, 1024), 'white')
        for point in points: im.putpixel(point, (0, 0, 0))
        out = io.BytesIO(); im.save(out, format='PNG'); return out.getvalue()

    def test_exact_half_open_bar_and_complete_difference_bitmap(self):
        points = [(x, y) for x in (735, 736) for y in range(30, 36)]
        reference = self.png([])
        report = library_comparison(self.png(points), reference, PRODUCTION_V2)
        self.assertTrue(report['masked_match']); self.assertFalse(report['whole_image_exact'])
        self.assertEqual(report['different_pixels'], 12)
        bitmap = report['difference_bitmap_row_major_lsb_first']
        self.assertEqual(len(bitmap), 98304)
        self.assertEqual(sum(v.bit_count() for v in bitmap), 12)
        for x, y in points:
            index = y * 768 + x
            self.assertTrue(bitmap[index // 8] & (1 << (index % 8)))
        self.assertFalse(library_comparison(self.png(points), reference, 'open-document-insertion-v1')['masked_match'])

    def test_every_neighbor_and_semantic_counterexample_refuses(self):
        controls = [(734,30),(737,35),(735,29),(735,36),(740,33),(731,30),
                    (679,33),(700,33),(30,32),(660,98),(109,210),(40,405),(384,512)]
        reference = self.png([])
        for point in controls:
            with self.subTest(point=point):
                report = library_comparison(self.png([point]), reference, PRODUCTION_V2)
                self.assertFalse(report['masked_match'])
                self.assertEqual(report['outside_exclusion_pixels'], 1)


if __name__ == '__main__': unittest.main()
