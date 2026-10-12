"""Synthetic source5 control/phase tests. No network or tablet operations."""
import json
import copy
import hashlib
import multiprocessing
import tempfile
import time
import unittest
import os
import subprocess
from pathlib import Path
from unittest.mock import patch
from open_document_host import Refused,HostSequence
from open_document_transport import SSHAdapter
from coordinator import SOURCE5_REMOTE_CONTROL_BYTES
from source5_navigation import ControlLedger,FUTURE_CONTROL_BYTES,FrameLedger,PhaseBudget,bounded_local,native_positive

class QuotaTests(unittest.TestCase):
    def adapter(self,directory):
        adapter=object.__new__(SSHAdapter)
        adapter.output=Path(directory)
        adapter.control_ledger=ControlLedger()
        adapter.postnative_control={}
        adapter.stop_attempted=False
        return adapter

    def test_claim_precedes_worker_and_any_output(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=self.adapter(directory)
            def writer(fn,args,deadline,**kwargs):
                self.assertEqual(adapter.control_ledger.used,4096)
                self.assertFalse(any(Path(directory).iterdir()))
                return fn(*args)
            with patch("open_document_transport.bounded_local",side_effect=writer):
                adapter.retain_bounded("root-registration-previous",{"registered":True},time.monotonic()+2)
            self.assertEqual(json.loads((Path(directory)/"root-registration-previous.json").read_text()),{"registered":True})
            self.assertEqual(adapter.control_ledger.used,4096)

    def test_exhaustion_never_starts_worker_and_preserves_terminal_reserve(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=self.adapter(directory)
            adapter.control_ledger.claim(FUTURE_CONTROL_BYTES-65536)
            with patch("open_document_transport.bounded_local") as worker:
                with self.assertRaises(Refused):
                    adapter.retain_bounded("gesture-previous",{},time.monotonic()+2)
                worker.assert_not_called()
            self.assertFalse(any(Path(directory).iterdir()))
            adapter.control_ledger.claim(65536,terminal=True)
            self.assertEqual(adapter.control_ledger.remaining,0)

    def test_worker_failure_never_refunds(self):
        for failure in ("write failed","deadline exceeded","partial output"):
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as directory:
                adapter=self.adapter(directory)
                with patch("open_document_transport.bounded_local",side_effect=ValueError(failure)):
                    with self.assertRaises(Refused):
                        adapter.retain_bounded("gesture-next",{},time.monotonic()+2)
                self.assertEqual(adapter.control_ledger.used,16384)

    def test_record_overflow_and_existing_file_keep_irreversible_claim(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=self.adapter(directory)
            with patch("open_document_transport.bounded_local",side_effect=lambda fn,args,deadline,**kwargs:fn(*args)):
                with self.assertRaises(Refused):
                    adapter.retain_bounded("root-registration-next",{"data":"x"*4096},time.monotonic()+2)
                self.assertEqual(adapter.control_ledger.used,4096)
                path=Path(directory)/"root-registration-next.json"
                path.write_text("preserved")
                with self.assertRaises(FileExistsError):
                    adapter.retain_bounded("root-registration-next",{},time.monotonic()+2)
                self.assertEqual(path.read_text(),"preserved")
                self.assertEqual(adapter.control_ledger.used,8192)

    def test_unknown_role_refuses_before_claim_or_worker(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=self.adapter(directory)
            with patch("open_document_transport.bounded_local") as worker:
                with self.assertRaises(Refused):
                    adapter.retain_bounded("arbitrary-output",{},time.monotonic()+2)
                worker.assert_not_called()
            self.assertEqual(adapter.control_ledger.used,0)



def echo_value(value):
    return value

def slow_value():
    time.sleep(10)

def large_value():
    return b"x"*(3*1024*1024)

def partial_result_worker(path,fn,args,result_limit):
    with open(path,"xb") as stream:
        stream.write(b"partial")
        stream.flush()
    time.sleep(10)

class LocalWorkerTests(unittest.TestCase):
    def test_real_worker_finishes_and_observes_death(self):
        self.assertEqual(bounded_local(echo_value,({"done":True},),time.monotonic()+3),{"done":True})
    def test_slow_worker_is_killed_and_not_left_live(self):
        before={p.pid for p in multiprocessing.active_children()}
        start=time.monotonic()
        with self.assertRaises(ValueError):
            bounded_local(slow_value,(),start+0.4)
        self.assertLess(time.monotonic()-start,1.5)
        self.assertEqual({p.pid for p in multiprocessing.active_children()},before)
    def test_partial_file_is_not_a_completion_signal(self):
        before={p.pid for p in multiprocessing.active_children()}
        with patch("source5_navigation._work_file",partial_result_worker):
            with self.assertRaises(ValueError):
                bounded_local(echo_value,(True,),time.monotonic()+0.4)
        self.assertEqual({p.pid for p in multiprocessing.active_children()},before)
    def test_oversize_and_tiny_result_limits_refuse(self):
        with self.assertRaises(ValueError):
            bounded_local(large_value,(),time.monotonic()+3)
        with self.assertRaises(ValueError):
            bounded_local(echo_value,("x"*1000,),time.monotonic()+3,result_limit=512)

class BudgetTests(unittest.TestCase):
    def test_capture_and_processing_clocks_are_single_use(self):
        with patch("source5_navigation.time.monotonic",return_value=100):
            phase=PhaseBudget(150,False)
            with self.assertRaises(ValueError):
                phase.arrived()
            phase.capture()
            with self.assertRaises(ValueError):
                phase.capture()
            phase.arrived()
            with self.assertRaises(ValueError):
                phase.arrived()

    def test_prospective_post_capacity_is_protected_on_each_claim(self):
        ledger=FrameLedger()
        for _ in range(4): ledger.claim()
        before=(ledger.frames,ledger.bytes)
        with self.assertRaises(ValueError): ledger.claim()
        self.assertEqual((ledger.frames,ledger.bytes),before)
        ledger.reserve_creation()
        for _ in range(3): ledger.claim(reserved=True)
        self.assertEqual(ledger.frames,7)
        self.assertEqual(ledger.bytes,192*1024*1024)
        with self.assertRaises(ValueError): ledger.claim(reserved=True)
    def test_phase_admission_and_deadlines_do_not_renew(self):
        with patch("source5_navigation.time.monotonic",return_value=100):
            with self.assertRaises(ValueError): PhaseBudget(140,both=True)
            phase=PhaseBudget(141,both=True)
            self.assertEqual(phase.staging_end,103)
            self.assertEqual(phase.end,120)
        with patch("source5_navigation.time.monotonic",return_value=108):
            self.assertEqual(phase.capture(),118)
        with patch("source5_navigation.time.monotonic",return_value=117):
            self.assertEqual(phase.arrived(),119)
        with patch("source5_navigation.time.monotonic",return_value=119):
            with self.assertRaises(ValueError): phase.check(phase.processing_end)

# These images are explicitly synthetic identity/control fixtures, not tablet proof.
def image_bytes(kind):
    import io
    from PIL import Image,ImageDraw
    image=Image.new("RGB",(768,1024),"white")
    draw=ImageDraw.Draw(image)
    if kind=="library": draw.rectangle((90,100,600,900),fill="gray")
    if kind in {"source","source-footer"}:
        if kind=="source": draw.rectangle((90,100,600,900),fill="black"); draw.text((380,950),"2",fill="black")
        draw.rectangle((300,997,470,1005),fill="black")
    stream=io.BytesIO(); image.save(stream,format="PNG")
    return stream.getvalue()

def selection():
    from open_document_timing import SOURCE5,PRODUCTION_V2,V2_CONTRACT,PRESERVED_PAGES,FIXTURE_DOCUMENT
    return {"fixture_inventory_variant":SOURCE5,"experiment_kind":PRODUCTION_V2,
        "production_contract":copy.deepcopy(V2_CONTRACT),"nonce":"1"*32,
        "boot_id":"22222222-2222-2222-2222-222222222222","document_id":FIXTURE_DOCUMENT,
        "page_ids":list(PRESERVED_PAGES),"target_id":"33333333-3333-3333-3333-333333333333",
        "source_index":5,"source_page_id":PRESERVED_PAGES[5],"neighbor_index":6,
        "neighbor_page_id":PRESERVED_PAGES[6],"insertion_index":6,
        "immutable_hashes":{"/synthetic-protected-"+str(i):"a"*64 for i in range(26)}}

def positive_native():
    trial={key:value for key,value in {"enabled":True,"development_explicit_fixture":True,
        "mutation_attempted":True,"returned":True,"returned_bool_known":True,"returned_bool":True,
        "exception":False,"duplicate_callback":False,"durable_success":False,
        "effect_requires_reconciliation":True,"callback_count":1,"phase":"call-observed","guard_stage":"accepted",
        "attempted_at_ms":55533,"returned_at_ms":55536,"first_callback_at_ms":55624,"cleanup_return_ms":55640}.items()}
    return {"creation_trial":trial,"creation_after_open_gate":True,"creation_setup_cutoff_ms":95000,
        "access_anchor":"creation-arm-observation","access_budget_ms":5000,"creation_arm_accepted_at_ms":55530}

class RouteAdapter:
    """No-network raw-evidence supplier; actual public HostSequence owns decisions."""
    def __init__(self,selected,initial="source",change=None,remaining=80):
        self.selected=selected; self.initial=initial; self.change=change; self.remaining=remaining
        self.calls=[]; self.frame_ledger=FrameLedger(); self.arm_attempted=False
        self.root_inode="6 123"; self.production_window=selected["nonce"]+" 100 460 127 257"
        self.clock=130000; self.opens=0; self.stop_count=0
    def raw(self,stage,current=None):
        self.clock+=100
        nine=stage in {"result","normalization-source","inserted-destination"}
        ids=self.selected["page_ids"][:]
        if nine: ids.insert(6,self.selected["target_id"])
        current=5 if current is None else current
        pages=[]
        for index,page in enumerate(ids):
            old=self.selected["page_ids"].index(page) if page in self.selected["page_ids"] else None
            p={"id":page,"idx":{"value":"5a" if old is None else str(old)},"deleted":{"value":False}}
            if old==5: p["redir"]={"value":1}
            if old==6: p["redir"]={"value":2}
            pages.append(p)
        content={"pageCount":len(ids),"cPages":{"pages":pages,"lastOpened":{"value":ids[current]}}}
        metadata={"lastOpenedPage":current}
        kind=("library" if stage=="ready" else "source-footer" if stage=="open" and self.opens==1 else
              self.initial if stage=="result" else "blank" if stage=="inserted-destination" else "source")
        raw={"window":[self.selected["nonce"],100,460,127,257],"boot_id":self.selected["boot_id"],"pid":42,"start_ticks":561,
            "closed":False,"uptime":str(self.clock/1000)+" 0.00\n",
            "waiting":(self.selected["nonce"]+" 42 561 waiting 222000\n").encode(),
            "hashes":dict(self.selected["immutable_hashes"]),"last_open":None if stage=="ready" else self.selected["document_id"],
            "last_open_raw":b"LastOpen=@ByteArray()\n" if stage=="ready" else ("LastOpen=@ByteArray("+self.selected["document_id"]+")\n").encode(),
            "content":content,"metadata":metadata,"png":image_bytes(kind),"diagnostics":positive_native(),
            "response_deadline_monotonic":time.monotonic()+5,
            "idle":{"interval_ms":2000,"invalidated":False,"touch_active":False,"pen_contact":0,"pen_tool":0}}
        raw["content_raw_sha256"]=hashlib.sha256(json.dumps(content).encode()).hexdigest()
        raw["metadata_raw_sha256"]=hashlib.sha256(json.dumps(metadata).encode()).hexdigest()
        if self.change: self.change(stage,raw)
        return raw
    def retain(self,stage,evidence): self.calls.append("retain:"+stage)
    def wait_library(self):
        self.frame_ledger.claim(); return self.raw("ready")
    def open_once(self,*args): self.calls.append("open")
    def snapshot(self,stage,*args):
        self.frame_ledger.claim()
        if stage=="open": self.opens+=1
        return self.raw(stage)
    def reserve_navigation_frames(self): self.frame_ledger.reserve_creation()
    def arm_once(self,*args): self.arm_attempted=True; self.calls.append("arm")
    def result_snapshot(self,*args):
        self.frame_ledger.claim(reserved=True); return self.raw("result",6)
    def validate_creation_evidence(self,*args): return True
    def known_deadline(self,*args): return time.monotonic()+self.remaining
    def publish_reconciliation(self,*args): self.calls.append("publish-reconciliation")
    def begin_navigation(self,both=False): return PhaseBudget(self.known_deadline(),both=both)
    def navigation_move(self,direction,current,*args):
        self.calls.append(direction); return {"released":True}
    def navigation_snapshot(self,stage,*args):
        self.frame_ledger.claim(reserved=True)
        return self.raw(stage,5 if stage=="normalization-source" else 6)
    def retain_bounded(self,stage,*args): self.calls.append("retain:"+stage)
    def stop_once(self,*args): self.stop_count+=1; self.calls.append("stop")

class PublicRouteTests(unittest.TestCase):
    def run_route(self,**kwargs):
        selected=selection()
        adapter=RouteAdapter(selected,**kwargs)
        host=HostSequence(adapter,selected,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
        return host,adapter
    def test_actual_public_run_normalizes_then_verifies_target_and_stops_once(self):
        host,adapter=self.run_route()
        report=host.run()
        self.assertTrue(report["destination_verified"])
        self.assertTrue(report["normalization_performed"])
        self.assertEqual([c for c in adapter.calls if c in {"open","arm","previous","next","stop"}],
            ["open","arm","previous","next","stop"])
        self.assertEqual(adapter.frame_ledger.frames,7)
        self.assertEqual(adapter.stop_count,1)
    def test_initial_target_blank_needs_no_gesture(self):
        host,adapter=self.run_route(initial="blank")
        self.assertFalse(host.run()["normalization_performed"])
        self.assertNotIn("previous",adapter.calls); self.assertNotIn("next",adapter.calls)
        self.assertEqual(adapter.stop_count,1)
    def test_unknown_or_source_owned_initial_state_never_moves(self):
        for failure in ("source-owner","mixed","foreign","immutable","unknown-native","bad-pixels"):
            def change(stage,raw,failure=failure):
                if stage!="result": return
                if failure=="source-owner":
                    raw["metadata"]["lastOpenedPage"]=5; raw["content"]["cPages"]["lastOpened"]["value"]=selection()["page_ids"][5]
                elif failure=="mixed": raw["metadata"]["lastOpenedPage"]=5
                elif failure=="foreign": raw["content"]["cPages"]["lastOpened"]["value"]="foreign"
                elif failure=="immutable": raw["hashes"]={}
                elif failure=="unknown-native": raw["diagnostics"]["creation_trial"]["returned_bool_known"]=False
                else: raw["png"]=image_bytes("library")
            with self.subTest(failure=failure):
                host,adapter=self.run_route(change=change)
                with self.assertRaises((Refused,ValueError)): host.run()
                self.assertNotIn("previous",adapter.calls); self.assertNotIn("next",adapter.calls)
                self.assertEqual(adapter.stop_count,1)
    def test_late_initial_result_refuses_before_motion(self):
        host,adapter=self.run_route(remaining=40)
        with self.assertRaises(ValueError): host.run()
        self.assertNotIn("previous",adapter.calls); self.assertEqual(adapter.stop_count,1)
    def test_wrong_normalized_source_never_admits_next(self):
        def change(stage,raw):
            if stage=="normalization-source": raw["png"]=image_bytes("library")
        host,adapter=self.run_route(change=change)
        with self.assertRaises(Refused): host.run()
        self.assertIn("previous",adapter.calls); self.assertNotIn("next",adapter.calls)
        self.assertEqual(adapter.stop_count,1)
    def test_destination_mismatch_never_corrects_or_replays(self):
        def change(stage,raw):
            if stage=="inserted-destination": raw["png"]=image_bytes("source")
        host,adapter=self.run_route(change=change)
        with self.assertRaises(Refused): host.run()
        self.assertEqual(adapter.calls.count("previous"),1); self.assertEqual(adapter.calls.count("next"),1)
        self.assertEqual(adapter.stop_count,1)



def geometry_views():
    views={}
    for direction,sign,start in (("previous",1,100),("next",-1,700)):
        virtual=[[start+sign*(590*i//15),512] for i in range(16)]
        views[direction]={"direction":direction,"geometry_variant":"fixed-gap10-v1",
            "navigation_policy":"stop-at-declared-neighbor-v1","virtual_path":virtual,
            "native_path":[[x*1404//768,y*1872//1024] for x,y in virtual]}
    return views

def ssh_fixture(directory,transport,whole=False):
    from owned_frame_capture import TOOLS
    from source5_navigation import CONTROL_BYTES,canonical
    profile=selection()
    profile["gesture_paths"]=geometry_views()
    profile["gesture_path_hashes"]={f"gesture-paths-{d}.json":hashlib.sha256(canonical(v)).hexdigest()
        for d,v in profile["gesture_paths"].items()}
    profile["tools"]=TOOLS|{k:"b"*64 for k in ("owned_tap_guardian","observe_input","publish_creation_arm","owned_relative_gesture")}
    profile["encoder_hashes"]={k:"c"*64 for k in ("/usr/bin/openssl","/usr/lib/libssl.so.3","/usr/lib/libcrypto.so.3")}
    profile["source5_control_bytes_reserve"]=CONTROL_BYTES
    files={"host-profile.json":"d"*64,"owned_relative_gesture":"b"*64}
    profile["packet_file_sizes"]={"owned_relative_gesture":4000000}
    receipt={k:v for k,v in profile.items() if k in {"fixture_inventory_variant","experiment_kind","document_id",
        "source_index","source_page_id","neighbor_index","neighbor_page_id","insertion_index"}}
    receipt.update(remote_root="/run/rmb-qt-probe-"+profile["nonce"],before_page_ids=profile["page_ids"],files=files)
    if whole:
        profile["library_tap_variant"]="primary-1641396"
        receipt["library_tap_variant"]=profile["library_tap_variant"]
        for name in ("runtime.files","document-immutable.files"):
            files[name]="d"*64
            profile["packet_file_sizes"][name]=1024
    return SSHAdapter([],receipt,profile,directory,transport,image_bytes("library"))

class PublicAdapterTests(unittest.TestCase):
    def test_poll_limit_refuses_thirty_first_rpc_and_result_reentry(self):
        with tempfile.TemporaryDirectory() as directory:
            calls=[]
            adapter=ssh_fixture(directory,lambda *args: calls.append(args) or
                {"stdout":"","stderr":"","exit":0,"timeout":False})
            adapter.diagnostic_poll_count=29
            adapter.call("last read-only poll",record_role="poll")
            self.assertEqual(adapter.diagnostic_poll_count,30)
            used=adapter.control_ledger.used
            with self.assertRaises(Refused): adapter.call("extra poll",record_role="poll")
            self.assertEqual(len(calls),1)
            self.assertEqual(adapter.control_ledger.used,used)
            adapter.result_poll_started=True
            with self.assertRaises(Refused):
                SSHAdapter.result_snapshot(adapter,("boot",42,561),242)
            self.assertEqual(len(calls),1)

    def test_prearm_postnative_reservation_is_atomic_before_any_rpc(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=whole_fixture(directory)
            adapter.tap_released=True
            adapter.control_ledger.claim(FUTURE_CONTROL_BYTES-65536-adapter.control_ledger.used-163839)
            used=adapter.control_ledger.used
            with self.assertRaises(Refused): adapter.arm_once(None,None,None)
            self.assertFalse(adapter.arm_attempted)
            self.assertFalse(adapter.scripts)
            self.assertEqual(adapter.postnative_control,{})
            self.assertEqual(adapter.control_ledger.used,used)
            self.assertTrue(adapter.control_ledger.overflow)

    def test_reserved_stop_survives_exhausted_ordinary_and_unused_branch_tickets(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=phase_harness(directory)
            before=adapter.control_ledger.used
            adapter.reserve_postnative_control()
            self.assertEqual(adapter.control_ledger.used-before,163840)
            stage_tickets=list(adapter.postnative_control["staging-clock"])
            adapter.control_ledger.claim(FUTURE_CONTROL_BYTES-65536-adapter.control_ledger.used)
            with self.assertRaises(Refused): adapter.call("ordinary")
            helper_tickets=list(adapter.postnative_control["helper"])
            with self.assertRaises(Refused): adapter.call("forbidden helper",record_role="helper")
            self.assertEqual(adapter.postnative_control["helper"],helper_tickets)
            self.assertFalse(adapter.scripts)
            adapter.stop_once(adapter.identity,252)
            self.assertTrue(adapter.stop_attempted)
            self.assertEqual(adapter.postnative_control["stop"],[])
            self.assertEqual(adapter.postnative_control["stop-clock"],[])
            self.assertEqual(adapter.postnative_control["staging-clock"],stage_tickets)
            self.assertEqual(adapter.control_ledger.remaining,65536)

    def test_frame_claim_precedes_first_capture_context_clock_and_output(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=ssh_fixture(directory,lambda *args:None)
            def early(stage,cutoff,idle):
                self.assertEqual(adapter.frame_ledger.frames,1)
                self.assertEqual(adapter.active_capture_claim,("ready",1))
                self.assertTrue(adapter.capture_scope)
                adapter.call("early context",record_role="clock")
                raise Refused("synthetic context failure")
            adapter._raw_snapshot=early
            adapter.transport=lambda *args: {"stdout":"","stderr":"","exit":0,"timeout":False}
            with self.assertRaises(Refused): adapter.raw_snapshot("ready",150,True)
            self.assertEqual(adapter.frame_ledger.frames,1)
            self.assertEqual(adapter.control_ledger.used,SOURCE5_REMOTE_CONTROL_BYTES)
            self.assertTrue(adapter.capture_failed)
            self.assertFalse(adapter.capture_scope)
            self.assertIsNone(adapter.active_capture_claim)
            self.assertEqual(len(list(Path(directory).glob("transport-*.json"))),1)

    def test_invalid_stage_exhausted_quota_and_forged_scope_never_transport(self):
        with tempfile.TemporaryDirectory() as directory:
            calls=[]
            adapter=ssh_fixture(directory,lambda *args:calls.append(args))
            with self.assertRaises(Refused): adapter.raw_snapshot("invalid",150,True)
            self.assertEqual(adapter.frame_ledger.frames,0)
            adapter.capture_scope=True
            with self.assertRaises(Refused): adapter.call("unclaimed context")
            adapter.capture_scope=False
            for _ in range(4): adapter.frame_ledger.claim()
            with self.assertRaises(Refused): adapter.raw_snapshot("ready",150,True)
            self.assertEqual(adapter.frame_ledger.frames,4)
            self.assertFalse(calls)
            self.assertFalse(any(Path(directory).iterdir()))
            self.assertTrue(adapter.capture_failed)

    def test_phase_none_control_call_is_preclaimed_before_transport(self):
        with tempfile.TemporaryDirectory() as directory:
            seen=[]
            def transport(args,script,timeout):
                seen.append(adapter.control_ledger.used)
                return {"stdout":"observed","stderr":"","exit":0,"timeout":False}
            adapter=ssh_fixture(directory,transport)
            self.assertEqual(adapter.call("read-only synthetic command"),"observed")
            self.assertEqual(seen,[SOURCE5_REMOTE_CONTROL_BYTES+8192])
            self.assertEqual(adapter.control_ledger.used,SOURCE5_REMOTE_CONTROL_BYTES+8192)
    def test_phase_none_exhaustion_admits_no_transport(self):
        with tempfile.TemporaryDirectory() as directory:
            calls=[]
            adapter=ssh_fixture(directory,lambda *args:calls.append(args))
            adapter.control_ledger.claim(FUTURE_CONTROL_BYTES-65536-adapter.control_ledger.used)
            with self.assertRaises(Refused): adapter.call("read-only")
            self.assertFalse(calls)
            self.assertFalse(any(Path(directory).iterdir()))
    def test_ordinary_quota_exhaustion_preserves_guarded_stop_transport(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=phase_harness(directory)
            adapter.control_ledger.claim(FUTURE_CONTROL_BYTES-65536-adapter.control_ledger.used)
            with self.assertRaises(Refused):
                adapter.call("read-only")
            before=len(adapter.scripts)
            adapter.stop_once(adapter.identity,252)
            self.assertTrue(adapter.stop_attempted)
            self.assertEqual(len(adapter.scripts)-before,2)
            self.assertTrue(any("stop.request" in script for script in adapter.scripts[before:]))
            self.assertEqual(adapter.control_ledger.remaining,65536-2048-8192)
            self.assertTrue(adapter.control_ledger.overflow)

    def test_valid_consistent_profile_drift_is_rejected_before_transport(self):
        with tempfile.TemporaryDirectory() as directory:
            calls=[]
            adapter=ssh_fixture(directory,lambda *args:calls.append(args))
            adapter.profile["target_id"]="44444444-4444-4444-4444-444444444444"
            with self.assertRaises(Refused): adapter.call("read-only")
            self.assertFalse(calls)
    def test_stop_clears_expired_processing_phase_but_waits_original_helper_lease(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=ssh_fixture(directory,lambda *args:None)
            adapter.identity=("boot",42,561)
            adapter.navigation_phase=PhaseBudget(time.monotonic()+50,both=True)
            adapter.navigation_phase.processing_end=time.monotonic()-1
            adapter.pending_helper_deadline=time.monotonic()+0.05
            events=[]
            def budget(*args):
                self.assertIsNone(adapter.navigation_phase)
                self.assertIsNone(adapter.pending_helper_deadline)
                events.append("budget")
                return 1,time.monotonic()+1
            adapter.action_budget=budget
            adapter.prelude=lambda *args,**kw:""
            adapter.call=lambda *args,**kw:events.append("stop")
            adapter.stop_once(adapter.identity,125)
            self.assertEqual(events,["budget","stop"])
            self.assertTrue(adapter.stop_attempted)
    def test_terminal_report_validates_phase_map_and_typed_witnesses(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=ssh_fixture(directory,lambda *args:None)
            binding={"sealed":"synthetic"}
            report={"nonce":"a"*32,"nine_inventory":binding,"navigation_policy":"stop-at-declared-neighbor-v1",
                "geometry_variant":"fixed-gap10-v1","writer_death_confirmed":True,"owned_down_prefix":True,
                "release_positive":True,"unresolved":False,"protocol_uncertain":False,"ui_verified":False,
                "status":"released-after-target-observation-ui-unverified","complete_step":8,
                "trajectory_complete":False,"transition_unresolved":True,
                "target_observation":{"step":9,"samples":{"first":[5,6],"second":[5,6],"bracket_changed":False},
                    "witness_fields":[[False,True],[False,True]],"origin":"worker","phase":"context",
                    "parent_waitstate":"await-written","worker_phase":"context","worker_step":9}}
            adapter.validate_navigation_release(report,"a"*32,binding,5)
            for field,value in (("protocol_uncertain",True),("release_positive",False),("writer_death_confirmed",False)):
                broken=copy.deepcopy(report);broken[field]=value
                with self.assertRaises(Refused):adapter.validate_navigation_release(broken,"a"*32,binding,5)
            broken=copy.deepcopy(report);broken["target_observation"]["witness_fields"]=[[0,1],[0,1]]
            with self.assertRaises(Refused):adapter.validate_navigation_release(broken,"a"*32,binding,5)
    def test_closed_target_file_profile_refuses_absence_and_extra_paths(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            adapter=ssh_fixture(directory,lambda *args:None)
            adapter.target_absent_before_arm=True
            waiting=SimpleNamespace(nonce=adapter.profile["nonce"],pid=42,start_ticks=561,expiry_ms=222000)
            ink=adapter.doc+"/"+adapter.profile["target_id"]+".rm"
            thumb=adapter.doc+".thumbnails/"+adapter.profile["target_id"]+".png"
            raw={"creation_arm_raw":f"{waiting.nonce} 42 561 create 222000\n".encode(),
                "target_files":{"target-paths":ink+"\n"+thumb+"\n","target-ink.stat":"600 0 1 424\n",
                    "target-thumb.stat":"600 0 1 1706\n",
                    "target-ink.hash":"1dfdc53d0fb38c48758c6a0663df8182fb4a4b6f362f8b4b90cb02d5e7841462  "+ink+"\n",
                    "target-thumb.hash":"9dbb20c8af2e99438341e492f5dfd61d21a26db8c9ff78db545cf7e0bdcdd603  "+thumb+"\n"}}
            self.assertTrue(adapter.validate_creation_evidence(raw,waiting))
            for key,value in (("target-ink.stat","absent\n"),("target-thumb.stat","symlink\n"),
                ("target-paths",ink+"\n"+thumb+"\n"+ink+".extra\n")):
                broken=copy.deepcopy(raw);broken["target_files"][key]=value
                with self.assertRaises(ValueError):adapter.validate_creation_evidence(broken,waiting)


def capture_response(adapter,raw):
    import base64
    def b(value): return base64.b64encode(value if isinstance(value,bytes) else str(value).encode()).decode()
    png=raw["png"];frame_hash="e"*64;native_hash="f"*64
    receipt=f"nonce={adapter.profile['nonce']} frame_sha256={frame_hash} native_sha256={native_hash} overview_sha256={hashlib.sha256(png).hexdigest()} conversion_complete=true native_authority=false\n"
    encoded={"pid":42,"start_ticks":561,"root_inode":"6 123","window":b(adapter.production_window+"\n"),
        "waiting":b(raw["waiting"]),"capture_uptime":b(raw["uptime"]),"uptime":b(raw["uptime"]),"last_open":b(raw["last_open_raw"]),
        "frame_hash":frame_hash,"native_hash":native_hash,"capture_receipt":b(receipt),
        "content":b(json.dumps(raw["content"])),"metadata":b(json.dumps(raw["metadata"])),"png":b(png),
        "hashes":b("".join(v+"  "+k+"\n" for k,v in raw["hashes"].items())),
        "idle":b("kernel slots 0..31 selected 0\nobserver ready; owned_modifier=false; 2s\n"),
        "diagnostics":b(json.dumps(raw["diagnostics"])),"config_raw":b(raw["last_open_raw"])}
    ink=adapter.doc+"/"+adapter.profile["target_id"]+".rm"
    thumb=adapter.doc+".thumbnails/"+adapter.profile["target_id"]+".png"
    encoded.update({"creation_arm_raw":b(f"{adapter.profile['nonce']} 42 561 create 222000\n"),
        "target-ink.stat":b("600 0 1 424\n"),"target-thumb.stat":b("600 0 1 1706\n"),
        "target-ink.hash":b("1dfdc53d0fb38c48758c6a0663df8182fb4a4b6f362f8b4b90cb02d5e7841462  "+ink+"\n"),
        "target-thumb.hash":b("9dbb20c8af2e99438341e492f5dfd61d21a26db8c9ff78db545cf7e0bdcdd603  "+thumb+"\n"),
        "target-paths":b(ink+"\n"+thumb+"\n")})
    return json.dumps(encoded)

class PhaseHarness(SSHAdapter):
    """Existing pre-insertion fixtures, real new SSHAdapter post-insertion path."""
    def initialize_fixture(self):
        self.route=RouteAdapter(self.profile)
        self.route.frame_ledger=self.frame_ledger
        self.clock=130.0;self.configs=[];self.scripts=[]
        self.candidate_remote_at=127
        self.candidate_local_anchor=time.monotonic()-3
        self.production_window=self.profile["nonce"]+" 100 460 127 257"
        self.identity=(self.profile["boot_id"],42,561)
        self.root_inode="6 123"
        self.sdk_expiry_ms=222000
        self.target_absent_before_arm=True
        self.failure=None
    def wait_library(self): return self.route.wait_library()
    def open_once(self,*args): self.route.open_once(*args)
    def snapshot(self,*args): return self.route.snapshot(*args)
    def reserve_navigation_frames(self): self.frame_ledger.reserve_creation()
    def arm_once(self,*args): self.arm_attempted=True
    def result_snapshot(self,*args):
        from open_document_transport import decode_snapshot
        raw=self.route.result_snapshot(*args)
        result=decode_snapshot(capture_response(self,raw),self.profile["nonce"],False,None,
            self.root_inode,time.monotonic()+5,True,self.profile["boot_id"],True)
        result["capture_prefix"]="host-result"
        self.capture_sources["host-result"]={"claim":self.frame_ledger.frames,"phase":"result",
            "sha256":hashlib.sha256(result["png"]).hexdigest(),"root_inode":self.root_inode}
        return result
    def transport_fixture(self,args,script,timeout):
        import re,shlex
        self.scripts.append(script);self.clock+=0.1
        stdout=""
        if script=="set -eu\ncat /proc/uptime\n":
            stdout=f"{self.clock:.2f} 0.00\n"
        elif 'mkdir -m 700 "$gesture_root"' in script:
            match=re.search(r"printf %s '(\{.*\})' \| sed",script)
            if not match: raise AssertionError("Actual config serialization absent")
            config=json.loads(match[1]);self.configs.append(config)
            nonce=config["nonce"]
            registration={"nonce":nonce,"root":"/run/rmb-gesture-"+nonce,"dev":6,"ino":999,
                "owner_sha256":hashlib.sha256(nonce.encode()).hexdigest(),"phase":config["direction"],
                "candidate_nonce":self.profile["nonce"]}
            stdout=json.dumps({"registration":registration,"remote_uptime":f"{self.clock:.2f}"})
        elif 'nohup "$root/owned_relative_gesture"' in script:
            config=self.configs[-1];current=config["current_index"];target=5 if current==6 else 6
            if self.failure=="helper": return {"stdout":"","stderr":"uncertain","timeout":True,"exit":None}
            report={"nonce":config["nonce"],"nine_inventory":config["nine_inventory"],
                "geometry_variant":"fixed-gap10-v1","navigation_policy":"stop-at-declared-neighbor-v1",
                "writer_death_confirmed":True,"owned_down_prefix":True,"release_positive":True,
                "unresolved":False,"protocol_uncertain":False,"ui_verified":False,
                "status":"released-after-target-observation-ui-unverified","complete_step":8,
                "trajectory_complete":False,"transition_unresolved":True,
                "target_observation":{"step":9,"origin":"parent","phase":"before-intent","parent_waitstate":"await-request",
                    "worker_phase":None,"worker_step":None,"samples":{"first":[target,target],"second":[target,target],"bracket_changed":False},
                    "witness_fields":[[True,True],[True,True]]}}
            stdout=json.dumps(report)
        elif 'native_frame_acquire" "$pid" "$start"' in script:
            stage="normalization-source" if 'host-normalization-source.claim' in script else "inserted-destination"
            raw=self.route.raw(stage,5 if stage=="normalization-source" else 6)
            stdout=capture_response(self,raw)
        return {"stdout":stdout,"stderr":"","timeout":False,"exit":0}

def phase_harness(directory):
    adapter=ssh_fixture(directory,lambda *args:None)
    adapter.__class__=PhaseHarness
    adapter.initialize_fixture()
    adapter.transport=adapter.transport_fixture
    return adapter

class WholeRouteHarness(PhaseHarness):
    """Only SSH answers are synthetic; all public production methods are real."""
    wait_library=SSHAdapter.wait_library
    open_once=SSHAdapter.open_once
    snapshot=SSHAdapter.snapshot
    reserve_navigation_frames=SSHAdapter.reserve_navigation_frames
    arm_once=SSHAdapter.arm_once
    result_snapshot=SSHAdapter.result_snapshot

    def transport_fixture(self,args,script,timeout):
        import base64
        def b(data): return base64.b64encode(data).decode()
        def answered(value):
            self.scripts.append(script)
            self.clock+=0.1
            return {"stdout":value,"stderr":"","exit":0,"timeout":False}
        if 'then cat "$root/candidate.window";' in script:
            return answered(self.profile["nonce"]+" 100 460 127 257\n"+f"{self.clock:.2f}"+" 0.00\n")
        if 'native_frame_acquire" "$pid" "$start"' in script:
            if "host-ready-000.claim" in script: stage="ready"
            elif "host-open-" in script:
                stage="open"
                self.route.opens+=1
            elif "host-trigger.claim" in script: stage="trigger"
            elif "host-result.claim" in script: stage="result"
            elif "host-normalization-source.claim" in script: stage="normalization-source"
            else: stage="inserted-destination"
            raw=self.route.raw(stage,6 if stage in {"result","inserted-destination"} else 5)
            if stage not in {"result","normalization-source","inserted-destination"}:
                raw["diagnostics"]={}
            if stage=="ready":
                raw["last_open"]=None
                raw["last_open_raw"]=b"LastOpen=@ByteArray()\n"
            return answered(capture_response(self,raw))
        if 'tap_cutoff_ms=$(' in script:
            self.tap_fixture={"nonce":self.profile["nonce"],"boot_id":self.profile["boot_id"],
                "pid":88,"start_ticks":777,"slot":0,"tracking_id":123,
                "native_point":[164,1396],"lease_end_ms":int((self.clock+12)*1000),
                "status":"guardian-ready-no-down","exclusive_kernel_ownership":False}
            return answered(json.dumps({"ready":b(json.dumps(self.tap_fixture).encode()+b"\n"),
                "setup":b(b"synthetic guardian setup\n"),"selected_cutoff_ms":int((self.clock+14)*1000),
                "uptime":b((f"{self.clock:.2f}"+" 0.00\n").encode())}))
        if '--publish-go' in script:
            released=self.tap_fixture|{"status":"guardian-released"}
            supervisor=released|{"status":"normal-child-exit"}
            return answered(json.dumps({"released":b(json.dumps(released).encode()+b"\n"),
                "supervisor":b(json.dumps(supervisor).encode()+b"\n"),"lease":b(b"synthetic lease\n"),
                "publication":b(json.dumps({"nonce":self.profile["nonce"],"published":True,
                    "retry_permitted":False,"input_events":0}).encode()+b"\n")}))
        if 'control_bytes=0' in script:
            return answered('{"control_bytes":4000000}')
        if '--publish "$root/publish_creation_arm"' in script:
            return answered('{"published":true,"retry_permitted":false}')
        if 'then cat "$root/diagnostics.json"; fi' in script:
            return answered(json.dumps(positive_native()))
        return super().transport_fixture(args,script,timeout)

def whole_fixture(directory):
    adapter=ssh_fixture(directory,lambda *args:None,whole=True)
    adapter.__class__=WholeRouteHarness
    adapter.initialize_fixture()
    adapter.production_window=None
    adapter.candidate_remote_at=None
    adapter.candidate_local_anchor=None
    adapter.sdk_expiry_ms=None
    adapter.identity=None
    adapter.root_inode=None
    adapter.target_absent_before_arm=False
    adapter.transport=adapter.transport_fixture
    return adapter

class WholePublicRouteTests(unittest.TestCase):
    def test_guardian_without_accepted_ready_never_launches_or_writes(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=whole_fixture(directory)
            with self.assertRaises(Refused):
                adapter.open_once(("boot",42,561),180)
            self.assertFalse(adapter.tap_attempted)
            self.assertIsNone(adapter.guardian_source_claim)
            self.assertFalse(adapter.scripts)
            self.assertFalse(any(Path(directory).iterdir()))

    def test_ambiguous_tap_keeps_ready_assignment_and_never_arms_native(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=whole_fixture(directory)
            original=adapter.transport_fixture
            def fail(args,script,timeout):
                if 'tap_cutoff_ms=$(' in script:
                    adapter.scripts.append(script)
                    return {"stdout":"","stderr":"uncertain tap launch","exit":None,"timeout":True}
                return original(args,script,timeout)
            adapter.transport=fail
            host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
            with self.assertRaises(Refused): host.run()
            self.assertEqual(adapter.frame_ledger.frames,1)
            self.assertEqual(adapter.guardian_source_claim,("host-ready-000",1))
            self.assertTrue(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)
            self.assertTrue(adapter.stop_attempted)
            self.assertFalse(any("--publish-go" in script for script in adapter.scripts))
            self.assertFalse(any('--publish "$root/publish_creation_arm"' in script for script in adapter.scripts))

    def test_real_ready_open_trigger_creation_and_both_navigation_phases(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=whole_fixture(directory)
            host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
            report=host.run()
            self.assertTrue(report["destination_verified"])
            self.assertEqual(adapter.frame_ledger.frames,7)
            self.assertEqual(adapter.trigger_source_claim,("host-trigger",4))
            self.assertEqual(adapter.frame_ledger.bytes,192*1024*1024)
            self.assertEqual(adapter.capture_prefixes,["host-ready-000","host-open-000","host-open-001",
                "host-trigger","host-result","host-normalization-source","host-inserted-destination"])
            self.assertEqual(adapter.host_record_counts[1],3)
            self.assertEqual(adapter.guardian_source_claim,("host-ready-000",1))
            self.assertTrue(adapter.tap_released)
            self.assertTrue(adapter.arm_attempted)
            self.assertTrue(adapter.stop_attempted)
            self.assertEqual(adapter.diagnostic_poll_count,1)
            self.assertTrue(adapter.result_poll_started)
            self.assertTrue(all(not tickets for tickets in adapter.postnative_control.values()))
            self.assertEqual([c["direction"] for c in adapter.configs],["previous","next"])
            self.assertLess(adapter.control_ledger.used,FUTURE_CONTROL_BYTES-65536)
            for name in ("tap-ready","tap-go-publication","tap-released"):
                self.assertTrue((Path(directory)/(name+".json")).is_file())

class GeneratedShellTests(unittest.TestCase):
    @unittest.skipUnless(os.name=="posix","Actual POSIX map execution requires Linux")
    def test_actual_map_cap_retains_overflow_sentinel_and_refuses_it(self):
        from owned_frame_capture import capture_script
        text=capture_script("host-normalization-source","1"*32,
            "22222222-2222-2222-2222-222222222222",limited=True)
        line=next(line for line in text.splitlines() if line.startswith("dd if=/proc/$pid/maps bs=1 count=262145 2>/dev/null"))
        for size in (262143,262144,262145,524288):
            with self.subTest(size=size), tempfile.TemporaryDirectory() as directory:
                root=Path(directory)
                (root/"maps-input").write_bytes(b"x"*size)
                script='root='+directory+'\n'+line.replace("/proc/$pid/maps",'"$root/maps-input"')+'\n'+\
                    'test "$(stat -c %s "$root/host-normalization-source.maps-before")" -le 262144\n'
                result=subprocess.run(["sh","-c",script],capture_output=True,timeout=5)
                self.assertEqual(result.returncode,0 if size<=262144 else 1)
                self.assertEqual((root/"host-normalization-source.maps-before").stat().st_size,min(size,262145))

    @unittest.skipUnless(os.name=="posix","Actual POSIX pipeline execution requires Linux")
    def test_actual_generated_pipelines_keep_failure_and_overflow_status(self):
        from owned_frame_capture import capture_script
        text=capture_script("host-normalization-source","1"*32,
            "22222222-2222-2222-2222-222222222222",limited=True)
        for kind,executable,limit in (("producer","native_frame_acquire",525825),
                                      ("converter","owned_frame_alarm",65537)):
            line=next(line for line in text.splitlines() if line.startswith('( "$root/'+executable+'"'))
            for size,exit_code,signal in ((0,0,False),(0,17,False),(limit*2,0,False),(0,0,True)):
                with self.subTest(kind=kind,size=size,exit_code=exit_code,signal=signal), tempfile.TemporaryDirectory() as directory:
                    root=Path(directory)
                    fake=root/executable
                    fake.write_text("#!/bin/sh\nhead -c "+str(size)+" /dev/zero >&2\n"+
                        ("kill -PIPE $$\n" if signal else "exit "+str(exit_code)+"\n"))
                    fake.chmod(0o700)
                    script='root='+directory+'\npid=42\nstart=561\nframe_hash='+"f"*64+'\nset +e\n'+line+'\n'
                    result=subprocess.run(["sh","-c",script],capture_output=True,timeout=5)
                    self.assertEqual(result.returncode,0)
                    status=int((root/("host-normalization-source."+kind+"-exit")).read_text())
                    retained=(root/("host-normalization-source."+kind+"-stderr")).stat().st_size
                    self.assertLessEqual(retained,limit)
                    if size:
                        self.assertEqual(retained,limit)
                    elif signal:
                        self.assertNotEqual(status,0)
                    else:
                        self.assertEqual(status,exit_code)

    def test_capture_caps_preserve_exit_and_overflow_sentinel_grammar(self):
        from owned_frame_capture import capture_script
        text=capture_script("host-normalization-source","1"*32,
            "22222222-2222-2222-2222-222222222222",limited=True)
        self.assertEqual(text.count("dd if=/proc/$pid/maps bs=1 count=262145 2>/dev/null"),2)
        self.assertNotIn('cat /proc/$pid/maps >',text)
        self.assertIn("dd bs=1 count=525825 2>/dev/null",text)
        self.assertIn("dd bs=1 count=65537 2>/dev/null",text)
        self.assertIn('producer=$(cat "$root/host-normalization-source.producer-exit")',text)
        self.assertIn('converter=$(cat "$root/host-normalization-source.converter-exit")',text)
        self.assertIn('test "$producer" = 0',text)
        self.assertIn('test "$converter" = 0',text)
        self.assertIn('-le 65536',text)
        for invalid in ("host-result-2","host-destination","../host-result"):
            with self.assertRaises(ValueError):
                capture_script(invalid,"1"*32,"22222222-2222-2222-2222-222222222222",limited=True)

class CompleteAdapterPhaseTests(unittest.TestCase):
    def test_public_host_run_uses_actual_post_native_transport_and_shared_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=phase_harness(directory)
            host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
            report=host.run()
            self.assertTrue(report["destination_verified"])
            self.assertEqual([c["direction"] for c in adapter.configs],["previous","next"])
            launches=[s for s in adapter.scripts if 'nohup "$root/owned_relative_gesture"' in s]
            self.assertEqual(len(launches),2)
            self.assertEqual(len(adapter.helper_source_claims),2)
            self.assertTrue(any('cp "$root/host-result.png" "$gesture_root/source.png"' in script for script in adapter.scripts))
            self.assertTrue(any('cp "$root/host-normalization-source.png" "$gesture_root/source.png"' in script for script in adapter.scripts))
            self.assertFalse(any("source.png" in script and base64_payload in script for script in adapter.scripts
                for base64_payload in [__import__("base64").b64encode(image_bytes("source")).decode()]))
            self.assertTrue(all('< /dev/null' in s for s in launches))
            self.assertTrue(all('--source5-capped "$gesture_root"' in s for s in launches))
            import re
            self.assertFalse(any(re.search(r"\bcp\s[^\n]*owned_relative_gesture",s) for s in adapter.scripts))
            self.assertTrue(adapter.stop_attempted)
            self.assertIsNone(adapter.pending_helper_deadline)
            for config in adapter.configs:
                self.assertLess(config["overall_ceiling_ms"],242000)
                self.assertEqual(config["overall_ceiling_ms"],config["go_not_after_ms"]+5000)
                self.assertEqual(config["nine_inventory"]["candidate_window_sha256"],
                    hashlib.sha256((adapter.production_window+"\n").encode()).hexdigest())
            self.assertLess(adapter.control_ledger.used,FUTURE_CONTROL_BYTES-65536)
    def test_changed_source_frame_refuses_before_staging_transport(self):
        for field,value in (("capture_prefix","host-normalization-source"),("png",b"changed")):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                adapter=phase_harness(directory)
                original=adapter.result_snapshot
                def changed(*args):
                    raw=original(*args)
                    raw[field]=value
                    return raw
                adapter.result_snapshot=changed
                host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
                with self.assertRaises((Refused,ValueError)):
                    host.run()
                self.assertEqual(adapter.configs,[])
                self.assertEqual(adapter.helper_source_claims,set())
                self.assertTrue(adapter.stop_attempted)

    def test_failed_staging_keeps_source_claim_and_never_launches_helper(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=phase_harness(directory)
            original=adapter.transport_fixture
            def failed(args,script,timeout):
                if 'mkdir -m 700 "$gesture_root"' in script:
                    adapter.scripts.append(script)
                    return {"stdout":"","stderr":"staging uncertain","timeout":True,"exit":None}
                return original(args,script,timeout)
            adapter.transport=failed
            host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
            with self.assertRaises(Refused):
                host.run()
            self.assertEqual(len(adapter.helper_source_claims),1)
            self.assertFalse(any('nohup "$root/owned_relative_gesture"' in script for script in adapter.scripts))
            self.assertTrue(adapter.stop_attempted)

    def test_oversize_serialized_config_refuses_before_remote_root_output(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=phase_harness(directory)
            host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
            from source5_navigation import canonical as actual
            def oversize(value):
                data=actual(value)
                return data+b" "*16384 if isinstance(value,dict) and "go_not_after_ms" in value else data
            with patch("open_document_transport.canonical",side_effect=oversize):
                with self.assertRaises(Refused): host.run()
            self.assertEqual(adapter.configs,[])
            self.assertFalse(any('mkdir -m 700 "$gesture_root"' in script for script in adapter.scripts))
            self.assertEqual(len(adapter.helper_source_claims),1)
            self.assertTrue(adapter.stop_attempted)

    def test_ambiguous_helper_preserves_original_lease_and_never_launches_next(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=phase_harness(directory);adapter.failure="helper"
            host=HostSequence(adapter,adapter.profile,image_bytes("library"),image_bytes("source"),image_bytes("source-footer"))
            with self.assertRaises(Refused): host.run()
            self.assertEqual([c["direction"] for c in adapter.configs],["previous"])
            self.assertTrue(adapter.stop_attempted)
            self.assertIsNone(adapter.pending_helper_deadline)

if __name__=="__main__":
    unittest.main()
