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
        self.tap_attempted = False
        self.arm_attempted = False
        self.stop_attempted = False
        self.original_probe = None

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
bounded() {
 "$@" & child=$!
 while kill -0 "$child" 2>/dev/null; do
  if test "$(now_upper)" -ge "$cutoff"; then
   kill -TERM "$child" 2>/dev/null || :
   kill -KILL "$child" 2>/dev/null || :
   wait "$child" 2>/dev/null || :
   return 90
  fi
  sleep 0.1
 done
 wait "$child"
 test "$(now_upper)" -lt "$cutoff"
}
b64() { base64 "$1" | tr -d '\n'; }
"""
        for name, digest in self.profile["tools"].items():
            if name not in {"hardware_probe", "observe_input", "publish_creation_arm"} or not re.fullmatch("[0-9a-f]{64}", digest):
                raise Refused("Bounded exact tool identity required")
            text += f'test -f "$root/{name}" && test ! -L "$root/{name}"\ntest "$(stat -c \'%a %u %h\' "$root/{name}")" = \'700 0 1\'\n'
            text += f'test "$(sha256sum "$root/{name}" | cut -d " " -f 1)" = {q(digest)}\n'
        return text

    def raw_snapshot(self, stage, cutoff, idle):
        if stage not in {"ready", "open", "trigger", "result"}:
            raise Refused("Unknown snapshot stage")
        script = self.prelude(cutoff, self.identity)
        prefix = "host-" + stage
        # Every output is nonce-root-owned and exclusive. Snapshot failure cannot
        # renew a stage or lead to a tap/arm retry.
        script += f'test ! -e "$root/{prefix}.png" && test ! -L "$root/{prefix}.png"\n'
        script += f'test "$(sha256sum {q(self.profile["screenshot_path"])} | cut -d " " -f 1)" = {q(self.profile["screenshot_sha256"])}\n'
        if idle:
            script += f'bounded "$root/observe_input" 2 > "$root/{prefix}.idle" 2>&1\n'
            script += '/usr/bin/evtest --query /dev/input/event1 EV_KEY BTN_TOUCH\n/usr/bin/evtest --query /dev/input/event1 EV_KEY BTN_TOOL_PEN\n'
        else:
            script += f': > "$root/{prefix}.idle"\n'
        script += f'bounded {q(self.profile["screenshot_path"])} "$root/{prefix}.png" > "$root/{prefix}.capture" 2>&1\n'
        for ext in ("content", "metadata"):
            script += f'before=$(sha256sum "$doc.{ext}" | cut -d " " -f 1)\ncp "$doc.{ext}" "$root/{prefix}.{ext}"\n'
            script += f'test "$(sha256sum "$root/{prefix}.{ext}" | cut -d " " -f 1)" = "$before"\ntest "$(sha256sum "$doc.{ext}" | cut -d " " -f 1)" = "$before"\n'
        paths = " ".join(q(p) for p in self.profile["immutable_hashes"])
        script += f'bounded sha256sum {paths} > "$root/{prefix}.hashes"\n'
        script += 'test "$(sed \'s/^.*) //\' /proc/$pid/stat | awk \'{print $20}\')" = "$start"\n'
        script += 'test "$(now_upper)" -lt "$cutoff"\n'
        script += 'test "$(systemctl show xochitl.service -p MainPID --value)" = "$pid"\n'
        script += 'for name in entry.closed restore.claim stop.request stop.admitted; do test ! -e "$root/$name" && test ! -L "$root/$name"; done\n'
        script += 'test "$(stat -c \'%a %u\' "$root")" = \'700 0\'\nprivate "$root/owner"\n'
        script += f'test "$(cat "$root/owner")" = {q(self.profile["nonce"])}\n'
        if self.root_inode:
            script += f'test "$(stat -c \'%d %i\' "$root")" = {q(self.root_inode)}\n'
        script += r"""printf '{"pid":%s,"start_ticks":%s,"root_inode":"%s","window":"%s","waiting":"%s","uptime":"%s","last_open":"%s",' "$pid" "$start" "$(stat -c '%d %i' "$root")" "$(b64 "$root/candidate.window")" "$(b64 "$root/creation-waiting")" "$(b64 /proc/uptime)" "$(grep '^LastOpen' /home/root/.config/remarkable/xochitl.conf | base64 | tr -d '\n')"
"""
        for key in ("content", "metadata", "png", "hashes", "idle"):
            script += f'printf \'"{key}":"%s",\' "$(b64 "$root/{prefix}.{key}")"\n'
        script += r"""if test -f "$root/diagnostics.json" && test ! -L "$root/diagnostics.json"; then
 private "$root/diagnostics.json"
 printf '"diagnostics":"%s"}\n' "$(b64 "$root/diagnostics.json")"
else printf '"diagnostics":""}\n'; fi
"""
        raw = json.loads(self.call(script, 10))
        decode = lambda key: base64.b64decode(raw[key], validate=True)
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
                    "pid": raw["pid"], "start_ticks": raw["start_ticks"], "closed": False,
                    "waiting": decode("waiting"), "uptime": decode("uptime").decode("ascii"),
                    "last_open": match[1] if match else None, "content": json.loads(decode("content")),
                    "metadata": json.loads(decode("metadata")), "png": decode("png"), "hashes": hashes,
                    "content_raw_sha256": hashlib.sha256(decode("content")).hexdigest(),
                    "metadata_raw_sha256": hashlib.sha256(decode("metadata")).hexdigest(),
                    "idle": {"interval_ms": 2000 if idle else None, "invalidated": not idle_ok, "touch_active": not idle_ok, "pen_contact": 0 if idle else None, "pen_tool": 0 if idle else None},
                    "diagnostics": json.loads(decode("diagnostics")) if raw["diagnostics"] else {}}
        if self.root_inode is not None and self.root_inode != raw["root_inode"]:
            raise Refused("Private root inode changed")
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
        # Preserve the preexisting helper debug PNG, including absence state.
        script += r"""test ! -e "$root/host-original-probe.claim"
(set -C; : > "$root/host-original-probe.claim")
if test -e /tmp/reader-buddy-probe.png || test -L /tmp/reader-buddy-probe.png; then
 test -f /tmp/reader-buddy-probe.png && test ! -L /tmp/reader-buddy-probe.png
 test "$(stat -c '%u %h' /tmp/reader-buddy-probe.png)" = '0 1'
 cp -p /tmp/reader-buddy-probe.png "$root/host-original-probe.png"
 printf present > "$root/host-original-probe.state"
else printf absent > "$root/host-original-probe.state"; fi
bounded "$root/hardware_probe" tap 110 280 > "$root/host-open.log" 2>&1
"""
        self.call(script, 6)

    def snapshot(self, stage, cutoff, idle):
        return self.raw_snapshot(stage, cutoff, idle)

    def arm_once(self, waiting, timing, ready):
        if self.arm_attempted: raise Refused("Creation publication already attempted")
        self.arm_attempted = True
        script = self.prelude(timing.stage_end("trigger"), self.identity)
        # Recheck the exact actor partitions after the host observation and
        # immediately before publishing permission. Observation is not a lock.
        for name in ("runtime.files", "document-immutable.files"):
            script += f'private "$root/{name}"\n'
            script += f'test "$(sha256sum "$root/{name}" | cut -d " " -f 1)" = {q(self.receipt["files"][name])}\n'
            script += f'bounded sha256sum -c "$root/{name}" > "$root/host-prepublication-{name}.check" 2>&1\n'
        for ext in ("content", "metadata"):
            # Compare exact captured remote bytes, not reserialized JSON. The
            # previously retained stage file is tied to this host observation.
            digest = ready.get(ext + "_raw_sha256")
            if not digest: raise Refused("Exact sidecar-byte binding missing")
            script += f'test "$(sha256sum "$doc.{ext}" | cut -d " " -f 1)" = {q(digest)}\n'
            script += f'test "$(sha256sum "$root/host-trigger.{ext}" | cut -d " " -f 1)" = {q(digest)}\n'
            script += f'test ! -e "$root/host-source.{ext}" && test ! -L "$root/host-source.{ext}"\ncp "$root/host-trigger.{ext}" "$root/host-source.{ext}"\nchmod 600 "$root/host-source.{ext}"\n'
        args = [waiting.nonce, waiting.pid, waiting.start_ticks, waiting.expiry_ms, timing.armed_at, timing.deadline, timing.candidate_at, self.profile["document_id"]]
        script += 'bounded "$root/publish_creation_arm" ' + " ".join(q(v) for v in args) + ' > "$root/host-publication.json" 2> "$root/host-publication.stderr"\ncat "$root/host-publication.json"\n'
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
        if not self.tap_attempted:
            return
        # This cleanup does not publish permissions, control services or retry
        # input. It only restores the fixed debug path explicitly saved by us.
        script = f"""set -eu
root={q(self.root)}
test -d "$root" && test ! -L "$root"
test "$(stat -c '%a %u' "$root")" = '700 0'
test "$(stat -c '%d %i' "$root")" = {q(self.root_inode)}
test -f "$root/owner" && test ! -L "$root/owner"
test "$(stat -c '%a %u %h' "$root/owner")" = '600 0 1'
test "$(cat "$root/owner")" = {q(self.profile["nonce"])}
test -f "$root/host-original-probe.claim" && test ! -L "$root/host-original-probe.claim"
test "$(stat -c '%a %u %h' "$root/host-original-probe.claim")" = '600 0 1'
test -f "$root/host-original-probe.state" && test ! -L "$root/host-original-probe.state"
test "$(stat -c '%a %u %h' "$root/host-original-probe.state")" = '600 0 1'
test ! -L /tmp/reader-buddy-probe.png
if test -e /tmp/reader-buddy-probe.png; then
 test -f /tmp/reader-buddy-probe.png
 test "$(stat -c '%u %h' /tmp/reader-buddy-probe.png)" = '0 1'
fi
case "$(cat "$root/host-original-probe.state")" in
 present) test -f "$root/host-original-probe.png" && test ! -L "$root/host-original-probe.png"
  test "$(stat -c '%u %h' "$root/host-original-probe.png")" = '0 1'
  cp -p "$root/host-original-probe.png" /tmp/reader-buddy-probe.png
  test "$(sha256sum /tmp/reader-buddy-probe.png | cut -d ' ' -f 1)" = "$(sha256sum "$root/host-original-probe.png" | cut -d ' ' -f 1)" ;;
 absent) test ! -L /tmp/reader-buddy-probe.png; rm -f /tmp/reader-buddy-probe.png ;;
 *) exit 90 ;;
esac
"""
        self.call(script, 3)
