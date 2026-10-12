"""Pinned SSH adapter for the reviewed bounded development host sequence."""
import base64
import hashlib
import json
from pathlib import Path
import re
import shlex
import time
import uuid
from open_document_host import Refused, same_pixels
from open_document_timing import upper_uptime_ms, OpenDocumentTiming, diagnostic_cutoff, PRODUCTION_V2, fixture_inventory_selection, fixture_source_selection, SOURCE5
from open_document_protocol import parse_waiting
from source5_navigation import FrameLedger, PhaseBudget, POST_STAGES, canonical, bounded_local, CONTROL_BYTES, FUTURE_CONTROL_BYTES, ControlLedger
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, TOOLS as CAPTURE_TOOLS, OBSERVATION_KIND, PHASE_LABELS, capture_script, validate_receipt, hash_tool_checks, ordered_hash_checks


def q(value):
    return shlex.quote(str(value))


def retain_record(path, evidence, maximum=2*1024*1024):
    def encode(value):
        if isinstance(value,bytes): return {"base64":base64.b64encode(value).decode(),"sha256":hashlib.sha256(value).hexdigest()}
        raise TypeError("Unsupported evidence")
    text=json.dumps(evidence,default=encode,indent=2)+"\n"
    if len(text.encode())>maximum: raise ValueError("Bounded host record exceeded")
    with Path(path).open("x",encoding="utf-8") as stream:
        stream.write(text)
        stream.flush()
    return len(text.encode())

def decode_snapshot(response,nonce,diagnostic,diagnostic_end,expected_root_inode,response_deadline,post_creation,boot_id,idle):
    raw = json.loads(response)
    def decode(key):
        value=base64.b64decode(raw[key],validate=True)
        if post_creation or "config_raw" in raw:
            cap={"png":850000,"content":65536,"metadata":65536,"config_raw":65536,"diagnostics":8192,
                "hashes":4096,"capture_receipt":2048,"idle":2048,"window":256,"waiting":512,
                "last_open":65536,"creation_arm_raw":512}.get(key,2048)
            if len(value)>cap: raise Refused("Closed source5 field exceeded: "+key)
        return value
    try:
        capture_evidence = validate_receipt(decode("capture_receipt").decode("ascii"), nonce, raw["frame_hash"], decode("png"), raw["native_hash"])
    except (KeyError, ValueError, UnicodeError) as error:
        raise Refused("Native owned-frame receipt binding failed") from error
    window = decode("window").decode("ascii").strip().split()
    if len(window) != 5 or any(not re.fullmatch("[0-9]+", v) for v in window[1:]):
        raise Refused("Malformed fixed actor window")
    log = decode("idle").decode("utf-8")
    idle_ok = re.fullmatch(r"kernel slots 0\.\.31 selected (?:[0-9]|[12][0-9]|3[01])\nobserver ready; owned_modifier=false; 2s\n", log) is not None
    hashes = {}
    for line in decode("hashes").decode("ascii").splitlines():
        digest, path = line.split("  ", 1)
        if not re.fullmatch("[0-9a-f]{64}", digest) or path in hashes:
            raise Refused("Malformed immutable hash observation")
        hashes[path] = digest
    last_raw = decode("last_open")
    last = last_raw.decode("utf-8").strip()
    match = re.fullmatch(r"LastOpen=@ByteArray\(([0-9a-f-]{36})\)", last)
    observed = {"window": [window[0]] + [int(v) for v in window[1:]], "boot_id": boot_id,
                "response_deadline_monotonic": response_deadline,
                "pid": raw["pid"], "start_ticks": raw["start_ticks"], "closed": False,
                "waiting": decode("waiting"), "uptime": decode("uptime").decode("ascii"),
                "last_open": match[1] if match else None, "last_open_raw": last_raw, "content": json.loads(decode("content")),
                "metadata": json.loads(decode("metadata")), "png": decode("png"), "hashes": hashes,
                "content_raw": decode("content"), "metadata_raw": decode("metadata"),
                "content_raw_sha256": hashlib.sha256(decode("content")).hexdigest(),
                "metadata_raw_sha256": hashlib.sha256(decode("metadata")).hexdigest(),
                "idle": {"interval_ms": 2000 if idle else None, "invalidated": not idle_ok, "touch_active": not idle_ok, "pen_contact": 0 if idle else None, "pen_tool": 0 if idle else None},
                "capture": capture_evidence, "diagnostics": json.loads(decode("diagnostics")) if raw["diagnostics"] else {}}
    if "config_raw" in raw:
        observed["config_raw"]=decode("config_raw")
    if post_creation:
        observed["creation_arm_raw"]=decode("creation_arm_raw")
        observed["target_files"]={key:decode(key).decode("ascii") for key in ("target-ink.stat","target-ink.hash","target-thumb.stat","target-thumb.hash","target-paths")}
    if diagnostic:
        observed['phases'] = decode('phases')
        observed['diagnostic_cutoff'] = diagnostic_end
    if expected_root_inode is not None and expected_root_inode != raw["root_inode"]:
        raise Refused("Private root inode changed")
    if time.monotonic() >= response_deadline:
        raise Refused("Snapshot decoding exhausted fixed response deadline")
    observed["_root_inode"] = raw["root_inode"]
    return observed


class SSHAdapter:
    def __init__(self, arguments, receipt, profile, output, transport, library_png, control_ledger=None):
        self.arguments, self.receipt, self.profile = arguments, receipt, profile
        self.fixture_inventory = fixture_inventory_selection(profile)
        self.fixture_source = fixture_source_selection(profile)
        self.sealed_source5 = canonical({k:profile.get(k) for k in ("fixture_inventory_variant","source_index","source_page_id","neighbor_index","neighbor_page_id","insertion_index","target_id","document_id","page_ids","gesture_paths","gesture_path_hashes","tools")})
        self.frame_ledger = FrameLedger() if self.fixture_inventory[0] == SOURCE5 else None
        if self.frame_ledger is not None:
            sizes=profile.get("packet_file_sizes")
            if (type(profile.get("source5_control_bytes_reserve")) is not int or profile.get("source5_control_bytes_reserve")!=CONTROL_BYTES or type(sizes) is not dict
                or set(sizes)!=(set(receipt["files"])-{"host-profile.json"})
                or any(type(n) is not int or n<0 for n in sizes.values())
                or sum(sizes.values())+2*65536+FUTURE_CONTROL_BYTES>CONTROL_BYTES):
                raise Refused("Actual packet/control reservation unavailable")
        self.capture_prefixes=[]
        self.capture_sources={}
        self.helper_source_claims=set()
        self.guardian_source_claim=None
        self.trigger_source_claim=None
        self.diagnostic_poll_count=0
        self.result_poll_started=False
        self.postnative_control={}
        self.poll_clock_scope=False
        self.source5_profile_seal=canonical(profile)
        self.source5_receipt_seal=canonical(receipt)
        self.control_ledger=control_ledger or ControlLedger()
        if self.frame_ledger is not None:
            from coordinator import SOURCE5_REMOTE_CONTROL_BYTES
            if self.control_ledger.remote_reserved<SOURCE5_REMOTE_CONTROL_BYTES:
                self.control_ledger.reserve_remote(SOURCE5_REMOTE_CONTROL_BYTES-self.control_ledger.remote_reserved)
        self.navigation_phase = None
        self.reconciled_creation = None
        self.reconciled_creation_sha256 = None
        if fixture_inventory_selection(receipt, 'before_page_ids') != self.fixture_inventory:
            raise Refused('Profile/receipt fixture selection mismatch')
        self.output, self.transport = Path(output), transport
        self.root = receipt["remote_root"]
        self.doc = "/home/root/.local/share/remarkable/xochitl/" + profile["document_id"]
        self.library_png = library_png
        self.identity = None
        self.root_inode = None
        self.counter = 0
        self.open_frames = 0
        self.ready_frames = 0
        self.ready_cutoff = None
        self.capture_failed = False
        self.diagnostic_frames = 0
        self.diagnostic_context = None
        self.diagnostic_end = None
        self.tap_attempted = False
        self.library_tap_variant = None
        self.arm_attempted = False
        self.stop_attempted = False
        self.tap_released = False
        self.tap_release_sha256 = None
        self.tap_supervisor_sha256 = None
        self.action_deadline = None
        self.production_window = None
        self.production_waiting = None
        self.sdk_expiry_ms = None
        self.candidate_local_anchor = None
        self.candidate_remote_at = None
        self.fixed_local_deadlines = {}

    def retain(self, stage, evidence):
        if self.frame_ledger is not None and stage in {
            "source5-control-reservation","tap-ready","tap-go-publication","tap-released"}:
            return self.retain_bounded(stage,evidence,self.action_deadline)
        if self.frame_ledger is not None:
            if self.frame_ledger.frames==0:
                raise Refused("No claimed capture owns this observation record")
            if not hasattr(self,"host_record_counts"): self.host_record_counts={}
            number=self.frame_ledger.frames
            self.host_record_counts[number]=self.host_record_counts.get(number,0)+1
            if self.host_record_counts[number]>3: raise Refused("Three host records per capture exhausted")
        if self.navigation_phase is not None and self.navigation_phase.processing_end is not None:
            return self.retain_bounded(stage,evidence,self.navigation_phase.processing_end,control=False)
        if self.frame_ledger is not None:
            return retain_record(str(self.output/(stage+".json")),evidence)
        def encode(value):
            if isinstance(value, bytes):
                return {"base64": base64.b64encode(value).decode("ascii"), "sha256": hashlib.sha256(value).hexdigest()}
            raise TypeError("Unsupported evidence type")
        (self.output / (stage + ".json")).write_text(json.dumps(evidence, default=encode, indent=2) + "\n")

    def call(self, script, timeout=4, record_role="ordinary", record_deadline=None):
        if record_deadline is not None and time.monotonic() >= record_deadline:
            raise Refused("Fixed record deadline exhausted before transport")
        if (fixture_inventory_selection(self.profile) != self.fixture_inventory or
            fixture_inventory_selection(self.receipt, 'before_page_ids') != self.fixture_inventory):
            raise Refused('Sealed fixture inventory changed; no transport permitted')
        if self.frame_ledger is not None and (canonical(self.profile)!=self.source5_profile_seal or canonical(self.receipt)!=self.source5_receipt_seal):
            raise Refused("Sealed source5 profile/receipt changed; no transport")
        if self.frame_ledger is not None and canonical({k:self.profile.get(k) for k in ("fixture_inventory_variant","source_index","source_page_id","neighbor_index","neighbor_page_id","insertion_index","target_id","document_id","page_ids","gesture_paths","gesture_path_hashes","tools")}) != self.sealed_source5:
            raise Refused("Source5 tuple changed; no transport")
        recovery_role=record_role in {"stop","stop-clock"} or (record_role=="clock" and self.stop_attempted)
        if self.frame_ledger is not None and self.control_ledger.overflow and not recovery_role:
            raise Refused("Control quota exhausted; only original recovery may proceed")
        capture_record=self.frame_ledger is not None and getattr(self,"capture_scope",False)
        if capture_record:
            active=getattr(self,"active_capture_claim",None)
            if active is None or active[1]!=self.frame_ledger.frames:
                raise Refused("Capture transport lacks its active frame reservation")
        claim=None
        caps={"clock":2048,"poll-clock":2048,"staging-clock":2048,"stop-clock":2048,
            "staging":8192,"helper":32768,"poll":16384,"stop":8192,"ordinary":8192}
        if self.frame_ledger is not None and record_role=="poll" and self.diagnostic_poll_count>=30:
            raise Refused("SOURCE5 diagnostic poll maximum reached; no retry")
        if self.frame_ledger is not None and not capture_record:
            if record_role not in caps: raise Refused("Unknown transport record role")
            try:
                claim=self.take_postnative_record(record_role)
                if claim is None:
                    recovery=record_role in {"stop","stop-clock"} or (record_role=="clock" and self.stop_attempted)
                    claim=self.control_ledger.reserve_record(caps[record_role],terminal=recovery)
            except ValueError as error: raise Refused(str(error)) from error
        if self.frame_ledger is not None and record_role=="poll":
            self.diagnostic_poll_count+=1
        started = time.monotonic() if self.is_diagnostic() else None
        result = self.transport(self.arguments, script, timeout)
        self.last_response_arrival = time.monotonic() if self.frame_ledger is not None else None
        if self.navigation_phase is not None and getattr(self,"capture_rpc",False):
            self.navigation_phase.arrived()
        if self.frame_ledger is not None and (len(result.get("stdout","").encode()) > 2*1024*1024 or len(result.get("stderr","").encode()) > 65536):
            raise Refused("Bounded source5 transport evidence exceeded")
        if started is not None:
            result = result | {'local_monotonic_started': started,
                               'local_monotonic_completed': time.monotonic()}
        self.counter += 1
        if self.frame_ledger is not None:
            if self.navigation_phase is not None:
                deadline=self.navigation_phase.processing_end or getattr(self,"call_deadline",self.navigation_phase.staging_end)
            else:
                # Snapshot/clock evidence belongs to that operation's sealed
                # bound, not a preceding tap's shorter contact lease.
                deadline=record_deadline if record_deadline is not None else (getattr(self,"action_deadline",None) or (time.monotonic()+2))
            self.retain_bounded(f"transport-{self.counter:03}",result,deadline,
                control=not capture_record,maximum=caps[record_role],reservation=claim)
        else:
            (self.output / f"transport-{self.counter:03}.json").write_text(json.dumps(result, indent=2) + "\n")
        if result["timeout"] or result["exit"] != 0:
            raise Refused("Remote action failed or uncertain; never replay an action")
        return result["stdout"]

    def is_diagnostic(self):
        return self.profile.get('experiment_kind') == OBSERVATION_KIND

    def require_publication(self):
        if self.profile.get('experiment_kind', 'open-document-insertion-v1') not in {'open-document-insertion-v1', PRODUCTION_V2}:
            raise Refused('Observation purpose cannot publish input or creation permission')

    def selected_tap(self):
        if not self.is_v2():
            return "--tap", [201, 1360]
        from prepare_packet import library_tap_selection
        try:
            selected = library_tap_selection(self.profile)
            library_tap_selection(self.receipt)
            if self.receipt.get("library_tap_variant") != self.profile["library_tap_variant"]:
                raise ValueError("Conflicting packet tap selection")
            if self.library_tap_variant is not None and self.library_tap_variant != self.profile["library_tap_variant"]:
                raise ValueError("Preselected tap variant changed; never switch")
            self.library_tap_variant = self.profile["library_tap_variant"]
            return selected
        except ValueError as error:
            raise Refused(str(error)) from error

    def is_v2(self):
        return self.profile.get('experiment_kind') == PRODUCTION_V2

    def action_guard(self, deadline=None):
        end = self.action_deadline if deadline is None else deadline
        if self.is_v2() and (end is None or time.monotonic() >= end):
            raise Refused('Complete action processing exceeded immutable deadline; never replay')

    def action_budget(self, cutoff, cap, sdk_expiry_ms=None, full=False):
        """A query cannot renew the stage; its entire cost is charged."""
        effective_cutoff = min(cutoff, sdk_expiry_ms / 1000) if sdk_expiry_ms is not None else cutoff
        _, _, deadline = self.clock_query(effective_cutoff)
        remaining = deadline - time.monotonic()
        if remaining <= 0 or (full and remaining < cap):
            raise Refused('Full fixed action reservation unavailable; never replay')
        self.action_deadline = deadline
        return min(cap, remaining), deadline

    def known_deadline(self, cutoff):
        end = self.fixed_local_deadlines.get(cutoff)
        if self.candidate_local_anchor is not None:
            derived = self.candidate_local_anchor + cutoff - self.candidate_remote_at
            end = derived if end is None else min(end, derived)
        return end

    def clock_query(self, cutoff):
        """Known bounds apply before the query; refresh can only tighten."""
        anchor = time.monotonic()
        known = self.known_deadline(cutoff) if self.is_v2() else None
        if known is not None and anchor >= known:
            raise Refused('Fixed stage expired before clock query')
        cap = min(3, known - anchor) if known is not None else 3
        clock_script='set -eu\ncat /proc/uptime\n'
        role=("stop-clock" if self.stop_attempted else "poll-clock" if self.poll_clock_scope
            else "staging-clock" if self.navigation_phase is not None else "clock")
        clock_text=self.call(clock_script,cap,record_role=role,record_deadline=anchor+cap) if self.frame_ledger is not None else self.call(clock_script,cap)
        remote=upper_uptime_ms(clock_text)
        if self.frame_ledger is not None:
            self.clock_sample=(int(float(clock_text.split()[0])*1000),self.last_response_arrival)
        deadline = anchor + cutoff - remote / 1000
        if known is not None:
            deadline = min(deadline, known)
        if self.is_v2():
            self.fixed_local_deadlines[cutoff] = deadline
            if self.candidate_remote_at is not None:
                refreshed = anchor + self.candidate_remote_at - remote / 1000
                self.candidate_local_anchor = refreshed if self.candidate_local_anchor is None else min(self.candidate_local_anchor, refreshed)
        return remote, anchor, deadline

    def production_closure_checks(self):
        checks = ''.join(f'test ! -e "$root/{name}"\ntest ! -L "$root/{name}"\n'
                       for name in ('diagnostics.json', 'callback.json', 'creation-arm', 'creation-arm.tmp'))
        if self.sdk_expiry_ms is not None:
            checks += f'test $(( $(now_upper)*1000 )) -lt {self.sdk_expiry_ms}\n'
        return checks

    def poll_pause(self, deadline):
        if not self.is_v2():
            time.sleep(0.1)
            return
        self.action_guard(deadline)
        time.sleep(min(0.1, max(0, deadline - time.monotonic())))
        self.action_guard(deadline)

    def diagnostic_closure_checks(self):
        return ''.join(f'test ! -e "$root/{name}"\ntest ! -L "$root/{name}"\n'
                       for name in ('entry.closed', 'restore.claim', 'stop.request', 'stop.admitted',
                                    'diagnostics.json', 'callback.json', 'tap-start.claim', 'tap.go', 'creation-arm', 'creation-arm.tmp'))

    def diagnostic_phase_setup(self, prefix):
        return f'''test ! -e "$root/{prefix}.phases"
test ! -L "$root/{prefix}.phases"
(set -C; : > "$root/{prefix}.phases")
private "$root/{prefix}.phases"
diagnostic_phase() {{
 read diagnostic_uptime diagnostic_idle diagnostic_extra < /proc/uptime || return 90
 test -z "$diagnostic_extra" || return 90
 test "${{#diagnostic_uptime}}" -le 16 || return 90
 printf '%s %s\\n' "$1" "$diagnostic_uptime" >> "$root/{prefix}.phases"
}}
diagnostic_phase record_start || exit 90
'''

    def prelude(self, cutoff=None, identity=None, snapshot=False):
        text = f"""set -eu
umask 077
root={q(self.root)}
doc={q(self.doc)}
private() {{
 test -f "$1"; test ! -L "$1"
 test "$(stat -c '%a %u %h' "$1")" = '600 0 1'
}}
test -d "$root"; test ! -L "$root"
test "$(stat -c '%a %u' "$root")" = '700 0'
initial_root_identity=$(stat -c '%d %i' "$root")
private "$root/owner"
test "$(cat "$root/owner")" = {q(self.profile["nonce"])}
private "$root/candidate.window"
private "$root/attempt.identity"
private "$root/creation-waiting"
read pid start extra < "$root/attempt.identity"
test -z "$extra"
case "$pid:$start" in *[!0-9:]*|:*|*:) exit 90;; esac
test "$pid" -gt 1; test "$start" -gt 0
test "$(sed 's/^.*) //' /proc/$pid/stat | awk '{{print $20}}')" = "$start"
test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"
for name in entry.closed restore.claim stop.request stop.admitted; do
 test ! -e "$root/$name"; test ! -L "$root/$name"
done
test "$(cat /proc/sys/kernel/random/boot_id)" = {q(self.profile["boot_id"])}
"""
        if self.root_inode:
            text += f'test "$(stat -c \'%d %i\' "$root")" = {q(self.root_inode)}\n'
        if identity:
            text += f'test "$pid $start" = {q(str(identity[1]) + " " + str(identity[2]))}\n'
        if cutoff:
            text += f"cutoff={int(cutoff)}\n"
        else:
            text += 'read selected_nonce armed deadline candidate cutoff < "$root/candidate.window"\n'
        text += r"""now_upper() { awk '{n=int($1); if($1>n)n++; print n}' /proc/uptime; }
test "$(now_upper)" -lt "$cutoff"
b64() { /usr/bin/openssl base64 -A -in "$1"; }
"""
        text += hash_tool_checks()
        if self.is_v2() and self.production_window is not None:
            text += f'test "$(cat "$root/candidate.window")" = {q(self.production_window)}\n'
        if self.is_v2() and self.production_waiting is not None:
            text += f'test "$(cat "$root/creation-waiting")" = {q(self.production_waiting)}\n'
        for path, digest in self.profile.get("encoder_hashes", {}).items():
            if path not in {"/usr/bin/openssl", "/usr/lib/libssl.so.3", "/usr/lib/libcrypto.so.3"} or not re.fullmatch("[0-9a-f]{64}", digest):
                raise Refused("Exact encoder/provider path and digest required")
            text += f'test -f {q(path)}; test ! -L {q(path)}\ntest "$(stat -c \'%u %h\' {q(path)})" = \'0 1\'\n'
            if not snapshot:
                text += f'test "$(sha256sum {q(path)} | cut -d " " -f 1)" = {q(digest)}\n'
        for name, digest in self.profile["tools"].items():
            allowed_tools = {"owned_tap_guardian", "observe_input", "publish_creation_arm"} | set(CAPTURE_TOOLS)
            if self.fixture_inventory[0] == SOURCE5: allowed_tools = allowed_tools | {"owned_relative_gesture"}
            if name not in allowed_tools or not re.fullmatch("[0-9a-f]{64}", digest):
                raise Refused("Bounded exact tool identity required")
            text += f'test -f "$root/{name}"; test ! -L "$root/{name}"\ntest "$(stat -c \'%a %u %h\' "$root/{name}")" = \'700 0 1\'\n'
            if not snapshot:
                text += f'test "$(sha256sum "$root/{name}" | cut -d " " -f 1)" = {q(digest)}\n'
        return text

    def snapshot_artifact_checks(self, prefix):
        encoders = {'/usr/bin/openssl', '/usr/lib/libssl.so.3', '/usr/lib/libcrypto.so.3'}
        tools = {'owned_tap_guardian', 'observe_input', 'publish_creation_arm'} | set(CAPTURE_TOOLS)
        if self.fixture_inventory[0] == SOURCE5:
            tools = tools | {'owned_relative_gesture'}
        if set(self.profile.get('encoder_hashes', {})) != encoders or set(self.profile['tools']) != tools:
            raise Refused('Complete snapshot encoder and helper pins required')
        entries = [(q(path), path, self.profile['encoder_hashes'][path]) for path in sorted(encoders)]
        entries += [(f'"$root/{name}"', f'$root/{name}', self.profile['tools'][name]) for name in sorted(tools)]
        return ordered_hash_checks(f'$root/{prefix}.artifact-hashes', entries)

    def sidecar_prepare(self, prefix):
        script = ''
        for ext in ('content', 'metadata'):
            script += f'test ! -e "$root/{prefix}.{ext}"\ntest ! -L "$root/{prefix}.{ext}"\n'
            script += f'initial_{ext}_result=$(sha256sum "$doc.{ext}")\ninitial_{ext}=${{initial_{ext}_result%% *}}\n'
            script += f'test "${{#initial_{ext}}}" = 64\ncase "$initial_{ext}" in *[!0-9a-f]*) exit 90;; esac\n'
            script += f'test "$initial_{ext}_result" = "$initial_{ext}  $doc.{ext}"\n'
            copier=f'dd if="$doc.{ext}" bs=1 count=65537 2>/dev/null' if self.frame_ledger is not None else f'cat "$doc.{ext}"'
            script += f'(set -C; {copier} > "$root/{prefix}.{ext}")\nprivate "$root/{prefix}.{ext}"\n'
            if self.frame_ledger is not None:
                script += f'test "$(stat -c \'%s\' "$root/{prefix}.{ext}")" -le 65536\n'
            script += f'copied_{ext}_result=$(sha256sum "$root/{prefix}.{ext}")\n'
            script += f'test "$copied_{ext}_result" = "$initial_{ext}  $root/{prefix}.{ext}"\n'
        return script

    def sidecar_close(self, prefix):
        return ''.join(f'closing_{ext}_result=$(sha256sum "$doc.{ext}")\n'
                       f'test "$closing_{ext}_result" = "$initial_{ext}  $doc.{ext}"\n'
                       for ext in ('content', 'metadata'))

    def raw_snapshot(self, stage, cutoff, idle):
        if self.capture_failed:
            raise Refused("Prior capture failed; no further acquisition")
        if getattr(self,"capture_scope",False):
            raise Refused("Nested capture is not permitted")
        if stage not in {"ready","open","trigger","result","diagnostic","normalization-source","inserted-destination"}:
            raise Refused("Unknown snapshot stage")
        if self.is_diagnostic() != (stage=="diagnostic"):
            raise Refused("Snapshot stage does not match explicit purpose")
        if type(cutoff) is not int or cutoff<=0:
            raise Refused("Fixed canonical snapshot cutoff required")
        if stage in {"normalization-source","inserted-destination"} and (
            self.frame_ledger is None or self.reconciled_creation is None or self.navigation_phase is None):
            raise Refused("Closed postcreation phase lacks reconciliation")
        try:
            if self.frame_ledger is not None:
                try: self.frame_ledger.claim(reserved=stage in POST_STAGES)
                except ValueError as error: raise Refused(str(error)) from error
                self.active_capture_claim=(stage,self.frame_ledger.frames)
            self.capture_scope=self.frame_ledger is not None
            return self._raw_snapshot(stage, cutoff, idle)
        except Exception:
            self.capture_failed = True
            raise
        finally:
            self.capture_scope=False
            self.active_capture_claim=None

    def snapshot_timeout(self, cutoff):
        if type(cutoff) is not int or cutoff <= 0:
            raise Refused("Fixed canonical snapshot cutoff required")
        remote, anchor, fixed_deadline = self.clock_query(cutoff)
        now = time.monotonic()
        elapsed = now - anchor
        if elapsed < 0:
            raise Refused("Monotonic clock regressed")
        remaining = cutoff - remote / 1000 - elapsed
        if remaining <= 0:
            raise Refused("Snapshot response fixed deadline exhausted")
        deadline = min(fixed_deadline,
                       now + CAPTURE_CONTRACT['maximum_response_seconds'])
        if deadline <= now:
            raise Refused('Immutable snapshot stage exhausted before acquisition')
        return deadline - now, deadline

    def _raw_snapshot(self, stage, cutoff, idle):
        diagnostic = stage == 'diagnostic'
        if self.is_diagnostic() != diagnostic:
            raise Refused('Snapshot stage does not match explicit purpose')
        if stage not in {"ready", "open", "trigger", "result", 'diagnostic', "normalization-source", "inserted-destination"}:
            raise Refused("Unknown snapshot stage")
        if self.navigation_phase is not None:
            self.navigation_phase.capture()
            self.call_deadline=self.navigation_phase.capture_end
        if stage in {"normalization-source","inserted-destination"} and (self.frame_ledger is None or self.reconciled_creation is None or self.navigation_phase is None):
            raise Refused("Closed postcreation phase lacks reconciliation")
        if self.frame_ledger is not None and (
            not getattr(self,"capture_scope",False)
            or getattr(self,"active_capture_claim",None)!=(stage,self.frame_ledger.frames)):
            raise Refused("Snapshot implementation requires its active frame reservation")
        script = self.prelude(cutoff, self.identity, snapshot=True)
        if self.is_v2() and stage not in POST_STAGES:
            script += self.production_closure_checks()
        prefix = "host-" + stage
        if diagnostic:
            if not idle or self.diagnostic_context is None or cutoff != self.diagnostic_end or self.diagnostic_frames >= 3:
                raise Refused('Bounded diagnostic context, cutoff and full idle required')
            prefix += f'-{self.diagnostic_frames:03}'
            self.diagnostic_frames += 1
            script += self.diagnostic_closure_checks()
        if stage == "ready":
            if not idle or self.ready_frames >= CAPTURE_CONTRACT['maximum_ready_frames']:
                raise Refused("Bounded ready observation with full idle required")
            if self.ready_cutoff is not None and cutoff != self.ready_cutoff:
                raise Refused("Ready cutoff cannot be renewed")
            self.ready_cutoff = cutoff
            prefix += f"-{self.ready_frames:03}"
            self.ready_frames += 1
        if stage == "open":
            if self.open_frames >= CAPTURE_CONTRACT['maximum_open_frames']:
                raise Refused("Bounded open observation frame count exhausted")
            prefix += f"-{self.open_frames:03}"
            self.open_frames += 1
        if self.frame_ledger is not None: self.capture_prefixes.append(prefix)
        # Claim precedes every prefix output, including idle observations.
        script += f'(set -C; printf %s {q(self.profile["nonce"])} > "$root/{prefix}.claim")\n'
        mark = lambda label: f'diagnostic_phase {label} || exit 90\n' if diagnostic else ''
        if diagnostic:
            script += self.diagnostic_phase_setup(prefix)
        script += mark('artifact_begin')
        script += self.snapshot_artifact_checks(prefix)
        script += mark('artifact_end') + mark('sidecar_prepare_begin')
        script += self.sidecar_prepare(prefix)
        if self.frame_ledger is not None:
            script+=f'initial_conf=$(sha256sum /home/root/.config/remarkable/xochitl.conf | cut -d " " -f 1)\n(set -C; dd if=/home/root/.config/remarkable/xochitl.conf bs=1 count=65537 2>/dev/null > "$root/{prefix}.conf")\ntest "$(stat -c \'%s\' "$root/{prefix}.conf")" -le 65536\n'
        script += mark('sidecar_prepare_end') + mark('idle_begin')
        # Every output is nonce-root-owned and exclusive. Snapshot failure cannot
        # renew a stage or lead to a tap/arm retry.
        script += f'test ! -e "$root/{prefix}.png"; test ! -L "$root/{prefix}.png"\n'
        if idle:
            script += f'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --observe "$root/observe_input" 2 > "$root/{prefix}.idle" 2>&1\n'
            script += '/usr/bin/evtest --query /dev/input/event1 EV_KEY BTN_TOUCH\n/usr/bin/evtest --query /dev/input/event1 EV_KEY BTN_TOOL_PEN\n'
        else:
            script += f': > "$root/{prefix}.idle"\n'
        script += mark('idle_end')
        script += capture_script(prefix, self.profile["nonce"], self.profile["boot_id"], diagnostic=diagnostic, limited=self.frame_ledger is not None)
        script += mark('sidecar_close_begin')
        script += self.sidecar_close(prefix)
        if self.frame_ledger is not None:
            script+=f'test "$(sha256sum /home/root/.config/remarkable/xochitl.conf | cut -d " " -f 1)" = "$initial_conf"\ntest "$(sha256sum "$root/{prefix}.conf" | cut -d " " -f 1)" = "$initial_conf"\n'
        script += mark('sidecar_close_end') + mark('immutable_begin')
        paths = " ".join(q(p) for p in self.profile["immutable_hashes"])
        script += f'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --hashes /usr/bin/sha256sum {paths} > "$root/{prefix}.hashes"\n'
        script += f'private "$root/{prefix}.hashes"\ntest "$(stat -c \'%s\' "$root/{prefix}.hashes")" -le 4096\n'
        script += mark('immutable_end')
        script += 'test "$(sed \'s/^.*) //\' /proc/$pid/stat | awk \'{print $20}\')" = "$start"\n'
        script += 'test "$(now_upper)" -lt "$cutoff"\n'
        script += 'test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"\n'
        script += 'for name in entry.closed restore.claim stop.request stop.admitted; do test ! -e "$root/$name"; test ! -L "$root/$name"; done\n'
        script += 'test "$(stat -c \'%a %u\' "$root")" = \'700 0\'\nprivate "$root/owner"\n'
        script += f'test "$(cat "$root/owner")" = {q(self.profile["nonce"])}\n'
        if self.root_inode:
            script += f'test "$(stat -c \'%d %i\' "$root")" = {q(self.root_inode)}\n'
        if diagnostic:
            script += self.diagnostic_closure_checks()
        if self.is_v2() and stage not in POST_STAGES:
            script += self.production_closure_checks()
        if self.frame_ledger is not None and stage in POST_STAGES:
            target=self.profile["target_id"]
            for label,path in (("target-ink",self.doc+"/"+target+".rm"),("target-thumb",self.doc+".thumbnails/"+target+".png")):
                script+=f'if test -L {q(path)}; then printf "symlink\\n" > "$root/{prefix}.{label}.stat"; : > "$root/{prefix}.{label}.hash"; elif test -f {q(path)}; then stat -c "%a %u %h %s" {q(path)} > "$root/{prefix}.{label}.stat"; sha256sum {q(path)} > "$root/{prefix}.{label}.hash"; elif test -e {q(path)}; then printf "other\\n" > "$root/{prefix}.{label}.stat"; : > "$root/{prefix}.{label}.hash"; else test -d {q(path.rsplit("/",1)[0])}; test ! -L {q(path.rsplit("/",1)[0])}; printf "absent\\n" > "$root/{prefix}.{label}.stat"; : > "$root/{prefix}.{label}.hash"; fi\n'
            script+=f'find {q(self.doc)} {q(self.doc+".thumbnails")} -maxdepth 1 -name {q(target+"*")} > "$root/{prefix}.target-paths"\n'
        script += mark('response_begin')
        script += r"""printf '{"pid":%s,"start_ticks":%s,"root_inode":"%s","window":"%s","waiting":"%s","capture_uptime":"%s","last_open":"%s",' "$pid" "$start" "$(stat -c '%d %i' "$root")" "$(b64 "$root/candidate.window")" "$(b64 "$root/creation-waiting")" "$(b64 /proc/uptime)" "$(grep '^LastOpen' /home/root/.config/remarkable/xochitl.conf | /usr/bin/openssl base64 -A)"
"""
        script += f'printf \'"frame_hash":"%s","native_hash":"%s","capture_receipt":"%s",\' "$frame_hash" "$native_hash" "$(b64 "$root/{prefix}.capture")"\n'
        if self.frame_ledger is not None and stage in POST_STAGES:
            script+='private "$root/creation-arm"\n'
            script+='printf \'"creation_arm_raw":"%s",\' "$(b64 "$root/creation-arm")"\n'
            for key in ("target-ink.stat","target-ink.hash","target-thumb.stat","target-thumb.hash","target-paths"):
                script+=f'printf \'"{key}":"%s",\' "$(b64 "$root/{prefix}.{key}")"\n'
        if self.frame_ledger is not None:
            script+=f'printf \'"config_raw":"%s",\' "$(b64 "$root/{prefix}.conf")"\n'
        for key in ("content", "metadata", "png", "hashes", "idle"):
            script += f'printf \'"{key}":"%s",\' "$(b64 "$root/{prefix}.{key}")"\n'
        script += r"""if test -f "$root/diagnostics.json" && test ! -L "$root/diagnostics.json"; then
 private "$root/diagnostics.json"
 printf '"diagnostics":"%s",' "$(b64 "$root/diagnostics.json")"
else printf '"diagnostics":"",'; fi
test "$(stat -c '%d %i' "$root")" = "$initial_root_identity"
test "$(sed 's/^.*) //' /proc/$pid/stat | awk '{print $20}')" = "$start"
test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"
for name in entry.closed restore.claim stop.request stop.admitted; do test ! -e "$root/$name"; test ! -L "$root/$name"; done
test "$(now_upper)" -lt "$cutoff"
"""
        if diagnostic:
            script += self.diagnostic_closure_checks() + mark('response_tail')
            script += f'private "$root/{prefix}.phases"\ntest "$(stat -c \'%s\' "$root/{prefix}.phases")" -le 1024\n'
            script += f'printf \'"phases":"%s",\' "$(b64 "$root/{prefix}.phases")"\n'
        script += 'printf \'"uptime":"%s"}\\n\' "$(b64 /proc/uptime)"\n'
        # Retain the original SDK setup bound through pre-arm decoding and
        # retention. Result access has its separate accepted A+5 interval.
        sdk_deadline = None
        if self.is_v2() and stage not in POST_STAGES and self.sdk_expiry_ms is not None:
            sdk_deadline = self.known_deadline(self.sdk_expiry_ms / 1000)
            if sdk_deadline is None:
                raise Refused('Original SDK local clock context required')
            stage_deadline = self.known_deadline(cutoff)
            self.fixed_local_deadlines[cutoff] = min(stage_deadline, sdk_deadline) if stage_deadline is not None else sdk_deadline
        timeout, response_deadline = self.snapshot_timeout(cutoff)
        if sdk_deadline is not None:
            response_deadline = min(response_deadline, sdk_deadline)
            timeout = min(timeout, response_deadline - time.monotonic())
            if timeout <= 0:
                raise Refused('Original SDK setup expired before acquisition')
        if self.navigation_phase is not None:
            response_deadline = min(response_deadline, self.navigation_phase.capture_end)
            timeout = min(timeout,response_deadline-time.monotonic())
        self.capture_rpc=self.navigation_phase is not None
        try:
            response=(self.call(script,timeout,record_deadline=response_deadline)
                      if self.frame_ledger is not None else self.call(script,timeout))
        finally: self.capture_rpc=False
        if (self.last_response_arrival if self.navigation_phase is not None else time.monotonic()) >= response_deadline:
            raise Refused("Complete snapshot response arrived after fixed deadline")
        if self.navigation_phase is not None:
            response_deadline = self.navigation_phase.processing_end
        args=(response,self.profile["nonce"],diagnostic,self.diagnostic_end,self.root_inode,response_deadline,
            self.frame_ledger is not None and stage in POST_STAGES,self.profile["boot_id"],idle)
        try:
            observed=bounded_local(decode_snapshot,args,response_deadline) if self.navigation_phase is not None else decode_snapshot(*args)
        except ValueError as error: raise Refused(str(error)) from error
        self.root_inode=observed.pop("_root_inode")
        if self.frame_ledger is not None:
            self.capture_sources[prefix]={"claim":self.frame_ledger.frames,"phase":stage,
                "sha256":hashlib.sha256(observed["png"]).hexdigest(),"root_inode":self.root_inode}
            observed["capture_prefix"]=prefix
        return observed

    def wait_library(self):
        if self.is_diagnostic():
            raise Refused('Diagnostic purpose cannot enter production readiness')
        # This loop only waits for existing actor/SDK markers. No capture/input
        # retry and no deadline renewal. Unknown timing never permits mutation.
        end = time.monotonic() + 110
        while time.monotonic() < end:
            script = f"""set -eu
root={q(self.root)}
test ! -e "$root/restore.claim"; test ! -L "$root/restore.claim"; test ! -e "$root/stock.restored"; test ! -L "$root/stock.restored"; test ! -e "$root/recovery.failed"; test ! -L "$root/recovery.failed"
if test -f "$root/candidate.window" && test -f "$root/creation-waiting" && test -f "$root/attempt.identity"; then cat "$root/candidate.window"; fi
"""
            if self.is_v2():
                script = script.replace('cat "$root/candidate.window"; fi', 'cat "$root/candidate.window"; cat /proc/uptime; fi')
            marker_anchor = time.monotonic()
            line = self.call(script, min(3, max(0.001, end-marker_anchor))).strip()
            if line:
                if self.is_v2():
                    lines = line.splitlines()
                    if len(lines) != 2:
                        raise Refused('Candidate clock context required before acquisition')
                    line, marker_uptime = lines
                values = line.split()
                if len(values) != 5 or values[0] != self.profile["nonce"]:
                    raise Refused("Waiting actor window invalid")
                timing = OpenDocumentTiming(*map(int, values[1:4]), self.profile.get('experiment_kind', 'open-document-insertion-v1'))
                if int(values[4]) != timing.observation_end:
                    raise Refused('Fixed candidate cutoff mismatch')
                cutoff = timing.stage_end('ready')
                if self.is_v2():
                    self.candidate_remote_at = timing.candidate_at
                    self.candidate_local_anchor = marker_anchor + timing.candidate_at - upper_uptime_ms(marker_uptime+'\n') / 1000
                self.production_window = line
                raw = self.raw_snapshot("ready", cutoff, True)
                self.identity = (raw["boot_id"], raw["pid"], raw["start_ticks"])
                if self.is_v2():
                    waiting = parse_waiting(raw['waiting'], self.profile['nonce'], raw['pid'], raw['start_ticks'])
                    self.production_waiting = raw['waiting'].decode('ascii').rstrip('\n')
                    self.sdk_expiry_ms = waiting.expiry_ms
                return raw
            time.sleep(0.2)
        raise Refused("Candidate waiting markers not observed in fixed setup allowance")

    def wait_diagnostic_context(self):
        if self.capture_failed:
            raise Refused('Prior diagnostic failure; no repeated setup')
        try:
            return self._wait_diagnostic_context()
        except Exception:
            self.capture_failed = True
            raise

    def _wait_diagnostic_context(self):
        if not self.is_diagnostic() or self.diagnostic_context is not None:
            raise Refused('Fresh explicit diagnostic context required')
        end = time.monotonic() + 110
        while time.monotonic() < end:
            script = f'''set -eu
root={q(self.root)}
test ! -e "$root/restore.claim"
test ! -L "$root/restore.claim"
test ! -e "$root/stock.restored"
test ! -e "$root/recovery.failed"
'''
            script += self.diagnostic_closure_checks()
            script += f'''
if test -f "$root/candidate.window" && test -f "$root/creation-waiting" && test -f "$root/attempt.identity"; then
 test -d "$root"
 test ! -L "$root"
 test "$(stat -c '%a %u' "$root")" = '700 0'
 test "$(cat "$root/owner")" = {q(self.profile['nonce'])}
 test "$(cat /proc/sys/kernel/random/boot_id)" = {q(self.profile['boot_id'])}
'''
            script += self.diagnostic_closure_checks()
            script += ''' cat "$root/candidate.window"
 cat "$root/creation-waiting"
 stat -c '%d %i' "$root"
 cat "$root/attempt.identity"
 cat /proc/uptime
fi
'''
            response = self.call(script, 3)
            if response:
                if len(response.encode('ascii')) > 1024:
                    raise Refused('Diagnostic context exceeds bound')
                lines = response.splitlines()
                if len(lines) != 5:
                    raise Refused('Complete bounded diagnostic context required')
                window = lines[0].split()
                identity = lines[3].split()
                if len(window) != 5 or window[0] != self.profile['nonce'] or any(not re.fullmatch('[0-9]+', v) for v in window[1:]):
                    raise Refused('Fixed diagnostic window refused')
                if len(identity) != 2 or any(not re.fullmatch('[1-9][0-9]*', v) for v in identity) or int(identity[0]) <= 1 or not re.fullmatch('[0-9]+ [0-9]+', lines[2]):
                    raise Refused('Diagnostic process/root identity refused')
                waiting = (lines[1] + '\n').encode('ascii')
                parsed = parse_waiting(waiting, self.profile['nonce'], int(identity[0]), int(identity[1]))
                timing = OpenDocumentTiming(*map(int, window[1:4]))
                if int(window[4]) != timing.observation_end:
                    raise Refused('Diagnostic actor observation end changed')
                cutoff = diagnostic_cutoff(timing, parsed.expiry_ms)
                uptime = lines[4] + '\n'
                if upper_uptime_ms(uptime) >= cutoff * 1000:
                    raise Refused('Diagnostic fixed interval already expired')
                context = {'window': [window[0]] + list(map(int, window[1:])), 'boot_id': self.profile['boot_id'],
                           'pid': int(identity[0]), 'start_ticks': int(identity[1]), 'root_inode': lines[2],
                           'waiting': waiting, 'uptime': uptime, 'closed': False}
                self.identity = (context['boot_id'], context['pid'], context['start_ticks'])
                self.root_inode = context['root_inode']
                self.diagnostic_context = context
                self.diagnostic_end = cutoff
                return context
            time.sleep(0.2)  # Existing marker setup wait, never a render delay.
        raise Refused('Diagnostic context absent within original setup allowance')

    def open_once(self, identity, cutoff):
        self.require_publication()
        tap_command, _ = self.selected_tap()
        if self.tap_attempted: raise Refused("Ordinary open already attempted")
        if self.frame_ledger is not None:
            prefix=f"host-ready-{self.ready_frames-1:03}"
            captured=self.capture_sources.get(prefix)
            if (self.guardian_source_claim is not None or captured is None
                or captured["phase"]!="ready" or captured["root_inode"]!=self.root_inode
                or captured["claim"]!=self.frame_ledger.frames):
                raise Refused("Guardian outputs require the latest accepted READY frame")
            # Three exclusive guardian histories and fixed receipts consume the
            # READY frame's audited spare bytes, even if launch/release fails.
            self.guardian_source_claim=(prefix,captured["claim"])
        self.tap_attempted = True
        script = self.prelude(cutoff, identity)
        if self.is_v2():
            script += self.production_closure_checks()
        script += 'test $((cutoff-$(now_upper))) -ge 4\n'
        # Detached sole writer survives SSH loss; never wrap it in bounded().
        script += 'test ! -e "$root/tap-start.claim"; test ! -L "$root/tap-start.claim"\n'
        script += f'(set -C; printf \'%s\\n\' {q(self.profile["nonce"])} > "$root/tap-start.claim")\n'
        # Select a setup cutoff once from fresh remote BOOTTIME, capped by the
        # original open stage. Ceil+14 seconds is at most 15 seconds from now.
        script += f'tap_cutoff_ms=$(( $(now_upper)*1000 + 14000 ))\ntest "$tap_cutoff_ms" -le {int(cutoff) * 1000} || tap_cutoff_ms={int(cutoff) * 1000}\n'
        script += f'nohup "$root/owned_tap_guardian" {tap_command} {q(self.profile["nonce"])} {q(self.profile["boot_id"])} "$tap_cutoff_ms" < /dev/null > "$root/tap-guardian.log" 2>&1 &\n'
        script += r"""while ! test -f "$root/tap.ready"; do
 test ! -e "$root/tap.unresolved"; test ! -L "$root/tap.unresolved"; test "$(now_upper)" -lt "$cutoff"
 sleep 0.01
done
private "$root/tap.ready"
private "$root/tap.setup"
printf '{"ready":"%s","setup":"%s","selected_cutoff_ms":%s,"uptime":"%s"}\n' "$(b64 "$root/tap.ready")" "$(b64 "$root/tap.setup")" "$tap_cutoff_ms" "$(b64 /proc/uptime)"
"""
        timeout, ready_deadline = self.action_budget(cutoff, 3, self.sdk_expiry_ms) if self.is_v2() else (3, None)
        request_started = time.monotonic()
        envelope = json.loads(self.call(script, timeout))
        self.action_guard(ready_deadline)
        ready_bytes = base64.b64decode(envelope["ready"], validate=True)
        setup_bytes = base64.b64decode(envelope["setup"], validate=True)
        if not setup_bytes or len(setup_bytes) > 1024:
            raise Refused("Bounded complete guardian setup required")
        ready = self.tap_receipt(ready_bytes, "guardian-ready-no-down", cutoff)
        selected_cutoff = envelope["selected_cutoff_ms"]
        uptime = base64.b64decode(envelope["uptime"], validate=True).decode("ascii")
        if type(selected_cutoff) is not int or not upper_uptime_ms(uptime) < ready["lease_end_ms"] <= selected_cutoff <= int(cutoff) * 1000:
            raise Refused("Guardian immutable setup cutoff not observed")
        self.retain("tap-ready", {"receipt": ready_bytes, "setup": setup_bytes, "selected_cutoff_ms": selected_cutoff, "uptime": uptime})
        self.action_guard(ready_deadline)
        # Anchor before transport, conservatively charge the complete request
        # plus decoding/retention against the remotely observed setup lease.
        lease_deadline = request_started + (ready['lease_end_ms'] - upper_uptime_ms(uptime)) / 1000
        if self.is_v2() and time.monotonic() >= lease_deadline:
            raise Refused('Original guardian setup lease expired before GO')
        # Publish exactly once, only after complete ready/lease evidence is saved.
        script = self.prelude(cutoff, identity)
        if self.is_v2():
            script += self.production_closure_checks()
        script += f'test "$(sha256sum "$root/tap.ready" | cut -d " " -f 1)" = {q(hashlib.sha256(ready_bytes).hexdigest())}\n'
        script += f'test "$(sed \'s/^.*) //\' /proc/{ready["pid"]}/stat | awk \'{{print $20}}\')" = {q(ready["start_ticks"])}\n'
        script += 'test ! -e "$root/tap.go"; test ! -L "$root/tap.go"\n'
        script += f'(set -C; "$root/owned_tap_guardian" --publish-go {q(self.profile["nonce"])} {q(self.profile["boot_id"])} > "$root/tap.go-publication.json" 2> "$root/tap.go-publication.log")\n'
        script += r"""while ! test -f "$root/tap.released" || ! test -f "$root/tap.supervisor-complete"; do
 for name in tap.unresolved tap.empty tap.cleaned tap.recovered tap.supervisor-spent; do test ! -e "$root/$name"; test ! -L "$root/$name"; done
 test "$(now_upper)" -lt "$cutoff"
 sleep 0.01
done
private "$root/tap.released"
private "$root/tap.supervisor-complete"
private "$root/tap.lease"
private "$root/tap.go-publication.json"
printf '{"released":"%s","supervisor":"%s","lease":"%s","publication":"%s"}\n' "$(b64 "$root/tap.released")" "$(b64 "$root/tap.supervisor-complete")" "$(b64 "$root/tap.lease")" "$(b64 "$root/tap.go-publication.json")"
"""
        if self.is_v2():
            timeout, go_deadline = self.action_budget(cutoff, 3, self.sdk_expiry_ms)
            go_deadline = min(go_deadline, lease_deadline)
            self.action_guard(go_deadline)
            timeout = min(timeout, go_deadline - time.monotonic())
            self.action_deadline = go_deadline
        else:
            timeout, go_deadline = 3, None
        completed = json.loads(self.call(script, timeout))
        self.action_guard(go_deadline)
        publication_bytes = base64.b64decode(completed["publication"], validate=True)
        self.retain("tap-go-publication", {"receipt": publication_bytes})
        self.action_guard(go_deadline)
        if len(publication_bytes) > 512 or not publication_bytes.endswith(b"\n"):
            raise Refused("Bounded atomic tap permission receipt required; never replay")
        publication = json.loads(publication_bytes)
        if (set(publication) != {"nonce", "published", "retry_permitted", "input_events"} or
                publication["nonce"] != self.profile["nonce"] or publication["published"] is not True or
                publication["retry_permitted"] is not False or type(publication["input_events"]) is not int or
                publication["input_events"] != 0):
            raise Refused("Atomic tap permission publication not confirmed; never replay")
        released_bytes = base64.b64decode(completed["released"], validate=True)
        supervisor_bytes = base64.b64decode(completed["supervisor"], validate=True)
        lease_bytes = base64.b64decode(completed["lease"], validate=True)
        if not lease_bytes or len(lease_bytes) > 1024:
            raise Refused("Bounded complete contact lease required")
        released = self.tap_receipt(released_bytes, "guardian-released", cutoff)
        if released["lease_end_ms"] > ready["lease_end_ms"] or released | {"status": ready["status"], "lease_end_ms": ready["lease_end_ms"]} != ready:
            raise Refused("Guardian ready/released identity changed")
        supervisor = self.tap_receipt(supervisor_bytes, "normal-child-exit", cutoff)
        if supervisor | {"status": released["status"]} != released:
            raise Refused("Supervisor/contact lease identity changed")
        self.retain("tap-released", {"receipt": released_bytes, "supervisor": supervisor_bytes, "lease": lease_bytes})
        self.action_guard(go_deadline)
        self.tap_released = True
        self.tap_release_sha256 = hashlib.sha256(released_bytes).hexdigest()
        self.tap_supervisor_sha256 = hashlib.sha256(supervisor_bytes).hexdigest()

    def tap_receipt(self, data, status, cutoff):
        if len(data) > 512 or not data.endswith(b"\n"):
            raise Refused("Bounded complete guardian receipt required")
        value = json.loads(data)
        fields = {"nonce", "boot_id", "pid", "start_ticks", "slot", "tracking_id", "native_point", "lease_end_ms", "status", "exclusive_kernel_ownership"}
        if set(value) != fields or value["nonce"] != self.profile["nonce"] or value["boot_id"] != self.profile["boot_id"]:
            raise Refused("Guardian receipt identity mismatch")
        if any(type(value[key]) is not int or value[key] <= 0 for key in ("pid", "start_ticks", "tracking_id", "lease_end_ms")):
            raise Refused("Canonical guardian process/lease identity required")
        if type(value["native_point"]) is not list or len(value["native_point"]) != 2 or any(type(axis) is not int for axis in value["native_point"]) or value["pid"] <= 1 or value["tracking_id"] > 65535 or type(value["slot"]) is not int or value["slot"] != 0 or value["native_point"] != self.selected_tap()[1] or value["lease_end_ms"] > int(cutoff) * 1000 or value["status"] != status or value["exclusive_kernel_ownership"] is not False:
            raise Refused("Guardian release/point/deadline not admitted")
        return value

    def snapshot(self, stage, cutoff, idle):
        return self.raw_snapshot(stage, cutoff, idle)

    def source5_prepublication_check(self,name):
        from open_document_timing import SOURCE5_CONTROL_SHELL_SHA256
        if name not in {"runtime.files","document-immutable.files"}:
            raise Refused("Closed prepublication check pair required")
        output="host-prepublication-"+name+".check"
        script='test "$(readlink -f /bin/sh)" = /usr/bin/bash.bash\n'
        script+=f'test "$(sha256sum /usr/bin/bash.bash | cut -d " " -f 1)" = {q(SOURCE5_CONTROL_SHELL_SHA256)}\n'
        script+='(\nclient_pid=$BASHPID\nulimit -c 0 || exit 90\nulimit -f 8 || exit 90\n'
        script+='awk \'$1=="Max" && $2=="file" && $3=="size" {f++; if($4!=4096 || $5!=4096 || $6!="bytes") bad=1} $1=="Max" && $2=="core" && $3=="file" && $4=="size" {c++; if($5!=0 || $6!=0) bad=1} END {exit !(f==1 && c==1 && !bad)}\' "/proc/$client_pid/limits" || exit 90\n'
        script+=f'private "$root/{output}"\nstream=$(stat -c "%a %u %h %d %i" "$root/{output}")\n'
        for fd in (1,2):
            script+=f'test -f "/proc/$client_pid/fd/{fd}" || exit 90\ntest "$(stat -Lc \'%a %u %h %d %i\' "/proc/$client_pid/fd/{fd}")" = "$stream" || exit 90\n'
        script+=f'exec "$root/owned_frame_alarm" --check /usr/bin/sha256sum -c "$root/{name}"\n) > "$root/{output}" 2>&1\n'
        script+=f'private "$root/{output}"\ntest "$(stat -c %s "$root/{output}")" -lt 4096\n'
        return script

    def arm_once(self, waiting, timing, ready):
        self.require_publication()
        self.selected_tap()
        if self.arm_attempted: raise Refused("Creation publication already attempted")
        if not self.tap_released: raise Refused("Positive guardian release required before creation")
        if self.frame_ledger is not None:
            self.reserve_postnative_control()
            captured=self.capture_sources.get("host-trigger")
            if (self.trigger_source_claim is not None or captured is None or not isinstance(ready,dict)
                or ready.get("capture_prefix")!="host-trigger" or captured["phase"]!="trigger"
                or captured["root_inode"]!=self.root_inode or captured["claim"]!=self.frame_ledger.frames
                or captured["sha256"]!=hashlib.sha256(ready["png"]).hexdigest()):
                raise Refused("Publication outputs require the latest accepted TRIGGER frame")
            self.trigger_source_claim=("host-trigger",captured["claim"])
        self.arm_attempted = True
        script = self.prelude(timing.stage_end("trigger"), self.identity)
        if self.is_v2():
            script += self.production_closure_checks()
            script += f'test $(({timing.observation_end}-$(now_upper))) -ge 40\n'
            script += f'test $(({timing.deadline}-$(now_upper))) -ge 225\n'
            script += f'test $(({timing.deadline}-$(now_upper))) -ge $(({timing.observation_end}-$(now_upper)+185))\n'
        # Recheck the exact actor partitions after the host observation and
        # immediately before publishing permission. Observation is not a lock.
        for name in ("runtime.files", "document-immutable.files"):
            script += f'private "$root/{name}"\n'
            script += f'test ! -e "$root/host-prepublication-{name}.check"\ntest ! -L "$root/host-prepublication-{name}.check"\n'
            script += f'test "$(sha256sum "$root/{name}" | cut -d " " -f 1)" = {q(self.receipt["files"][name])}\n'
            script += 'test "$(now_upper)" -le $((cutoff-4))\n'
            if self.frame_ledger is not None: script+=self.source5_prepublication_check(name)
            else: script+=f'"$root/owned_frame_alarm" --check /usr/bin/sha256sum -c "$root/{name}" > "$root/host-prepublication-{name}.check" 2>&1\n'
        for ext in ("content", "metadata"):
            # Compare exact captured remote bytes, not reserialized JSON. The
            # previously retained stage file is tied to this host observation.
            digest = ready.get(ext + "_raw_sha256")
            if not digest: raise Refused("Exact sidecar-byte binding missing")
            script += f'test "$(sha256sum "$doc.{ext}" | cut -d " " -f 1)" = {q(digest)}\n'
            if self.frame_ledger is not None:
                script+=f'private "$root/host-trigger.{ext}"\ntest "$(stat -c %s "$root/host-trigger.{ext}")" -le 65536\n'
            script += f'test "$(sha256sum "$root/host-trigger.{ext}" | cut -d " " -f 1)" = {q(digest)}\n'
            script += f'test ! -e "$root/host-source.{ext}"; test ! -L "$root/host-source.{ext}"\ncp "$root/host-trigger.{ext}" "$root/host-source.{ext}"\nchmod 600 "$root/host-source.{ext}"\n'
        if not self.tap_release_sha256 or not self.tap_supervisor_sha256:
            raise Refused("Exact retained guardian release binding missing")
        script += 'private "$root/tap.released"\n'
        script += f'test "$(sha256sum "$root/tap.released" | cut -d " " -f 1)" = {q(self.tap_release_sha256)}\n'
        script += 'private "$root/tap.supervisor-complete"\n'
        script += f'test "$(sha256sum "$root/tap.supervisor-complete" | cut -d " " -f 1)" = {q(self.tap_supervisor_sha256)}\n'
        script += 'for name in tap.unresolved tap.empty tap.cleaned tap.recovered tap.supervisor-spent; do test ! -e "$root/$name"; test ! -L "$root/$name"; done\n'
        if self.frame_ledger is not None:
            target=self.profile["target_id"]
            for path in (self.doc+"/"+target+".rm",self.doc+".thumbnails/"+target+".png"):
                script+=f'test ! -e {q(path)}; test ! -L {q(path)}\n'
            script+=f'test -z "$(find {q(self.doc)} {q(self.doc+".thumbnails")} -maxdepth 1 -name {q(target+"*")})"\n'
        args = [waiting.nonce, waiting.pid, waiting.start_ticks, waiting.expiry_ms, timing.armed_at, timing.deadline, timing.candidate_at, self.profile["document_id"]]
        for name in ('host-publication.json', 'host-publication.stderr'):
            script += f'test ! -e "$root/{name}"\ntest ! -L "$root/{name}"\n'
        script += 'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --publish "$root/publish_creation_arm" ' + " ".join(q(v) for v in args) + ' > "$root/host-publication.json" 2> "$root/host-publication.stderr"\ncat "$root/host-publication.json"\n'
        if self.is_v2():
            args.append(PRODUCTION_V2)
            # Rebuild only the final invocation; argument pins select the new
            # publisher preset explicitly, never through a field override.
            script = script.rsplit('"$root/owned_frame_alarm" --publish', 1)[0] + '"$root/owned_frame_alarm" --publish "$root/publish_creation_arm" ' + " ".join(q(v) for v in args) + ' > "$root/host-publication.json" 2> "$root/host-publication.stderr"\ncat "$root/host-publication.json"\n'
            timeout, deadline = self.action_budget(timing.stage_end('trigger'), 12, waiting.expiry_ms, full=True)
            self.action_guard(deadline)
            if deadline - time.monotonic() < 12:
                raise Refused('Publication reservation consumed before RPC entry')
        else:
            timeout, deadline = 4, None
        result = json.loads(self.call(script, timeout))
        self.action_guard(deadline)
        if self.frame_ledger is not None: self.target_absent_before_arm = True
        if result.get("published") is not True or result.get("retry_permitted") is not False:
            raise Refused("Publication uncertain; never replay")

    def result_snapshot(self, identity, cutoff):
        self.require_publication()
        if self.frame_ledger is not None:
            if self.result_poll_started: raise Refused("SOURCE5 result polling already entered; no retry")
            self.result_poll_started=True
        # SDK access window is at most five seconds. Poll diagnostics only, then
        # collect exactly one same-candidate frame/files snapshot before stopping.
        for _ in range(30):
            script = self.prelude(cutoff, identity) + r"""
if test -f "$root/diagnostics.json" && test ! -L "$root/diagnostics.json"; then cat "$root/diagnostics.json"; fi
"""
            if self.frame_ledger is not None and self.diagnostic_poll_count>=30:
                raise Refused("SOURCE5 diagnostic poll maximum reached; no retry")
            self.poll_clock_scope=self.frame_ledger is not None
            try:
                timeout, deadline = self.action_budget(cutoff, 3) if self.is_v2() else (3, None)
            finally:
                self.poll_clock_scope=False
            text = (self.call(script,timeout,record_role="poll") if self.frame_ledger is not None else self.call(script,timeout)).strip()
            self.action_guard(deadline)
            if text:
                # SDK creates the receipt before its single bounded write.
                # An incomplete read is observation-not-ready, never a reason
                # to replay input/publication or renew this fixed result cap.
                if len(text.encode("utf-8")) > 8192:
                    raise Refused("Native diagnostics exceed reviewed bound")
                try:
                    diagnostics = json.loads(text)
                except json.JSONDecodeError:
                    self.poll_pause(deadline)
                    continue
                if not isinstance(diagnostics, dict):
                    raise Refused("Native diagnostics object required")
                trial = diagnostics.get("creation_trial", {})
                self.action_guard(deadline)
                if diagnostics.get("terminal_stage") or trial.get("returned") or trial.get("exception"):
                    return self.raw_snapshot("result", cutoff, self.frame_ledger is not None)
            self.poll_pause(deadline)
        raise Refused("Native result not observed before fixed result cap")

    def retain_bounded(self,stage,evidence,deadline,control=True,maximum=None,reservation=None):
        # Claim the closed role's entire maximum before starting any worker or
        # creating even its scratch result. Unused/failed reservations remain used.
        if control:
            if stage.startswith("root-registration-"): cap=4096
            elif stage.startswith("gesture-"): cap=16384
            elif stage=="source5-report": cap=8192
            elif stage in {"source5-control-reservation","tap-ready","tap-go-publication","tap-released"}: cap=4096
            elif stage.startswith("transport-") and maximum in {2048,8192,16384,32768}: cap=maximum
            else: raise Refused("Unknown bounded control-record role")
            try:
                ticket=reservation if reservation is not None else self.take_postnative_record(stage)
                if ticket is None: ticket=self.control_ledger.reserve_record(cap)
                if self.control_ledger.consume_record(ticket)!=cap:
                    raise ValueError("Control reservation role mismatch")
            except ValueError as error: raise Refused(str(error)) from error
        else:
            cap=2*1024*1024
        try:
            # Account for the worker's exclusive result file concurrently with
            # the final record. The writer returns only a byte count/error.
            bounded_local(retain_record,(str(self.output/(stage+".json")),evidence,cap-512),
                deadline,result_limit=512)
        except ValueError as error: raise Refused(str(error)) from error

    def validate_creation_evidence(self,raw,waiting):
        if not getattr(self,"target_absent_before_arm",False): raise ValueError("Target absence before arm unobserved")
        token=f"{waiting.nonce} {waiting.pid} {waiting.start_ticks} create {waiting.expiry_ms}\n".encode()
        if raw.get("creation_arm_raw") != token: raise ValueError("Original absolute waiting/arm token changed")
        facts=raw.get("target_files",{})
        paths=(self.doc+"/"+self.profile["target_id"]+".rm",self.doc+".thumbnails/"+self.profile["target_id"]+".png")
        if sorted(facts.get("target-paths","").splitlines()) != sorted(paths):
            raise ValueError("Unexpected target-associated path")
        expected=(("target-ink",paths[0],424,"1dfdc53d0fb38c48758c6a0663df8182fb4a4b6f362f8b4b90cb02d5e7841462"),
            ("target-thumb",paths[1],1706,"9dbb20c8af2e99438341e492f5dfd61d21a26db8c9ff78db545cf7e0bdcdd603"))
        for label,path,size,digest in expected:
            if facts.get(label+".stat") != f"600 0 1 {size}\n" or facts.get(label+".hash") != f"{digest}  {path}\n":
                raise ValueError("Unsupported target file state; preserve and refuse")
        return True

    def publish_reconciliation(self,creation,deadline):
        data=canonical(creation)
        script=self.prelude(int(self.candidate_remote_at)+115,self.identity,snapshot=True)
        script+='test ! -e "$root/reconciled-creation.json"; test ! -L "$root/reconciled-creation.json"\n'
        script+=f'printf %s {q(base64.b64encode(data).decode())} | /usr/bin/openssl base64 -d -A > "$root/reconciled-creation.json"\n'
        script+='chmod 600 "$root/reconciled-creation.json"\nprivate "$root/reconciled-creation.json"\n'
        script+=f'test "$(sha256sum "$root/reconciled-creation.json" | cut -d " " -f 1)" = {q(hashlib.sha256(data).hexdigest())}\n'
        remaining=deadline-time.monotonic()
        if remaining<=0: raise Refused("Reconciliation staging exhausted")
        self.call(script,remaining,record_role="staging")
        if time.monotonic()>=deadline: raise Refused("Reconciliation arrived late")
        self.reconciled_creation=creation
        self.reconciled_creation_sha256=hashlib.sha256(data).hexdigest()

    def navigation_move(self,direction,current,source,creation,phase):
        from open_document_timing import validate_gesture_paths
        try: validate_gesture_paths(self.profile.get("gesture_paths"))
        except ValueError as error: raise Refused(str(error)) from error
        if (direction,current) not in {("previous",6),("next",5)} or self.frame_ledger is None:
            raise Refused("Closed nine-page movement only")
        phase.check(phase.staging_end)
        expected_prefix="host-result" if direction=="previous" else "host-normalization-source"
        expected_phase="result" if direction=="previous" else "normalization-source"
        captured=self.capture_sources.get(expected_prefix)
        if (source.get("capture_prefix")!=expected_prefix or captured is None
            or captured["phase"]!=expected_phase or captured["root_inode"]!=self.root_inode
            or captured["sha256"]!=hashlib.sha256(source["png"]).hexdigest()
            or captured["claim"] in self.helper_source_claims):
            raise Refused("No fresh claimed source frame owns helper staging")
        # This extra PNG/control copy consumes the existing source frame's
        # audited 25MiB reservation. Failed staging never releases that claim.
        self.helper_source_claims.add(captured["claim"])
        if creation is not None:
            if self.reconciled_creation is not None: raise Refused("Reconciliation already published")
            self.reconciled_creation=creation
            self.reconciled_creation_sha256=hashlib.sha256(canonical(creation)).hexdigest()
        if self.reconciled_creation is None: raise Refused("No reconciled native creation")
        nonce=uuid.uuid4().hex
        gesture_root="/run/rmb-gesture-"+nonce
        target=self.profile["target_id"]
        candidate_nonce=self.profile["nonce"]
        cutoff=int(self.candidate_remote_at)+115
        self.call_deadline=phase.staging_end
        self.clock_query(cutoff) # ALL query/retention cost is inside staging3.
        phase.check(phase.staging_end)
        lower_ms,arrival=self.clock_sample
        go_cutoff=min(cutoff*1000-5000,int(lower_ms+max(0,phase.staging_end-arrival)*1000))
        overall_ms=go_cutoff+5000
        binding={"variant":"source5-nine-after-insertion-v1","target_id":target,
            "candidate_nonce":candidate_nonce,"boot_id":self.identity[0],"pid":self.identity[1],
            "start_ticks":self.identity[2],"reconciled_creation_path":self.root+"/reconciled-creation.json",
            "reconciled_creation_sha256":self.reconciled_creation_sha256,
            "candidate_root_dev":int(self.root_inode.split()[0]),"candidate_root_ino":int(self.root_inode.split()[1]),
            "phase":"normalization-source" if direction=="previous" else "inserted-destination",
            "candidate_result_ceiling_ms":cutoff*1000,
            "candidate_window_sha256":self.reconciled_creation["candidate_window_sha256"]}
        view=self.profile["gesture_paths"][direction]
        config={"nonce":nonce,"boot":self.identity[0],"stock_pid":self.identity[1],"stock_start":self.identity[2],
            "document":self.profile["document_id"],"page_ids":self.reconciled_creation["page_ids"],
            "current_index":current,"direction":direction,"geometry_variant":view["geometry_variant"],
            "navigation_policy":view["navigation_policy"],"virtual_path":view["virtual_path"],"native_path":view["native_path"],
            "root_dev":0,"root_ino":0,"source_observed_ms":upper_uptime_ms(source["uptime"]),
            "go_not_after_ms":go_cutoff,"overall_ceiling_ms":overall_ms,
            "source_image_sha256":hashlib.sha256(source["png"]).hexdigest(),"source_image_verified":True,
            "sidecar_sha256":[hashlib.sha256(source["config_raw"]).hexdigest(),source["content_raw_sha256"],source["metadata_raw_sha256"]],
            "page_map_sha256":self.reconciled_creation["page_map_sha256"],"executable_sha256":self.profile["tools"]["owned_relative_gesture"],
            "nine_inventory":binding}
        script=self.prelude(cutoff,self.identity,snapshot=True)
        if creation is not None:
            data=canonical(creation)
            script+='test ! -e "$root/reconciled-creation.json"; test ! -L "$root/reconciled-creation.json"\n'
            script+=f'printf %s {q(base64.b64encode(data).decode())} | /usr/bin/openssl base64 -d -A > "$root/reconciled-creation.json"\n'
        script+='private "$root/reconciled-creation.json"\n'
        script+=f'test "$(sha256sum "$root/reconciled-creation.json" | cut -d " " -f 1)" = {q(self.reconciled_creation_sha256)}\n'
        geometry_name="gesture-paths-"+direction+".json"
        geometry_hash=self.profile["gesture_path_hashes"][geometry_name]
        script+=f'private "$root/{geometry_name}"\ntest "$(sha256sum "$root/{geometry_name}" | cut -d " " -f 1)" = {q(geometry_hash)}\n'
        for path,digest in [("/home/root/.config/remarkable/xochitl.conf",config["sidecar_sha256"][0]),
            (self.doc+".content",config["sidecar_sha256"][1]),(self.doc+".metadata",config["sidecar_sha256"][2])]:
            script+=f'test "$(sha256sum {q(path)} | cut -d " " -f 1)" = {q(digest)}\n'
        script+=f'gesture_root={q(gesture_root)}\ntest ! -e "$gesture_root"; test ! -L "$gesture_root"\nmkdir -m 700 "$gesture_root"\nprintf %s {q(nonce)} > "$gesture_root/owner"\n'
        script+=f'private "$root/{expected_prefix}.png"\ntest "$(stat -c \'%s\' "$root/{expected_prefix}.png")" -le 850000\n'
        script+=f'test "$(sha256sum "$root/{expected_prefix}.png" | cut -d " " -f 1)" = {q(config["source_image_sha256"])}\n'
        script+=f'cp "$root/{expected_prefix}.png" "$gesture_root/source.png"\nchmod 600 "$gesture_root/source.png"\nprivate "$gesture_root/source.png"\n'
        script+=f'test "$(sha256sum "$gesture_root/source.png" | cut -d " " -f 1)" = {q(config["source_image_sha256"])}\n'
        script+='read gesture_dev gesture_ino extra <<EOF\n$(stat -c "%d %i" "$gesture_root")\nEOF\ntest -z "$extra"\n'
        script+=f'map=$("$root/owned_relative_gesture" --nine-page-map-sha256 {q(self.doc+".content")} {q(target)})\n'
        script+=f'test "$map" = {q(config["page_map_sha256"])}\n'
        config_text=canonical(config).decode()
        # Two unsigned 64-bit root identities replace single-digit zeros;
        # sed may append one newline. Bound the final bytes BEFORE any output.
        if len(config_text.encode())+39>16384:
            raise Refused("SOURCE5 config exceeds its original reader cap before staging")
        script+='test "${#gesture_dev}" -le 20; test "${#gesture_ino}" -le 20\n'
        script+=f'printf %s {q(config_text)} | sed "s/\\\"root_dev\\\":0/\\\"root_dev\\\":$gesture_dev/;s/\\\"root_ino\\\":0/\\\"root_ino\\\":$gesture_ino/;s/{config["page_map_sha256"]}/$map/" > "$gesture_root/config.json"\n'
        owner_sha=hashlib.sha256(nonce.encode()).hexdigest()
        if not hasattr(self,"gesture_roots"): self.gesture_roots=[]
        if len(self.gesture_roots)>=2 or any(r["nonce"]==nonce for r in self.gesture_roots): raise Refused("At most two fresh registered roots")
        if not self.gesture_roots:
            script+='test ! -e "$root/gesture-roots.jsonl"; test ! -L "$root/gesture-roots.jsonl"\n(set -C; : > "$root/gesture-roots.jsonl")\nchmod 600 "$root/gesture-roots.jsonl"\n'
        script+='private "$root/gesture-roots.jsonl"\ntest "$(wc -l < "$root/gesture-roots.jsonl")" -lt 2\n'
        record_format=f'{{"nonce":"{nonce}","root":"{gesture_root}","dev":%s,"ino":%s,"owner_sha256":"{owner_sha}","phase":"{direction}","candidate_nonce":"{candidate_nonce}"}}'
        script+=f'record=$(printf {q(record_format)} "$gesture_dev" "$gesture_ino")\n'
        script+='test "$'+ '{#record}'+ '" -lt 512\nregistry_size=$(stat -c %s "$root/gesture-roots.jsonl")\n'
        script+='test $((registry_size+'+'$'+ '{#record}'+ '+1)) -le 1024\n'
        script+='printf \'%s\\n\' "$record" >> "$root/gesture-roots.jsonl"\n'
        script+='read observed_uptime observed_idle observed_extra < /proc/uptime\ntest -z "$observed_extra"\n'
        script+='printf \'{"registration":%s,"remote_uptime":"%s"}\\n\' "$(tail -n 1 "$root/gesture-roots.jsonl")" "$observed_uptime"\n'
        script+='chmod 600 "$gesture_root/owner" "$gesture_root/source.png" "$gesture_root/config.json" "$root/gesture-roots.jsonl"\n'
        self.call_deadline=phase.staging_end
        remaining=phase.staging_end-time.monotonic()
        if remaining<=0: raise Refused("Fresh staging budget exhausted before transport")
        stage_response=json.loads(self.call(script,remaining,record_role="staging"))
        stage_arrival=self.last_response_arrival
        registration=stage_response["registration"]
        remote_text=stage_response["remote_uptime"]
        if not re.fullmatch(r"[0-9]+\.[0-9]{1,3}",remote_text): raise Refused("Malformed lease clock sample")
        pending_deadline=stage_arrival+(overall_ms-int(float(remote_text)*1000))/1000
        if registration.get("nonce")!=nonce or registration.get("root")!=gesture_root or registration.get("owner_sha256")!=owner_sha or registration.get("candidate_nonce")!=candidate_nonce or registration.get("phase")!=direction or any(type(registration.get(k)) is not int or registration[k]<=0 for k in ("dev","ino")):
            raise Refused("Fresh root registration mismatch")
        self.gesture_roots.append(registration)
        self.retain_bounded("root-registration-"+direction,{"registration":registration,"remote_lower_ms":int(float(remote_text)*1000),
            "response_arrival_monotonic":stage_arrival,"go_cutoff_ms":go_cutoff,"overall_ceiling_ms":overall_ms,
            "pending_deadline_monotonic":pending_deadline,"staging_clock_lower_ms":lower_ms,
            "staging_clock_arrival_monotonic":arrival},phase.staging_end)
        phase.check(phase.staging_end)
        # The remote parent survives SSH-client cancellation; no extra supervisor.
        script=self.prelude(cutoff,self.identity,snapshot=True)
        script+=f'gesture_root={q(gesture_root)}\nprivate "$gesture_root/config.json"\n'
        script+='nohup "$root/owned_relative_gesture" --source5-capped "$gesture_root" < /dev/null > "$gesture_root/helper.stdout" 2> "$gesture_root/helper.stderr" &\nhelper_parent=$!\nwait "$helper_parent"\ncat "$gesture_root/gesture.result"\n'
        self.pending_helper_deadline=pending_deadline
        helper_end=min(phase.staging_end+5,phase.end,phase.result_deadline)
        self.call_deadline=helper_end
        text=self.call(script,helper_end-time.monotonic(),record_role="helper")
        if time.monotonic()>=helper_end: raise Refused("Helper response exhausted original allowance")
        report=json.loads(text)
        self.validate_navigation_release(report,nonce,binding,current)
        self.retain_bounded("gesture-"+direction,report,helper_end)
        self.pending_helper_deadline=None
        return report

    def validate_navigation_release(self,report,nonce,binding,current):
        if report.get("nonce")!=nonce or report.get("nine_inventory")!=binding or report.get("navigation_policy")!="stop-at-declared-neighbor-v1" or report.get("geometry_variant")!="fixed-gap10-v1":
            raise Refused("Released helper identity/policy mismatch")
        for field in ("writer_death_confirmed","owned_down_prefix","release_positive"):
            if report.get(field) is not True: raise Refused("Positive owned release missing")
        for field in ("unresolved","protocol_uncertain","ui_verified"):
            if report.get(field) is not False: raise Refused("Unknown helper release")
        status=report.get("status")
        if status=="released-gesture-ui-unverified":
            if report.get("trajectory_complete") is not True or type(report.get("complete_step")) is not int or report.get("complete_step")!=16 or report.get("target_observation") is not None or report.get("transition_unresolved") is not False:
                raise Refused("Contradictory full trajectory outcome")
        elif status=="released-after-target-observation-ui-unverified":
            step=report.get("complete_step")
            o=report.get("target_observation",{})
            if type(step) is not int or not 0<=step<=15 or report.get("trajectory_complete") is not False or report.get("transition_unresolved") is not True or o.get("step")!=step+1:
                raise Refused("Contradictory target terminal outcome")
            neighbor=5 if current==6 else 6
            samples=o.get("samples",{})
            pairs=[samples.get("first"),samples.get("second")]
            if type(samples.get("bracket_changed")) is not bool or any(type(pair) is not list or len(pair)!=2 or any(type(n) is not int or n not in {5,6} for n in pair) for pair in pairs):
                raise Refused("Invalid terminal samples")
            witness=[[n==neighbor for n in pair] for pair in pairs]
            actual_witness=o.get("witness_fields")
            if type(o.get("step")) is not int or type(actual_witness) is not list or len(actual_witness)!=2 or any(type(pair) is not list or len(pair)!=2 or any(type(v) is not bool for v in pair) for pair in actual_witness):
                raise Refused("Typed witness matrix required")
            if actual_witness!=witness or not any(any(pair) for pair in witness):
                raise Refused("No exact neighbor witness")
            if o.get("origin")=="parent":
                if o.get("phase") not in {"before-intent","after-intent"} or o.get("parent_waitstate")!="await-request" or o.get("worker_phase") is not None or o.get("worker_step") is not None: raise Refused("Parent terminal phase mismatch")
            elif o.get("origin")=="worker":
                if o.get("phase")!="context" or o.get("parent_waitstate")!="await-written" or o.get("worker_phase")!="context" or o.get("worker_step")!=o["step"]: raise Refused("Worker terminal phase mismatch")
            else: raise Refused("Unknown terminal origin")
        else: raise Refused("Helper failed; no corrective input")

    def await_pending_release(self):
        deadline=getattr(self,"pending_helper_deadline",None)
        if deadline is not None:
            while time.monotonic()<deadline:
                time.sleep(min(0.05,deadline-time.monotonic()))
            # Waiting protects the original authority only; it proves no cleanup.
            self.pending_helper_deadline=None

    def navigation_snapshot(self,stage,cutoff):
        if stage not in {"normalization-source","inserted-destination"}: raise Refused("Closed navigation capture required")
        return self.raw_snapshot(stage,cutoff,True)

    def reserve_postnative_control(self):
        if self.postnative_control: raise Refused("Postnative control capacity already reserved")
        roles=[("poll-clock",2048),("poll",16384),
            ("staging-clock",2048),("staging-clock",2048),("staging",8192),("staging",8192),
            ("registration",4096),("registration",4096),("helper",32768),("helper",32768),
            ("gesture",16384),("gesture",16384),("source5-report",8192),
            ("stop-clock",2048),("stop",8192)]
        try: tickets=self.control_ledger.reserve_records([size for role,size in roles])
        except ValueError as error: raise Refused(str(error)) from error
        for (role,size),ticket in zip(roles,tickets):
            self.postnative_control.setdefault(role,[]).append(ticket)

    def take_postnative_record(self,role):
        recovery=role in {"stop","stop-clock"} or (role=="clock" and self.stop_attempted)
        if self.control_ledger.overflow and not recovery:
            raise Refused("Control quota exhausted; no further qualification")
        if role.startswith("root-registration-"): role="registration"
        elif role.startswith("gesture-"): role="gesture"
        tickets=self.postnative_control.get(role)
        return tickets.pop(0) if tickets else None

    def reserve_navigation_frames(self):
        try: self.frame_ledger.reserve_creation()
        except (ValueError,AttributeError) as error: raise Refused(str(error)) from error
        cutoff=int(self.candidate_remote_at)+90
        timeout,deadline=self.action_budget(cutoff,3)
        script=self.prelude(cutoff,self.identity,snapshot=True)
        script+='test -z "$(find "$root" -type l)"\ncontrol_bytes=0\n'
        script+='test ! -e "$root/control-size-paths"; test ! -L "$root/control-size-paths"\n'
        script+='(set -C; (find "$root" -type f || printf \'\\nSOURCE5_CONTROL_PATH_ENUMERATION_FAILED\\n\') | dd bs=1 count=32769 2>/dev/null > "$root/control-size-paths")\n'
        script+='private "$root/control-size-paths"\ntest "$(stat -c %s "$root/control-size-paths")" -le 32768\n'
        script+='awk \'$0=="SOURCE5_CONTROL_PATH_ENUMERATION_FAILED" {bad=1} END {exit bad}\' "$root/control-size-paths"\n'
        patterns="|".join(q(self.root+"/"+prefix)+".*" for prefix in self.capture_prefixes)
        script+='while IFS= read -r path; do\n'
        if patterns: script+=f'case "$path" in {patterns}) continue;; esac\n'
        script+='size=$(stat -c "%s" "$path")\ncase "$size" in ""|*[!0-9]*) exit 90;; esac\ncontrol_bytes=$((control_bytes+size))\n'
        script+=f'test "$control_bytes" -le {CONTROL_BYTES-FUTURE_CONTROL_BYTES}\ndone < "$root/control-size-paths"\n'
        script+='printf \'{"control_bytes":%s}\\n\' "$control_bytes"\n'
        evidence=json.loads(self.call(script,timeout))
        self.action_guard(deadline)
        if type(evidence.get("control_bytes")) is not int or evidence["control_bytes"]>CONTROL_BYTES-FUTURE_CONTROL_BYTES:
            raise Refused("Generated control footprint cannot fit original reservation")
        self.retain("source5-control-reservation",evidence)

    def begin_navigation(self, both=False):
        deadline = self.known_deadline(int(self.candidate_remote_at)+115) if self.candidate_remote_at is not None else None
        if deadline is None: raise Refused("Original candidate clock missing")
        try: self.navigation_phase = PhaseBudget(deadline,both=both)
        except ValueError as error: raise Refused(str(error)) from error
        self.call_deadline=self.navigation_phase.staging_end
        return self.navigation_phase

    def stop_once(self, identity, cutoff):
        self.await_pending_release()
        self.navigation_phase=None
        if self.stop_attempted: return
        self.stop_attempted = True
        script = self.prelude(cutoff, identity) + r"""
test ! -e "$root/stop.request"; test ! -L "$root/stop.request"
(set -C; printf '%s\n' "$(cat "$root/owner")" > "$root/stop.request")
private "$root/stop.request"
"""
        timeout, deadline = self.action_budget(cutoff, 4) if self.is_v2() else (3, None)
        if self.frame_ledger is not None: self.call(script,timeout,record_role="stop")
        else: self.call(script,timeout)
        self.action_guard(deadline)

    def restore_probe(self):
        # Guardian input never writes the shared hardware-probe debug PNG.
        return
