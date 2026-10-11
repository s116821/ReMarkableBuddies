"""Pinned SSH adapter for the reviewed bounded development host sequence."""
import base64
import hashlib
import json
from pathlib import Path
import re
import shlex
import time
from open_document_host import Refused, same_pixels
from open_document_timing import upper_uptime_ms
from owned_frame_capture import CONTRACT as CAPTURE_CONTRACT, TOOLS as CAPTURE_TOOLS, capture_script, validate_receipt, hash_tool_checks


def q(value):
    return shlex.quote(str(value))


class SSHAdapter:
    def __init__(self, arguments, receipt, profile, output, transport, library_png):
        self.arguments, self.receipt, self.profile = arguments, receipt, profile
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
        self.tap_attempted = False
        self.arm_attempted = False
        self.stop_attempted = False
        self.tap_released = False
        self.tap_release_sha256 = None
        self.tap_supervisor_sha256 = None

    def retain(self, stage, evidence):
        def encode(value):
            if isinstance(value, bytes):
                return {"base64": base64.b64encode(value).decode("ascii"), "sha256": hashlib.sha256(value).hexdigest()}
            raise TypeError("Unsupported evidence type")
        (self.output / (stage + ".json")).write_text(json.dumps(evidence, default=encode, indent=2) + "\n")

    def call(self, script, timeout=4):
        result = self.transport(self.arguments, script, timeout)
        self.counter += 1
        (self.output / f"transport-{self.counter:03}.json").write_text(json.dumps(result, indent=2) + "\n")
        if result["timeout"] or result["exit"] != 0:
            raise Refused("Remote action failed or uncertain; never replay an action")
        return result["stdout"]

    def prelude(self, cutoff=None, identity=None):
        text = f"""set -eu
umask 077
root={q(self.root)}
doc={q(self.doc)}
private() {{
 test -f "$1" && test ! -L "$1"
 test "$(stat -c '%a %u %h' "$1")" = '600 0 1'
}}
test -d "$root" && test ! -L "$root"
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
test "$pid" -gt 1 && test "$start" -gt 0
test "$(sed 's/^.*) //' /proc/$pid/stat | awk '{{print $20}}')" = "$start"
test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"
for name in entry.closed restore.claim stop.request stop.admitted; do
 test ! -e "$root/$name" && test ! -L "$root/$name"
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
        for path, digest in self.profile.get("encoder_hashes", {}).items():
            if path not in {"/usr/bin/openssl", "/usr/lib/libssl.so.3", "/usr/lib/libcrypto.so.3"} or not re.fullmatch("[0-9a-f]{64}", digest):
                raise Refused("Exact encoder/provider path and digest required")
            text += f'test -f {q(path)} && test ! -L {q(path)}\ntest "$(stat -c \'%u %h\' {q(path)})" = \'0 1\'\n'
            text += f'test "$(sha256sum {q(path)} | cut -d " " -f 1)" = {q(digest)}\n'
        for name, digest in self.profile["tools"].items():
            if name not in ({"owned_tap_guardian", "observe_input", "publish_creation_arm"} | set(CAPTURE_TOOLS)) or not re.fullmatch("[0-9a-f]{64}", digest):
                raise Refused("Bounded exact tool identity required")
            text += f'test -f "$root/{name}" && test ! -L "$root/{name}"\ntest "$(stat -c \'%a %u %h\' "$root/{name}")" = \'700 0 1\'\n'
            text += f'test "$(sha256sum "$root/{name}" | cut -d " " -f 1)" = {q(digest)}\n'
        return text

    def raw_snapshot(self, stage, cutoff, idle):
        if self.capture_failed:
            raise Refused("Prior capture failed; no further acquisition")
        try:
            return self._raw_snapshot(stage, cutoff, idle)
        except Exception:
            self.capture_failed = True
            raise

    def snapshot_timeout(self, cutoff):
        if type(cutoff) is not int or cutoff <= 0:
            raise Refused("Fixed canonical snapshot cutoff required")
        anchor = time.monotonic()
        remote = upper_uptime_ms(self.call('set -eu\ncat /proc/uptime\n', 3))
        now = time.monotonic()
        elapsed = now - anchor
        if elapsed < 0:
            raise Refused("Monotonic clock regressed")
        remaining = cutoff - remote / 1000 - elapsed
        if remaining <= 0:
            raise Refused("Snapshot response fixed deadline exhausted")
        deadline = min(anchor + cutoff - remote / 1000,
                       now + CAPTURE_CONTRACT['maximum_response_seconds'])
        return deadline - now, deadline

    def _raw_snapshot(self, stage, cutoff, idle):
        if stage not in {"ready", "open", "trigger", "result"}:
            raise Refused("Unknown snapshot stage")
        script = self.prelude(cutoff, self.identity)
        prefix = "host-" + stage
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
        # Claim precedes every prefix output, including idle observations.
        script += f'(set -C; printf %s {q(self.profile["nonce"])} > "$root/{prefix}.claim")\n'
        # Every output is nonce-root-owned and exclusive. Snapshot failure cannot
        # renew a stage or lead to a tap/arm retry.
        script += f'test ! -e "$root/{prefix}.png" && test ! -L "$root/{prefix}.png"\n'
        if idle:
            script += f'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --observe "$root/observe_input" 2 > "$root/{prefix}.idle" 2>&1\n'
            script += '/usr/bin/evtest --query /dev/input/event1 EV_KEY BTN_TOUCH\n/usr/bin/evtest --query /dev/input/event1 EV_KEY BTN_TOOL_PEN\n'
        else:
            script += f': > "$root/{prefix}.idle"\n'
        script += capture_script(prefix, self.profile["nonce"], self.profile["boot_id"])
        for ext in ("content", "metadata"):
            script += f'before=$(sha256sum "$doc.{ext}" | cut -d " " -f 1)\ncp "$doc.{ext}" "$root/{prefix}.{ext}"\n'
            script += f'test "$(sha256sum "$root/{prefix}.{ext}" | cut -d " " -f 1)" = "$before"\ntest "$(sha256sum "$doc.{ext}" | cut -d " " -f 1)" = "$before"\n'
        paths = " ".join(q(p) for p in self.profile["immutable_hashes"])
        script += f'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --hashes /usr/bin/sha256sum {paths} > "$root/{prefix}.hashes"\n'
        script += 'test "$(sed \'s/^.*) //\' /proc/$pid/stat | awk \'{print $20}\')" = "$start"\n'
        script += 'test "$(now_upper)" -lt "$cutoff"\n'
        script += 'test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"\n'
        script += 'for name in entry.closed restore.claim stop.request stop.admitted; do test ! -e "$root/$name" && test ! -L "$root/$name"; done\n'
        script += 'test "$(stat -c \'%a %u\' "$root")" = \'700 0\'\nprivate "$root/owner"\n'
        script += f'test "$(cat "$root/owner")" = {q(self.profile["nonce"])}\n'
        if self.root_inode:
            script += f'test "$(stat -c \'%d %i\' "$root")" = {q(self.root_inode)}\n'
        script += r"""printf '{"pid":%s,"start_ticks":%s,"root_inode":"%s","window":"%s","waiting":"%s","capture_uptime":"%s","last_open":"%s",' "$pid" "$start" "$(stat -c '%d %i' "$root")" "$(b64 "$root/candidate.window")" "$(b64 "$root/creation-waiting")" "$(b64 /proc/uptime)" "$(grep '^LastOpen' /home/root/.config/remarkable/xochitl.conf | /usr/bin/openssl base64 -A)"
"""
        script += f'printf \'"frame_hash":"%s","native_hash":"%s","capture_receipt":"%s",\' "$frame_hash" "$native_hash" "$(b64 "$root/{prefix}.capture")"\n'
        for key in ("content", "metadata", "png", "hashes", "idle"):
            script += f'printf \'"{key}":"%s",\' "$(b64 "$root/{prefix}.{key}")"\n'
        script += r"""if test -f "$root/diagnostics.json" && test ! -L "$root/diagnostics.json"; then
 private "$root/diagnostics.json"
 printf '"diagnostics":"%s",' "$(b64 "$root/diagnostics.json")"
else printf '"diagnostics":"",'; fi
test "$(stat -c '%d %i' "$root")" = "$initial_root_identity"
test "$(sed 's/^.*) //' /proc/$pid/stat | awk '{print $20}')" = "$start"
test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"
for name in entry.closed restore.claim stop.request stop.admitted; do test ! -e "$root/$name" && test ! -L "$root/$name"; done
test "$(now_upper)" -lt "$cutoff"
printf '"uptime":"%s"}\n' "$(b64 /proc/uptime)"
"""
        timeout, response_deadline = self.snapshot_timeout(cutoff)
        response = self.call(script, timeout)
        if time.monotonic() >= response_deadline:
            raise Refused("Complete snapshot response arrived after fixed deadline")
        raw = json.loads(response)
        decode = lambda key: base64.b64decode(raw[key], validate=True)
        try:
            capture_evidence = validate_receipt(decode("capture_receipt").decode("ascii"), self.profile["nonce"], raw["frame_hash"], decode("png"), raw["native_hash"])
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
        last = decode("last_open").decode("utf-8").strip()
        match = re.fullmatch(r"LastOpen=@ByteArray\(([0-9a-f-]{36})\)", last)
        observed = {"window": [window[0]] + [int(v) for v in window[1:]], "boot_id": self.profile["boot_id"],
                    "response_deadline_monotonic": response_deadline,
                    "pid": raw["pid"], "start_ticks": raw["start_ticks"], "closed": False,
                    "waiting": decode("waiting"), "uptime": decode("uptime").decode("ascii"),
                    "last_open": match[1] if match else None, "content": json.loads(decode("content")),
                    "metadata": json.loads(decode("metadata")), "png": decode("png"), "hashes": hashes,
                    "content_raw_sha256": hashlib.sha256(decode("content")).hexdigest(),
                    "metadata_raw_sha256": hashlib.sha256(decode("metadata")).hexdigest(),
                    "idle": {"interval_ms": 2000 if idle else None, "invalidated": not idle_ok, "touch_active": not idle_ok, "pen_contact": 0 if idle else None, "pen_tool": 0 if idle else None},
                    "capture": capture_evidence, "diagnostics": json.loads(decode("diagnostics")) if raw["diagnostics"] else {}}
        if self.root_inode is not None and self.root_inode != raw["root_inode"]:
            raise Refused("Private root inode changed")
        if time.monotonic() >= response_deadline:
            raise Refused("Snapshot decoding exhausted fixed response deadline")
        self.root_inode = raw["root_inode"]
        return observed

    def wait_library(self):
        # This loop only waits for existing actor/SDK markers. No capture/input
        # retry and no deadline renewal. Unknown timing never permits mutation.
        end = time.monotonic() + 110
        while time.monotonic() < end:
            script = f"""set -eu
root={q(self.root)}
test ! -e "$root/restore.claim" && test ! -e "$root/stock.restored" && test ! -e "$root/recovery.failed"
if test -f "$root/candidate.window" && test -f "$root/creation-waiting" && test -f "$root/attempt.identity"; then cat "$root/candidate.window"; fi
"""
            line = self.call(script, 3).strip()
            if line:
                values = line.split()
                if len(values) != 5 or values[0] != self.profile["nonce"]:
                    raise Refused("Waiting actor window invalid")
                cutoff = int(values[3]) + 20
                raw = self.raw_snapshot("ready", cutoff, True)
                self.identity = (raw["boot_id"], raw["pid"], raw["start_ticks"])
                return raw
            time.sleep(0.2)
        raise Refused("Candidate waiting markers not observed in fixed setup allowance")

    def open_once(self, identity, cutoff):
        if self.tap_attempted: raise Refused("Ordinary open already attempted")
        self.tap_attempted = True
        script = self.prelude(cutoff, identity)
        script += 'test $((cutoff-$(now_upper))) -ge 4\n'
        # Detached sole writer survives SSH loss; never wrap it in bounded().
        script += 'test ! -e "$root/tap-start.claim" && test ! -L "$root/tap-start.claim"\n'
        script += f'(set -C; printf \'%s\\n\' {q(self.profile["nonce"])} > "$root/tap-start.claim")\n'
        # Select a setup cutoff once from fresh remote BOOTTIME, capped by the
        # original open stage. Ceil+14 seconds is at most 15 seconds from now.
        script += f'tap_cutoff_ms=$(( $(now_upper)*1000 + 14000 ))\ntest "$tap_cutoff_ms" -le {int(cutoff) * 1000} || tap_cutoff_ms={int(cutoff) * 1000}\n'
        script += f'nohup "$root/owned_tap_guardian" --tap {q(self.profile["nonce"])} {q(self.profile["boot_id"])} "$tap_cutoff_ms" < /dev/null > "$root/tap-guardian.log" 2>&1 &\n'
        script += r"""while ! test -f "$root/tap.ready"; do
 test ! -e "$root/tap.unresolved" && test "$(now_upper)" -lt "$cutoff"
 sleep 0.01
done
private "$root/tap.ready"
private "$root/tap.setup"
printf '{"ready":"%s","setup":"%s","selected_cutoff_ms":%s,"uptime":"%s"}\n' "$(b64 "$root/tap.ready")" "$(b64 "$root/tap.setup")" "$tap_cutoff_ms" "$(b64 /proc/uptime)"
"""
        envelope = json.loads(self.call(script, 3))
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
        # Publish exactly once, only after complete ready/lease evidence is saved.
        script = self.prelude(cutoff, identity)
        script += f'test "$(sha256sum "$root/tap.ready" | cut -d " " -f 1)" = {q(hashlib.sha256(ready_bytes).hexdigest())}\n'
        script += f'test "$(sed \'s/^.*) //\' /proc/{ready["pid"]}/stat | awk \'{{print $20}}\')" = {q(ready["start_ticks"])}\n'
        script += 'test ! -e "$root/tap.go" && test ! -L "$root/tap.go"\n'
        script += f'(set -C; "$root/owned_tap_guardian" --publish-go {q(self.profile["nonce"])} {q(self.profile["boot_id"])} > "$root/tap.go-publication.json" 2> "$root/tap.go-publication.log")\n'
        script += r"""while ! test -f "$root/tap.released" || ! test -f "$root/tap.supervisor-complete"; do
 for name in tap.unresolved tap.empty tap.cleaned tap.recovered tap.supervisor-spent; do test ! -e "$root/$name" && test ! -L "$root/$name"; done
 test "$(now_upper)" -lt "$cutoff"
 sleep 0.01
done
private "$root/tap.released"
private "$root/tap.supervisor-complete"
private "$root/tap.lease"
private "$root/tap.go-publication.json"
printf '{"released":"%s","supervisor":"%s","lease":"%s","publication":"%s"}\n' "$(b64 "$root/tap.released")" "$(b64 "$root/tap.supervisor-complete")" "$(b64 "$root/tap.lease")" "$(b64 "$root/tap.go-publication.json")"
"""
        completed = json.loads(self.call(script, 3))
        publication_bytes = base64.b64decode(completed["publication"], validate=True)
        self.retain("tap-go-publication", {"receipt": publication_bytes})
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
        if value["pid"] <= 1 or value["tracking_id"] > 65535 or type(value["slot"]) is not int or value["slot"] != 0 or value["native_point"] != [201, 1360] or value["lease_end_ms"] > int(cutoff) * 1000 or value["status"] != status or value["exclusive_kernel_ownership"] is not False:
            raise Refused("Guardian release/point/deadline not admitted")
        return value

    def snapshot(self, stage, cutoff, idle):
        return self.raw_snapshot(stage, cutoff, idle)

    def arm_once(self, waiting, timing, ready):
        if self.arm_attempted: raise Refused("Creation publication already attempted")
        if not self.tap_released: raise Refused("Positive guardian release required before creation")
        self.arm_attempted = True
        script = self.prelude(timing.stage_end("trigger"), self.identity)
        # Recheck the exact actor partitions after the host observation and
        # immediately before publishing permission. Observation is not a lock.
        for name in ("runtime.files", "document-immutable.files"):
            script += f'private "$root/{name}"\n'
            script += f'test "$(sha256sum "$root/{name}" | cut -d " " -f 1)" = {q(self.receipt["files"][name])}\n'
            script += f'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --check /usr/bin/sha256sum -c "$root/{name}" > "$root/host-prepublication-{name}.check" 2>&1\n'
        for ext in ("content", "metadata"):
            # Compare exact captured remote bytes, not reserialized JSON. The
            # previously retained stage file is tied to this host observation.
            digest = ready.get(ext + "_raw_sha256")
            if not digest: raise Refused("Exact sidecar-byte binding missing")
            script += f'test "$(sha256sum "$doc.{ext}" | cut -d " " -f 1)" = {q(digest)}\n'
            script += f'test "$(sha256sum "$root/host-trigger.{ext}" | cut -d " " -f 1)" = {q(digest)}\n'
            script += f'test ! -e "$root/host-source.{ext}" && test ! -L "$root/host-source.{ext}"\ncp "$root/host-trigger.{ext}" "$root/host-source.{ext}"\nchmod 600 "$root/host-source.{ext}"\n'
        if not self.tap_release_sha256 or not self.tap_supervisor_sha256:
            raise Refused("Exact retained guardian release binding missing")
        script += 'private "$root/tap.released"\n'
        script += f'test "$(sha256sum "$root/tap.released" | cut -d " " -f 1)" = {q(self.tap_release_sha256)}\n'
        script += 'private "$root/tap.supervisor-complete"\n'
        script += f'test "$(sha256sum "$root/tap.supervisor-complete" | cut -d " " -f 1)" = {q(self.tap_supervisor_sha256)}\n'
        script += 'for name in tap.unresolved tap.empty tap.cleaned tap.recovered tap.supervisor-spent; do test ! -e "$root/$name" && test ! -L "$root/$name"; done\n'
        args = [waiting.nonce, waiting.pid, waiting.start_ticks, waiting.expiry_ms, timing.armed_at, timing.deadline, timing.candidate_at, self.profile["document_id"]]
        script += 'test "$(now_upper)" -le $((cutoff-4))\n"$root/owned_frame_alarm" --publish "$root/publish_creation_arm" ' + " ".join(q(v) for v in args) + ' > "$root/host-publication.json" 2> "$root/host-publication.stderr"\ncat "$root/host-publication.json"\n'
        result = json.loads(self.call(script, 4))
        if result.get("published") is not True or result.get("retry_permitted") is not False:
            raise Refused("Publication uncertain; never replay")

    def result_snapshot(self, identity, cutoff):
        # SDK access window is at most five seconds. Poll diagnostics only, then
        # collect exactly one same-candidate frame/files snapshot before stopping.
        for _ in range(30):
            script = self.prelude(cutoff, identity) + r"""
if test -f "$root/diagnostics.json" && test ! -L "$root/diagnostics.json"; then cat "$root/diagnostics.json"; fi
"""
            text = self.call(script, 3).strip()
            if text:
                # SDK creates the receipt before its single bounded write.
                # An incomplete read is observation-not-ready, never a reason
                # to replay input/publication or renew this fixed result cap.
                if len(text.encode("utf-8")) > 8192:
                    raise Refused("Native diagnostics exceed reviewed bound")
                try:
                    diagnostics = json.loads(text)
                except json.JSONDecodeError:
                    time.sleep(0.1)
                    continue
                if not isinstance(diagnostics, dict):
                    raise Refused("Native diagnostics object required")
                trial = diagnostics.get("creation_trial", {})
                if diagnostics.get("terminal_stage") or trial.get("returned") or trial.get("exception"):
                    return self.raw_snapshot("result", cutoff, False)
            time.sleep(0.1)
        raise Refused("Native result not observed before fixed result cap")

    def stop_once(self, identity, cutoff):
        if self.stop_attempted: return
        self.stop_attempted = True
        script = self.prelude(cutoff, identity) + r"""
test ! -e "$root/stop.request" && test ! -L "$root/stop.request"
(set -C; printf '%s\n' "$(cat "$root/owner")" > "$root/stop.request")
private "$root/stop.request"
"""
        self.call(script, 3)

    def restore_probe(self):
        # Guardian input never writes the shared hardware-probe debug PNG.
        return
