"""Offline sequencing, pixels, context and uncertainty cases. No transport."""
import copy
import io
import hashlib
import json
import unittest
from unittest.mock import patch
from PIL import Image, ImageDraw
from open_document_host import HostSequence, Refused


def png(library=False, body=True, footer=True):
    image = Image.new("RGB", (768, 1024), "white")
    d = ImageDraw.Draw(image)
    if body: d.rectangle((90, 100, 600, 900), fill="gray" if library else "black")
    if footer: d.rectangle((300, 997, 470, 1005), fill="black")
    out = io.BytesIO(); image.save(out, format="PNG"); return out.getvalue()


class Adapter:
    def __init__(self, selected):
        self.selected = selected
        self.calls = []
        self.change = None
        self.arm_error = False
        self.native_pages = False
        self.open_frames = 0
        self.ready_frames = 0

    def raw(self, stage):
        s = self.selected
        content = {"pageCount": 7, "cPages": {"pages": [{"id": p} for p in s["page_ids"]], "lastOpened": {"value": s["page_ids"][0]}}}
        if stage == "result" and self.native_pages:
            content["cPages"]["pages"].insert(1, {"id": s["target_id"]})
            content["pageCount"] = 8
        raw = {"window": [s["nonce"], 100, 460, 127, 197], "boot_id": s["boot_id"], "pid": 42, "start_ticks": 561,
               "closed": False, "uptime": {"ready": f"140.{self.ready_frames:02} 0.00\n", "open": "150.00 0.00\n", "trigger": "155.00 0.00\n", "result": "170.00 0.00\n"}[stage],
               "waiting": (s["nonce"] + " 42 561 waiting 167000\n").encode(), "hashes": dict(s["immutable_hashes"]),
               "last_open": s["document_id"], "content": content, "metadata": {"lastOpenedPage": 0},
               "png": png(stage == "ready"), "idle": {"interval_ms": 2000, "invalidated": False, "touch_active": False, "pen_contact": 0, "pen_tool": 0},
               "diagnostics": {"creation_trial": {"returned_bool": True, "callback_count": 1}}}
        raw["content_raw_sha256"] = hashlib.sha256(json.dumps(content).encode()).hexdigest()
        raw["metadata_raw_sha256"] = hashlib.sha256(json.dumps(raw["metadata"]).encode()).hexdigest()
        if stage == "open":
            raw["uptime"] = f"150.{self.open_frames:02} 0.00\n"
        if self.change: self.change(stage, raw)
        return raw

    def retain(self, stage, evidence): self.calls.append("retain:" + stage)
    def wait_library(self):
        self.calls.append("wait"); self.ready_frames += 1
        return self.raw("ready")
    def open_once(self, identity, cutoff): self.calls.append("tap")
    def snapshot(self, stage, cutoff, idle):
        self.calls.append("snapshot:" + stage)
        if stage == "open": self.open_frames += 1
        if stage == "ready": self.ready_frames += 1
        return self.raw(stage)
    def arm_once(self, waiting, timing, ready):
        self.calls.append("arm")
        if self.arm_error: raise OSError("transport outcome uncertain")
    def result_snapshot(self, identity, cutoff): self.calls.append("result"); return self.raw("result")
    def stop_once(self, identity, cutoff): self.calls.append("stop")


class HostTests(unittest.TestCase):
    def setUp(self):
        self.selection = {"nonce": "1" * 32, "boot_id": "boot", "document_id": "document",
                          "page_ids": ["source", "stroked", "third", "fourth", "fifth", "sixth", "seventh"],
                          "target_id": "new", "immutable_hashes": {"stroked.rm": "sha"}}
        self.adapter = Adapter(self.selection)
        self.host = HostSequence(self.adapter, self.selection, png(True), png(footer=False), png(body=False))

    def test_one_open_arm_and_stop_collect_same_candidate_before_stop(self):
        self.adapter.native_pages = True
        report = self.host.run()
        for call in ("tap", "arm", "stop"):
            self.assertEqual(self.adapter.calls.count(call), 1)
        self.assertLess(self.adapter.calls.index("result"), self.adapter.calls.index("stop"))
        self.assertTrue(report["observed_target_presence"])
        self.assertEqual(report["observed_page_ids"], report["expected_inserted_order"])
        self.assertIn("not inferred", report["native_success"])

    def test_matching_native_bool_does_not_invent_disk_or_view_success(self):
        report = self.host.run()
        self.assertFalse(report["observed_target_presence"])
        self.assertEqual(len(report["observed_page_ids"]), 7)
        self.assertIn("not inferred", report["native_success"])

    def test_library_banner_or_tile_pixel_ambiguity_prevents_tap(self):
        def change(stage, raw):
            if stage == "ready": raw["png"] = png()
        self.adapter.change = change
        with self.assertRaises(Refused): self.host.run()
        self.assertNotIn("tap", self.adapter.calls)
        self.assertNotIn("arm", self.adapter.calls)
        self.assertEqual(self.adapter.calls.count("stop"), 1)
        self.assertEqual(self.adapter.ready_frames, 3)

    def test_complete_nonmatching_observations_then_library_bind_first_and_retain_all(self):
        def change(stage, raw):
            if stage == 'ready' and self.adapter.ready_frames < 3:
                raw['png'] = png()
        self.adapter.change = change
        self.host.run()
        self.assertEqual(self.adapter.ready_frames, 3)
        for index in range(3):
            self.assertIn(f'retain:ready-frame-{index:03}', self.adapter.calls)
        self.assertEqual(self.adapter.calls.count('tap'), 1)
        self.assertEqual(self.host.identity, ('boot', 42, 561))

    def test_ready_faults_are_terminal_before_any_next_observation_or_tap(self):
        mutations = [lambda r: r['hashes'].update({'stroked.rm': 'changed'}),
                     lambda r: r['idle'].update(invalidated=True),
                     lambda r: r.update(uptime='147.00 0.00\n'),
                     lambda r: r.update(png=b'invalid PNG')]
        for mutate in mutations:
            adapter = Adapter(self.selection)
            def change(stage, raw):
                if stage == 'ready':
                    raw['png'] = png()
                    mutate(raw)
            adapter.change = change
            with self.assertRaises(Refused):
                HostSequence(adapter, self.selection, png(True), png(footer=False), png(body=False)).run()
            self.assertEqual(adapter.ready_frames, 1)
            self.assertNotIn('tap', adapter.calls)

    def test_later_library_cannot_rebind_identity_window_or_waiting(self):
        mutations = [lambda r: r.update(start_ticks=562),
                     lambda r: r['window'].__setitem__(3, 128),
                     lambda r: r.update(waiting=(self.selection['nonce'] + ' 42 561 waiting 168000\n').encode()),
                     lambda r: r.update(closed=True),
                     lambda r: r['hashes'].update({'stroked.rm': 'changed'}),
                     lambda r: r['idle'].update(pen_contact=1)]
        for mutate in mutations:
            adapter = Adapter(self.selection)
            def change(stage, raw):
                if stage == 'ready':
                    if adapter.ready_frames == 1:
                        raw['png'] = png()
                    else:
                        mutate(raw)
            adapter.change = change
            host = HostSequence(adapter, self.selection, png(True), png(footer=False), png(body=False))
            with self.assertRaises(Refused): host.run()
            self.assertEqual(adapter.ready_frames, 2)
            self.assertEqual(host.identity, ('boot', 42, 561))
            self.assertNotIn('tap', adapter.calls)

    def test_ready_transport_or_retention_failure_never_acquires_again(self):
        self.adapter.change = lambda stage, raw: raw.update(png=png()) if stage == 'ready' else None
        def failed_snapshot(stage, cutoff, idle):
            self.adapter.calls.append('failed-snapshot')
            raise Refused('partial response')
        self.adapter.snapshot = failed_snapshot
        with self.assertRaises(Refused): self.host.run()
        self.assertEqual(self.adapter.calls.count('failed-snapshot'), 1)
        self.assertNotIn('tap', self.adapter.calls)

    def test_host_retention_or_pixels_cannot_admit_after_local_snapshot_deadline(self):
        for times in ([10], [9, 10], [9, 9, 10]):
            with self.subTest(times=times):
                adapter = Adapter(self.selection)
                adapter.change = lambda stage, raw: raw.update(response_deadline_monotonic=10) if stage == 'ready' else None
                with patch('open_document_host.time.monotonic', side_effect=times), self.assertRaises(Refused):
                    HostSequence(adapter, self.selection, png(True), png(footer=False), png(body=False)).run()
                self.assertEqual(adapter.ready_frames, 1)
                self.assertNotIn('tap', adapter.calls)

    def test_wrong_open_does_not_retry_tap_or_arm(self):
        def change(stage, raw):
            if stage == "open": raw["last_open"] = "tutorial"
        self.adapter.change = change
        with self.assertRaises(Refused): self.host.run()
        self.assertEqual(self.adapter.calls.count("tap"), 1)
        self.assertNotIn("arm", self.adapter.calls)

    def test_pid_start_drift_order_and_nonblank_ink_change_prevent_arm(self):
        mutations = [lambda r: r.update(start_ticks=562),
                     lambda r: r["content"]["cPages"]["pages"].reverse(),
                     lambda r: r["hashes"].update({"stroked.rm": "changed"})]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                adapter = Adapter(self.selection)
                adapter.change = lambda stage, raw: mutate(raw) if stage == "trigger" else None
                host = HostSequence(adapter, self.selection, png(True), png(footer=False), png(body=False))
                with self.assertRaises(Refused): host.run()
                self.assertNotIn("arm", adapter.calls)
                self.assertEqual(adapter.calls.count("stop"), 1)

    def test_interval_expiry_and_held_input_prevent_arm(self):
        mutations = [lambda r: r.update(uptime="168.00 0.00\n"),
                     lambda r: r["idle"].update(invalidated=True),
                     lambda r: r["idle"].update(pen_contact=1)]
        for mutate in mutations:
            adapter = Adapter(self.selection)
            adapter.change = lambda stage, raw: mutate(raw) if stage == "trigger" else None
            with self.assertRaises(Refused): HostSequence(adapter, self.selection, png(True), png(footer=False), png(body=False)).run()
            self.assertNotIn("arm", adapter.calls)

    def test_uncertain_publication_is_never_replayed_and_requests_stop(self):
        self.adapter.arm_error = True
        with self.assertRaises(OSError): self.host.run()
        self.assertEqual(self.adapter.calls.count("arm"), 1)
        self.assertEqual(self.adapter.calls.count("stop"), 1)
        self.assertNotIn("result", self.adapter.calls)

    def test_unknown_or_nonportrait_pixels_refuse(self):
        with self.assertRaises(Refused): HostSequence(self.adapter, self.selection, b"unknown", png(), png())

    def test_loading_footer_then_loaded_body_without_footer(self):
        def change(stage, raw):
            if stage == "open": raw["png"] = png(body=False) if self.adapter.open_frames==1 else png(footer=False)
            if stage == "trigger": raw["png"] = png(footer=False)
        self.adapter.change=change
        self.host.run()
        self.assertEqual(self.adapter.open_frames,2)
        self.assertLess(self.adapter.calls.index("retain:source-footer-observed"),self.adapter.calls.index("retain:source-body-ready"))
        self.assertLess(self.adapter.calls.index("retain:source-body-ready"),self.adapter.calls.index("arm"))
        self.assertEqual(self.adapter.calls.count("tap"),1)

    def test_missing_footer_or_body_stops_after_bounded_frames(self):
        for missing in ("footer","body"):
            adapter=Adapter(self.selection)
            adapter.change=lambda stage,raw: raw.update(png=png(footer=False) if missing=="footer" else png(body=False)) if stage=="open" else None
            with self.assertRaises(Refused): HostSequence(adapter,self.selection,png(True),png(footer=False),png(body=False)).run()
            self.assertEqual(adapter.open_frames,5)
            self.assertNotIn("arm",adapter.calls)
            self.assertEqual(adapter.calls.count("tap"),1)

    def test_open_poll_continuity_and_fixed_deadline_failures(self):
        mutations=[lambda r:r.update(uptime="150.01 0.00\n"),
                   lambda r:r.update(uptime="162.00 0.00\n"),
                   lambda r:r.update(start_ticks=562),lambda r:r.update(closed=True),
                   lambda r:r["window"].__setitem__(3,128),
                   lambda r:r.update(content_raw_sha256="a"*64),
                   lambda r:r.update(metadata_raw_sha256="b"*64),
                   lambda r:r["hashes"].update({"stroked.rm":"changed"}),
                   lambda r:r.update(waiting=(self.selection["nonce"]+" 42 561 waiting 168000\n").encode())]
        for mutate in mutations:
            adapter=Adapter(self.selection)
            adapter.change=lambda stage,raw: mutate(raw) if stage=="open" and adapter.open_frames==2 else None
            with self.assertRaises(Refused): HostSequence(adapter,self.selection,png(True),png(footer=False),png(body=False)).run()
            self.assertNotIn("arm",adapter.calls)
            self.assertEqual(adapter.calls.count("tap"),1)

    def test_trigger_sidecar_drift_and_frame_retention_failure_refuse(self):
        self.adapter.change=lambda stage,raw: raw.update(metadata_raw_sha256="a"*64) if stage=="trigger" else None
        with self.assertRaises(Refused): self.host.run()
        self.assertNotIn("arm",self.adapter.calls)
        adapter=Adapter(self.selection)
        def fail_retain(stage,evidence):
            adapter.calls.append("retain:"+stage)
            if stage=="open-frame-000": raise OSError("retention unavailable")
        adapter.retain=fail_retain
        with self.assertRaises(OSError): HostSequence(adapter,self.selection,png(True),png(footer=False),png(body=False)).run()
        self.assertEqual(adapter.open_frames,1)
        self.assertNotIn("arm",adapter.calls)


if __name__ == "__main__": unittest.main()
