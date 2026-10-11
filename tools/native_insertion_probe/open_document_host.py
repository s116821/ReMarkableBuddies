"""Bounded development host sequence; the transport adapter supplies raw evidence.

This conjunction is operator observation, not atomic native active-page authority.
No retries of input, permission publication, or stop publication are allowed.
"""
import hashlib
import io
import re
import time
from PIL import Image
from open_document_protocol import parse_waiting
from open_document_timing import OpenDocumentTiming, upper_uptime_ms
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT

BODY = (64, 0, 720, 980)
FOOTER = (280, 986, 488, 1024)
MAX_OPEN_FRAMES = CAPTURE_CONTRACT['maximum_open_frames']
MAX_READY_FRAMES = CAPTURE_CONTRACT['maximum_ready_frames']


class Refused(ValueError):
    pass


def pixels(data, box=None):
    if type(data) is not bytes or len(data) > 8 * 1024 * 1024:
        raise Refused("Bounded PNG bytes required")
    try:
        with Image.open(io.BytesIO(data)) as im:
            if im.format != "PNG" or im.size != (768, 1024):
                raise Refused("Exact reviewed portrait viewport required")
            im.load()
            im = im.convert("RGB")
            return (im.crop(box) if box else im).tobytes()
    except (OSError, Image.DecompressionBombError) as error:
        raise Refused("PNG observation cannot be decoded safely") from error


def same_pixels(a, b, box=None):
    return pixels(a, box) == pixels(b, box)


class HostSequence:
    def __init__(self, adapter, selected, library_png, source_body_png, source_footer_png):
        self.adapter = adapter
        self.selected = selected
        self.library_png = library_png
        self.source_body_png = source_body_png
        self.source_footer_png = source_footer_png
        pixels(library_png); pixels(source_body_png); pixels(source_footer_png)
        self.candidate_window = None
        self.last_completed_ms = None
        self.source_sidecars = None
        self.trace = []
        self.identity = None
        self.timing = None
        self.waiting = None
        self.arm_attempted = False
        self.stop_attempted = False

    def record(self, stage, evidence):
        self.trace.append((stage, evidence))
        # The adapter must save raw evidence before a later mutation. Failed
        # evidence retention refuses the action; finally still requests recovery.
        self.adapter.retain(stage, evidence)

    def bind(self, raw):
        s = self.selected
        window = raw["window"]
        if len(window) != 5 or window[0] != s["nonce"]:
            raise Refused("Candidate window nonce mismatch")
        self.timing = OpenDocumentTiming(*window[1:4])
        if window[4] != self.timing.observation_end:
            raise Refused("Candidate cutoff mismatch")
        identity = (raw["boot_id"], raw["pid"], raw["start_ticks"])
        if identity[0] != s["boot_id"]:
            raise Refused("Boot changed")
        self.waiting = parse_waiting(raw["waiting"], s["nonce"], identity[1], identity[2])
        self.identity = identity
        self.candidate_window = tuple(window)

    def admit(self, raw, stage):
        self.response_time(raw)
        if (raw["boot_id"], raw["pid"], raw["start_ticks"]) != self.identity:
            raise Refused("Candidate process identity changed")
        if raw["closed"] is not False:
            raise Refused("Admission closed or unknown")
        now_ms = upper_uptime_ms(raw["uptime"])
        if tuple(raw["window"]) != self.candidate_window:
            raise Refused("Fixed candidate window changed")
        if self.last_completed_ms is not None and now_ms <= self.last_completed_ms:
            raise Refused("Completed observation time did not advance")
        if not self.timing.admit(stage, (now_ms + 999) // 1000):
            raise Refused("Stage expired before completed observation")
        if parse_waiting(raw["waiting"], self.selected["nonce"], raw["pid"], raw["start_ticks"]) != self.waiting:
            raise Refused("Waiting identity or fixed expiry changed")
        for path, digest in self.selected["immutable_hashes"].items():
            if raw["hashes"].get(path) != digest:
                raise Refused("Immutable ink/provider/file changed or unobserved")
        self.last_completed_ms = now_ms
        return now_ms

    def response_time(self, raw):
        # SSHAdapter supplies this local bound independently of remote JSON.
        # Evidence retention and pixel decoding must not extend that allowance.
        deadline = raw.get('response_deadline_monotonic')
        if deadline is not None and time.monotonic() >= deadline:
            raise Refused("Host processing exhausted fixed snapshot allowance")

    def document_context(self, raw):
        content, metadata = raw["content"], raw["metadata"]
        pages = content["cPages"]["pages"]
        ids = [p["id"] for p in pages]
        if ids != self.selected["page_ids"] or content["pageCount"] != 7:
            raise Refused("Exact seven-page order changed")
        if (raw["last_open"] != self.selected["document_id"] or
                metadata["lastOpenedPage"] != 0 or
                content["cPages"]["lastOpened"]["value"] != ids[0]):
            raise Refused("Exact source document/current page not observed")
    def document(self, raw):
        self.document_context(raw)
        if not same_pixels(raw["png"], self.source_body_png, BODY):
            raise Refused("Source page pixels ambiguous")

    def sidecars(self, raw):
        values = tuple(raw.get(key) for key in ("content_raw_sha256", "metadata_raw_sha256"))
        if any(type(value) is not str or not re.fullmatch("[0-9a-f]{64}", value) for value in values):
            raise Refused("Exact raw sidecar hashes missing")
        if self.source_sidecars is not None and values != self.source_sidecars:
            raise Refused("Raw source sidecars changed across observations")
        return values

    def observe_open(self):
        # Read-only capture polling never renews the fixed open deadline. The
        # footer can precede loaded body pixels, but every frame retains exact
        # identity/document context and every later frame preserves raw sidecars.
        footer_seen = False
        for index in range(MAX_OPEN_FRAMES):
            raw = self.adapter.snapshot("open", self.timing.stage_end("open"), False)
            self.record(f"open-frame-{index:03}", raw)
            self.admit(raw, "open")
            self.document_context(raw)
            hashes = self.sidecars(raw)
            if not footer_seen:
                if same_pixels(raw["png"], self.source_footer_png, FOOTER):
                    self.source_sidecars = hashes
                    footer_seen = True
                    self.record("source-footer-observed", raw)
                continue
            if same_pixels(raw["png"], self.source_body_png, BODY):
                self.record("source-body-ready", raw)
                return
        raise Refused("Actual source footer then fresh loaded body not observed within bounded frames")

    def idle(self, raw):
        idle = raw["idle"]
        if (idle.get("interval_ms") != 2000 or idle.get("invalidated") is not False
                or idle.get("touch_active") is not False
                or idle.get("pen_contact") != 0 or idle.get("pen_tool") != 0):
            raise Refused("Complete released input-idle observation missing")

    def stop_once(self):
        if self.identity is None or self.stop_attempted:
            return
        self.stop_attempted = True
        # This permission only requests guarded stopping, never insertion success.
        # Adapter refuses stale identity/closing state; actor deadline remains
        # independently responsible for recovery even after host/transport loss.
        self.adapter.stop_once(self.identity, self.timing.stage_end("stop"))

    def run(self):
        try:
            library = self.adapter.wait_library()
            for index in range(MAX_READY_FRAMES):
                self.record(f"ready-frame-{index:03}", library)
                if index == 0:
                    self.bind(library)
                self.admit(library, "ready")
                self.idle(library)
                matches = same_pixels(library["png"], self.library_png)
                self.response_time(library)
                if matches:
                    self.record("library", library)
                    self.response_time(library)
                    break
                if index + 1 == MAX_READY_FRAMES:
                    raise Refused("Approved Library not observed within bounded ready frames")
                library = self.adapter.snapshot("ready", self.timing.stage_end("ready"), True)
            # Adapter rechecks candidate identity/closing/time before this one
            # reviewed 768x1024 tap. Sorting alone never selects the tile.
            self.adapter.open_once(self.identity, self.timing.stage_end("open"))
            self.observe_open()
            # Full idle interval follows tap/release and observed source. No
            # intervening input is permitted. A disappearing footer is retained
            # above, while the final body/current-page evidence must still match.
            ready = self.adapter.snapshot("trigger", self.timing.stage_end("trigger"), True)
            self.record("pre-arm", ready)
            now_ms = self.admit(ready, "trigger")
            self.document(ready)
            self.sidecars(ready)
            self.idle(ready)
            if not self.timing.admit_creation_ms(now_ms, self.waiting.expiry_ms):
                raise Refused("SDK/recovery publication expiry reached")
            self.arm_attempted = True
            # Adapter must atomically publish once after a fresh remote BOOTTIME,
            # inode/owner/waiting/PID/start/closing and sidecar-hash recheck.
            self.adapter.arm_once(self.waiting, self.timing, ready)
            result = self.adapter.result_snapshot(self.identity, self.timing.stage_end("result"))
            self.record("result", result)
            self.admit(result, "result")
            pages = result["content"]["cPages"]["pages"]
            ids = [p["id"] for p in pages]
            native = result["diagnostics"].get("creation_trial", {})
            report = {
                "same_candidate": True, "native_receipt": native,
                "observed_page_ids": ids,
                "observed_target_presence": self.selected["target_id"] in ids,
                "expected_inserted_order": [self.selected["page_ids"][0], self.selected["target_id"]] + self.selected["page_ids"][1:],
                "source_pixels_match": same_pixels(result["png"], self.source_body_png, BODY),
                "source_footer_pixels_match": same_pixels(result["png"], self.source_footer_png, FOOTER),
                "observed_page_count": result["content"].get("pageCount"),
                "observed_current_page_id": result["content"].get("cPages", {}).get("lastOpened", {}).get("value"),
                "metadata_page_index": result["metadata"].get("lastOpenedPage"),
                "native_success": "not inferred; reconcile exact receipt/files/UI",
                "authority": "bounded operator observation, not atomic native active-page authority",
            }
            self.record("report", report)
            return report
        finally:
            self.stop_once()
