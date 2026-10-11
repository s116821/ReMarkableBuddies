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
from open_document_timing import OpenDocumentTiming, upper_uptime_ms, diagnostic_cutoff, PRODUCTION_V2, V2_CONTRACT, fixture_inventory_selection
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, OBSERVATION_KIND, PHASE_LABELS

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


def library_comparison(a, b, purpose):
    """Report every difference; exempt only the selected v2 final battery bar."""
    actual, reference = pixels(a), pixels(b)
    # One bit per viewport pixel retains the complete difference set with a
    # fixed 98304-byte bound. Raw frame/reference retain both exact RGB values.
    differences = bytearray(768 * 1024 // 8)
    count = 0
    outside = 0
    for offset in range(0, len(actual), 3):
        if actual[offset:offset+3] == reference[offset:offset+3]:
            continue
        index = offset // 3
        x, y = index % 768, index // 768
        differences[index // 8] |= 1 << (index % 8)
        count += 1
        if not (purpose == PRODUCTION_V2 and 735 <= x < 737 and 30 <= y < 36):
            outside += 1
    return {'whole_image_exact': count == 0, 'masked_match': outside == 0,
            'viewport': [768, 1024], 'bitmap_bytes': 98304,
            'actual_rgb_sha256': hashlib.sha256(actual).hexdigest(),
            'reference_rgb_sha256': hashlib.sha256(reference).hexdigest(),
            'exclusion_half_open': [735, 30, 737, 36] if purpose == PRODUCTION_V2 else None,
            'different_pixels': count, 'outside_exclusion_pixels': outside,
            'difference_bitmap_row_major_lsb_first': bytes(differences)}


class HostSequence:
    def __init__(self, adapter, selected, library_png, source_body_png, source_footer_png):
        self.adapter = adapter
        self.selected = selected
        self.fixture_inventory_variant, self.fixture_page_count = fixture_inventory_selection(selected)
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
        self.diagnostic_end = None
        self.library_baseline = None

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
        self.timing = OpenDocumentTiming(*window[1:4], self.selected.get('experiment_kind', 'open-document-insertion-v1'))
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
        if stage == 'diagnostic':
            admitted = self.diagnostic_end is not None and self.timing.candidate_at * 1000 <= now_ms < self.diagnostic_end * 1000
        else:
            admitted = self.timing.admit(stage, (now_ms + 999) // 1000)
        if not admitted:
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
        if ids != self.selected["page_ids"] or content["pageCount"] != self.fixture_page_count:
            raise Refused("Exact selected fixture order/count changed")
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

    def library_context(self, raw):
        """Immutable comparison tuple; never a selected-document witness."""
        content, metadata = raw['content'], raw['metadata']
        return (tuple(page['id'] for page in content['cPages']['pages']),
                content['pageCount'], content['cPages']['lastOpened']['value'],
                metadata['lastOpenedPage'], self.sidecars(raw),
                raw.get('last_open_raw'), raw['last_open'])

    def pending_library(self, raw, now_ms):
        baseline = self.library_baseline
        if (baseline is None or self.arm_attempted or raw.get('diagnostics') != {} or
            now_ms >= self.waiting.expiry_ms):
            raise Refused('Pending Library lacks original unarmed SDK/context authority')
        context = self.library_context(raw)
        if (context != baseline or context[0] != tuple(self.selected['page_ids']) or
            context[1] != self.fixture_page_count or context[2] != self.selected['page_ids'][0] or
            type(context[3]) is not int or context[3] != 0 or type(context[5]) is not bytes or
            context[5] != b'LastOpen=@ByteArray()\n' or context[6] is not None):
            raise Refused('Pending Library requires unchanged canonical raw baseline')

    def observe_open(self):
        # Read-only capture polling never renews the fixed open deadline. The
        # footer can precede loaded body pixels. A recognized pre-footer Library
        # retains baseline context only; every source frame requires exact
        # selected-document context and later frames preserve raw sidecars.
        footer_seen = False
        for index in range(MAX_OPEN_FRAMES):
            raw = self.adapter.snapshot("open", self.timing.stage_end("open"), False)
            self.record(f"open-frame-{index:03}", raw)
            now_ms = self.admit(raw, "open")
            if (self.selected.get('experiment_kind') == PRODUCTION_V2 and
                same_pixels(raw['png'], self.library_png)):
                if footer_seen:
                    raise Refused('Library returned after source footer')
                self.pending_library(raw, now_ms)
                self.response_time(raw)
                self.record(f'open-library-unready-{index:03}', raw)
                self.response_time(raw)
                continue
            self.document_context(raw)
            hashes = self.sidecars(raw)
            if not footer_seen:
                if same_pixels(raw["png"], self.source_footer_png, FOOTER):
                    self.source_sidecars = hashes
                    footer_seen = True
                    self.record("source-footer-observed", raw)
                self.response_time(raw)
                continue
            if same_pixels(raw["png"], self.source_body_png, BODY):
                self.record("source-body-ready", raw)
                self.response_time(raw)
                return
            self.response_time(raw)
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
        purpose = self.selected.get('experiment_kind', 'open-document-insertion-v1')
        if purpose not in {'open-document-insertion-v1', PRODUCTION_V2}:
            raise Refused('Observation purpose cannot enter production sequence')
        if purpose == PRODUCTION_V2 and self.selected.get('production_contract') != V2_CONTRACT:
            raise Refused('Exact production v2 contract required')
        try:
            library = self.adapter.wait_library()
            for index in range(MAX_READY_FRAMES):
                self.record(f"ready-frame-{index:03}", library)
                if index == 0:
                    self.bind(library)
                self.admit(library, "ready")
                self.idle(library)
                if purpose == PRODUCTION_V2:
                    comparison = library_comparison(library['png'], self.library_png, purpose)
                    self.record(f'library-comparison-{index:03}', comparison)
                    matches = comparison['masked_match']
                else:
                    matches = same_pixels(library['png'], self.library_png)
                self.response_time(library)
                if matches:
                    self.record("library", library)
                    if purpose == PRODUCTION_V2:
                        self.library_baseline = self.library_context(library)
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
            self.response_time(ready)
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
            self.response_time(result)
            return report
        finally:
            self.stop_once()

    def diagnostic_phases(self, raw):
        data = raw.get('phases')
        if type(data) is not bytes or len(data) > 1024 or not data.endswith(b'\n'):
            raise Refused('Complete bounded diagnostic phase record required')
        lines = data.decode('ascii').splitlines()
        if len(lines) != len(PHASE_LABELS):
            raise Refused('Diagnostic phase count refused')
        previous = self.timing.candidate_at * 1000
        result = []
        completed = upper_uptime_ms(raw['uptime'])
        for label, line in zip(PHASE_LABELS, lines):
            match = re.fullmatch(re.escape(label) + r' ([0-9]{1,12}\.[0-9]{1,3})', line)
            if not match:
                raise Refused('Diagnostic phase label or uptime refused')
            upper = upper_uptime_ms(match[1] + ' 0.00\n')
            if upper < previous or upper >= self.diagnostic_end * 1000 or upper > completed:
                raise Refused('Diagnostic phase time outside fixed interval')
            previous = upper
            result.append({'phase': label, 'uptime': match[1], 'upper_ms_since_candidate': upper - self.timing.candidate_at * 1000})
        return result

    def run_diagnostic(self):
        if self.selected.get('experiment_kind') != OBSERVATION_KIND:
            raise Refused('Explicit observation-only purpose required')
        try:
            context = self.adapter.wait_diagnostic_context()
            self.record('diagnostic-context', context)
            self.bind(context)
            self.diagnostic_end = diagnostic_cutoff(self.timing, self.waiting.expiry_ms)
            observations = []
            for index in range(3):
                raw = self.adapter.snapshot('diagnostic', self.diagnostic_end, True)
                self.record(f'diagnostic-frame-{index:03}', raw)
                self.admit(raw, 'diagnostic')
                self.idle(raw)
                if raw.get('diagnostics') or raw.get('diagnostic_cutoff') != self.diagnostic_end:
                    raise Refused('Diagnostic SDK closure or cutoff mismatch')
                phases = self.diagnostic_phases(raw)
                matches = same_pixels(raw['png'], self.library_png)
                self.response_time(raw)
                observations.append({'index': index, 'library_pixels_match': matches, 'phases': phases,
                                     'completed_uptime': raw['uptime'],
                                     'completed_upper_ms_since_candidate': upper_uptime_ms(raw['uptime']) - self.timing.candidate_at * 1000})
                if matches:
                    break
            report = {'purpose': OBSERVATION_KIND, 'production_ready': False, 'publication_permitted': False,
                      'fixed_cutoff': self.diagnostic_end, 'candidate_at': self.timing.candidate_at,
                      'library_observed': observations[-1]['library_pixels_match'], 'observations': observations,
                      'terminal_reason': 'library-observed' if observations[-1]['library_pixels_match'] else 'frame-cap',
                      'authority': 'sampled visual timing only; producer interval does not identify plane-read instant'}
            self.record('diagnostic-report', report)
            self.response_time(raw)
            return report
        finally:
            self.stop_once()
