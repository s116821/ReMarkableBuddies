"""Actual host path: recognized pending Library is read-only, never source authority."""
import unittest
from unittest.mock import patch
import test_open_document_host as host_tests
from open_document_host import Refused
from open_document_timing import PRODUCTION_V2, V2_CONTRACT

EMPTY = b'LastOpen=@ByteArray()\n'


class PendingLibraryTests(unittest.TestCase):
    def fixture(self, pending=1, loading=False, fault=None):
        f=host_tests.HostTests(); f.setUp()
        f.selection.update(experiment_kind=PRODUCTION_V2,production_contract=V2_CONTRACT)
        def change(stage,raw):
            raw['window'][4]=257
            raw['waiting']=('1'*32+' 42 561 waiting 222000\n').encode()
            raw['last_open_raw']=b'LastOpen=@ByteArray(document)\n'
            if stage=='ready' or (stage=='open' and f.adapter.open_frames<=pending):
                raw['png']=f.host.library_png
                raw['last_open']=None; raw['last_open_raw']=EMPTY; raw['diagnostics']={}
            if loading and stage=='ready' and f.adapter.ready_frames==1:
                raw['png']=host_tests.png(body=False,footer=False)
                raw['content_raw_sha256']='c'*64
            if fault is not None: fault(f,stage,raw)
        f.adapter.change=change
        return f

    def test_pending_then_footer_then_distinct_body_uses_one_input_and_arm(self):
        f=self.fixture(loading=True)
        f.host.run()
        self.assertEqual(f.adapter.open_frames,3)
        self.assertEqual(f.adapter.ready_frames,2)
        for action in ('tap','arm','stop'): self.assertEqual(f.adapter.calls.count(action),1)
        self.assertIn('retain:open-library-unready-000',f.adapter.calls)
        self.assertLess(f.adapter.calls.index('retain:open-library-unready-000'),f.adapter.calls.index('retain:source-footer-observed'))
        self.assertLess(f.adapter.calls.index('retain:source-footer-observed'),f.adapter.calls.index('retain:source-body-ready'))

    def test_cap_exhaustion_never_credits_footer_body_or_arm(self):
        f=self.fixture(pending=5)
        with self.assertRaises(Refused): f.host.run()
        self.assertEqual(f.adapter.open_frames,5)
        self.assertEqual(f.adapter.calls.count('tap'),1)
        self.assertNotIn('arm',f.adapter.calls)
        self.assertIsNone(f.host.source_sidecars)
        self.assertNotIn('retain:source-footer-observed',f.adapter.calls)

    def test_footer_on_last_attempt_requires_another_distinct_body_frame(self):
        f=self.fixture(pending=4)
        with self.assertRaises(Refused): f.host.run()
        self.assertEqual(f.adapter.open_frames,5)
        self.assertIn('retain:source-footer-observed',f.adapter.calls)
        self.assertNotIn('retain:source-body-ready',f.adapter.calls)
        self.assertNotIn('arm',f.adapter.calls)

    def test_library_after_footer_is_terminal(self):
        def fault(f,stage,raw):
            if stage=='open' and f.adapter.open_frames==2:
                raw['png']=f.host.library_png;raw['last_open']=None;raw['last_open_raw']=EMPTY;raw['diagnostics']={}
        f=self.fixture(pending=0,fault=fault)
        with self.assertRaisesRegex(Refused,'after source footer'): f.host.run()
        self.assertEqual(f.adapter.open_frames,2); self.assertNotIn('arm',f.adapter.calls)

    def test_each_context_fault_refuses_first_pending_frame(self):
        mutations=[
            lambda r:r.pop('last_open_raw'),
            lambda r:r.update(last_open_raw=b'LastOpen=malformed\n'),
            lambda r:r.update(last_open_raw=b'LastOpen=@ByteArray()'),
            lambda r:r.update(last_open='other-document'),
            lambda r:r.update(content_raw_sha256='a'*64),
            lambda r:r.update(metadata_raw_sha256='b'*64),
            lambda r:r['content'].update(pageCount=8),
            lambda r:r['content']['cPages']['pages'].reverse(),
            lambda r:r['content']['cPages']['lastOpened'].update(value='other-page'),
            lambda r:r['metadata'].update(lastOpenedPage=1),
            lambda r:r['metadata'].update(lastOpenedPage=False),
            lambda r:r.update(diagnostics={'terminal_stage':'cleanup'}),
            lambda r:r.update(closed=True),
            lambda r:r.update(pid=43),
            lambda r:r.update(hashes={}),
            lambda r:r.update(uptime='202.00 0.00\n'),
            lambda r:r.update(response_deadline_monotonic=0),
        ]
        for index,mutate in enumerate(mutations):
            def fault(f,stage,raw):
                if stage=='open': mutate(raw)
            with self.subTest(index=index):
                f=self.fixture(fault=fault)
                with self.assertRaises(Refused): f.host.run()
                self.assertEqual(f.adapter.open_frames,1)
                self.assertNotIn('arm',f.adapter.calls)
                self.assertNotIn('retain:open-library-unready-000',f.adapter.calls)

    def test_masked_library_or_modal_is_not_pending_library(self):
        from PIL import Image
        import io
        for point in ((735,30),(384,512)):
            def fault(f,stage,raw):
                if stage=='open':
                    im=Image.open(io.BytesIO(raw['png'])).convert('RGB')
                    old=im.getpixel(point);im.putpixel(point,tuple(255-v for v in old))
                    out=io.BytesIO();im.save(out,format='PNG');raw['png']=out.getvalue()
            with self.subTest(point=point):
                f=self.fixture(fault=fault)
                with self.assertRaises(Refused): f.host.run()
                self.assertEqual(f.adapter.open_frames,1);self.assertNotIn('arm',f.adapter.calls)

    def test_baseline_must_be_canonical_and_source_zero(self):
        for mutate in (lambda r:r.update(last_open_raw=b'unknown'),lambda r:r['metadata'].update(lastOpenedPage=1)):
            def fault(f,stage,raw):
                if stage=='ready': mutate(raw)
            f=self.fixture(fault=fault)
            with self.assertRaises(Refused): f.host.run()
            self.assertNotIn('arm',f.adapter.calls)

    def test_expired_sdk_and_late_retention_never_continue(self):
        for late in (False,True):
            def fault(f,stage,raw):
                if not late: raw['waiting']=('1'*32+' 42 561 waiting 150000\n').encode()
            f=self.fixture(fault=fault)
            original=f.adapter.retain
            def retain(stage,evidence):
                original(stage,evidence)
                if late and stage.startswith('open-library-unready-'):
                    evidence['response_deadline_monotonic']=0
            f.adapter.retain=retain
            with self.assertRaises(Refused): f.host.run()
            self.assertEqual(f.adapter.open_frames,1);self.assertNotIn('arm',f.adapter.calls)

    def test_transport_prearm_sdk_bound_refuses_acquisition_but_result_is_separate(self):
        import tempfile
        import test_open_document_transport as transport_tests
        for stage in ('open','result'):
            with self.subTest(stage=stage),tempfile.TemporaryDirectory() as root:
                adapter,calls=transport_tests.AdapterTests().adapter(root,{'timeout':True,'exit':None})
                adapter.profile.update(experiment_kind=PRODUCTION_V2,immutable_hashes={},boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
                adapter.sdk_expiry_ms=150000;adapter.candidate_local_anchor=0;adapter.candidate_remote_at=100
                with patch('open_document_transport.time.monotonic',return_value=50):
                    with self.assertRaises(Refused): adapter.raw_snapshot(stage,200,False)
                self.assertEqual(len(calls),0 if stage=='open' else 2)

    def test_retention_failure_is_terminal_without_another_capture(self):
        f=self.fixture(); original=f.adapter.retain
        def retain(stage,evidence):
            if stage.startswith('open-library-unready-'): raise OSError('retention fault')
            original(stage,evidence)
        f.adapter.retain=retain
        with self.assertRaises(OSError): f.host.run()
        self.assertEqual(f.adapter.open_frames,1); self.assertNotIn('arm',f.adapter.calls)

    def test_legacy_does_not_take_pending_branch(self):
        f=self.fixture(); f.selection['experiment_kind']='open-document-insertion-v1'
        original=f.adapter.change
        def change(stage,raw): original(stage,raw);raw['window'][4]=197
        f.adapter.change=change
        with self.assertRaises(Refused): f.host.run()
        self.assertEqual(f.adapter.open_frames,1)
        self.assertNotIn('retain:open-library-unready-000',f.adapter.calls)


if __name__=='__main__': unittest.main()
