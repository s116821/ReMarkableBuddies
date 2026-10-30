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
from open_document_timing import OpenDocumentTiming, upper_uptime_ms, diagnostic_cutoff, PRODUCTION_V2, V2_CONTRACT, fixture_inventory_selection, fixture_source_selection, SOURCE5, source_witness_selection, SOURCE5_STABLE_WITNESS
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
        self.source_witness_variant = source_witness_selection(selected)
        self.source_witnesses = []
        self.source_witness_report = None
        self.source_index, self.insertion_index = fixture_source_selection(selected)
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
        if self.source_witness_variant == SOURCE5_STABLE_WITNESS and (type(content['pageCount']) is not int or type(metadata['lastOpenedPage']) is not int):
            raise Refused('Stable source count/index requires exact integers')
        pages = content["cPages"]["pages"]
        ids = [p["id"] for p in pages]
        if ids != self.selected["page_ids"] or content["pageCount"] != self.fixture_page_count:
            raise Refused("Exact selected fixture order/count changed")
        if (raw["last_open"] != self.selected["document_id"] or
                metadata["lastOpenedPage"] != self.source_index or
                content["cPages"]["lastOpened"]["value"] != ids[self.source_index]):
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
            context[1] != self.fixture_page_count or context[2] != self.selected['page_ids'][self.source_index] or
            type(context[3]) is not int or context[3] != self.source_index or type(context[5]) is not bytes or
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
            if self.source_witness_variant == SOURCE5_STABLE_WITNESS:
                self.document(raw)
                context=self.library_context(raw)
                if (self.library_baseline is None or context[:5] != self.library_baseline[:5] or
                    self.library_baseline[5:] != (b'LastOpen=@ByteArray()\n', None) or
                    raw.get('last_open_raw') != ('LastOpen=@ByteArray('+self.selected['document_id']+')\n').encode()):
                    raise Refused('Stable source differs from approved raw Library context')
                self.source_sidecars=hashes
                self.source_witnesses.append(self.source_witness(raw))
                self.response_time(raw)
                return
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

    def source_witness(self, raw, phase='open'):
        witness=self.adapter.source_capture_witness(raw, phase)
        witness['footer_match']=same_pixels(raw['png'],self.source_footer_png,FOOTER)
        return witness

    def seal_source_witness(self, raw):
        if self.source_witness_variant != SOURCE5_STABLE_WITNESS:
            return
        second=self.source_witness(raw,'trigger')
        if (len(self.source_witnesses)!=1 or
            second['capture_prefix']==self.source_witnesses[0]['capture_prefix'] or
            second['capture_claim']<=self.source_witnesses[0]['capture_claim'] or
            second['completed_uptime_ms']<=self.source_witnesses[0]['completed_uptime_ms']):
            raise Refused('Two distinct advancing source captures required')
        self.source_witnesses.append(second)
        self.source_witness_report={'source_witness_variant':self.source_witness_variant,
            'observations':self.source_witnesses,'source_authority':'operator-conjunction',
            'native_current_page_authority':False}
        self.record('source-witness',self.source_witness_report)
        self.response_time(raw)

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
            if self.fixture_inventory_variant == SOURCE5:
                try: self.adapter.frame_ledger.forecast_source(2 if self.source_witness_variant == SOURCE5_STABLE_WITNESS else 3)
                except ValueError as error: raise Refused(str(error)) from error
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
            self.seal_source_witness(ready)
            if not self.timing.admit_creation_ms(now_ms, self.waiting.expiry_ms):
                raise Refused("SDK/recovery publication expiry reached")
            if self.fixture_inventory_variant == SOURCE5:
                self.adapter.reserve_navigation_frames()
            self.baseline_pages = ready["content"]["cPages"]["pages"]
            self.arm_attempted = True
            # Adapter must atomically publish once after a fresh remote BOOTTIME,
            # inode/owner/waiting/PID/start/closing and sidecar-hash recheck.
            self.adapter.arm_once(self.waiting, self.timing, ready)
            result = self.adapter.result_snapshot(self.identity, self.timing.stage_end("result"))
            self.record("result", result)
            if self.fixture_inventory_variant == SOURCE5:
                return self.source5_result(result)
            self.admit(result, "result")
            pages = result["content"]["cPages"]["pages"]
            ids = [p["id"] for p in pages]
            native = result["diagnostics"].get("creation_trial", {})
            report = {
                "same_candidate": True, "native_receipt": native,
                "observed_page_ids": ids,
                "observed_target_presence": self.selected["target_id"] in ids,
                "expected_inserted_order": self.selected["page_ids"][:self.insertion_index] + [self.selected["target_id"]] + self.selected["page_ids"][self.insertion_index:],
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

    def source5_context(self, raw, current):
        self.admit(raw,"result")
        expected = self.selected["page_ids"][:6]+[self.selected["target_id"]]+self.selected["page_ids"][6:]
        content,meta=raw["content"],raw["metadata"]
        pages=content["cPages"]["pages"]
        try:
            keys=[p["idx"]["value"] for p in pages]
            if any(type(k) is not str or not k for k in keys) or len(set(keys))!=9: raise ValueError("idx")
            if [p["id"] for p in sorted(pages,key=lambda p:p["idx"]["value"])] != expected: raise ValueError("order")
            old={p["id"]:p for p in self.baseline_pages}
            for p in pages:
                if p["id"] in old:
                    before=old[p["id"]]
                    if p.get("deleted")!=before.get("deleted") or ("redir" in p)!=("redir" in before) or p.get("redir")!=before.get("redir"): raise ValueError("original map")
            if any(p.get("deleted",{}).get("value") is True for p in pages): raise ValueError("deleted")
        except (KeyError,TypeError,ValueError) as error: raise Refused("Changed active order/redirect map") from error
        if (content.get("pageCount") != 9 or [p["id"] for p in pages] != expected
            or raw["last_open"] != self.selected["document_id"]
            or type(meta.get("lastOpenedPage")) is not int or meta["lastOpenedPage"] != current
            or content["cPages"]["lastOpened"]["value"] != expected[current]):
            raise Refused("Exact same-candidate nine-page/current state required")
        if (type(pages[5].get("redir",{}).get("value")) is not int
            or type(pages[7].get("redir",{}).get("value")) is not int
            or pages[5].get("redir",{}).get("value") != 1
            or pages[7].get("redir",{}).get("value") != 2
            or "redir" in pages[6] and pages[6]["redir"].get("value") is not None):
            raise Refused("Original PDF redirects/non-PDF target changed")
        try: self.adapter.validate_creation_evidence(raw,self.waiting)
        except ValueError as error: raise Refused(str(error)) from error
        return expected

    def source5_result(self, raw):
        from source5_navigation import native_positive, canonical, bounded_local, pixel_classification, page_map_sha256
        if len(self.selected.get("immutable_hashes",{}))!=26:
            raise Refused("Complete preserved source5 26-file baseline required")
        self.idle(raw)
        expected=self.source5_context(raw,6) # Initial source/mixed states never choose a fallback.
        if not self.arm_attempted or not self.adapter.arm_attempted:
            raise Refused("No original one-shot native publication")
        native=raw["diagnostics"].get("creation_trial",{})
        try:
            native_sha=native_positive(native,raw["diagnostics"])
            self.adapter.validate_creation_evidence(raw,self.waiting)
        except ValueError as error:
            raise Refused(str(error)) from error
        creation={"candidate_nonce":self.selected["nonce"],"boot_id":self.identity[0],
            "pid":self.identity[1],"start_ticks":self.identity[2],"document_id":self.selected["document_id"],
            "before_page_ids":self.selected["page_ids"],"page_ids":expected,"target_id":self.selected["target_id"],
            "source_index":5,"insertion_index":6,"native_receipt_sha256":native_sha,"page_map_sha256":page_map_sha256(raw["content"]),
            "preservation_verified":True,"creation_verified":True,
            "candidate_root_dev":int(self.adapter.root_inode.split()[0]),"candidate_root_ino":int(self.adapter.root_inode.split()[1]),
            "candidate_window_sha256":hashlib.sha256((self.adapter.production_window+"\n").encode()).hexdigest()}
        self.record("reconciled-creation",creation)
        self.response_time(raw)
        deadline=self.adapter.known_deadline(self.timing.stage_end("result"))
        if deadline is None: raise Refused("Original candidate clock missing")
        # Initial capture retains its existing response/processing deadline.
        classification=bounded_local(pixel_classification,(raw["png"],self.source_body_png,self.source_footer_png),
            min(raw["response_deadline_monotonic"],deadline))
        if classification["blank"]:
            self.adapter.publish_reconciliation(creation,min(time.monotonic()+3,deadline-1))
            report={"same_candidate":True,"destination_verified":True,"normalization_performed":False,
                "native_durable_success":False,"reconciled_creation":creation,"observed_page_ids":expected}
        elif classification["source"]:
            phase=self.adapter.begin_navigation(both=True)
            released=self.adapter.navigation_move("previous",6,raw,creation,phase)
            source=self.adapter.navigation_snapshot("normalization-source",self.timing.stage_end("result"))
            self.record("normalization-source",source)
            self.source5_context(source,5)
            self.idle(source)
            proof=bounded_local(pixel_classification,(source["png"],self.source_body_png,self.source_footer_png),
                source["response_deadline_monotonic"])
            if not proof["source"]: raise Refused("Normalization did not restore printed PDF source5 body")
            self.response_time(source)
            phase=self.adapter.begin_navigation(both=False)
            released=self.adapter.navigation_move("next",5,source,None,phase)
            destination=self.adapter.navigation_snapshot("inserted-destination",self.timing.stage_end("result"))
            self.record("inserted-destination",destination)
            self.source5_context(destination,6)
            self.idle(destination)
            proof=bounded_local(pixel_classification,(destination["png"],self.source_body_png,self.source_footer_png),
                destination["response_deadline_monotonic"])
            if not proof["blank"]: raise Refused("Inserted target pixels not blank under qualified layout")
            self.response_time(destination)
            report={"same_candidate":True,"destination_verified":True,"normalization_performed":True,
                "native_durable_success":False,"reconciled_creation":creation,"observed_page_ids":expected}
        else:
            raise Refused("Initial target-owned pixels are neither qualified blank nor exact source5")
        end=min(time.monotonic()+1,deadline)
        report['source_witness_variant']=self.source_witness_variant
        if self.source_witness_report is not None:
            report['source_witness']=self.source_witness_report
        self.adapter.retain_bounded("source5-report",report,end)
        if time.monotonic()>=end: raise Refused("Final report exceeded fixed allowance")
        self.adapter.frame_ledger.terminal()
        return report

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
