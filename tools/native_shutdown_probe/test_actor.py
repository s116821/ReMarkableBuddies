"""Focused exact-source shell fixtures. Owned files/processes; OS/service reads are injected.

No service manager, SSH, payload load, tablet or native result is exercised.
"""
import os
import importlib.util
import json
import hashlib
import contextlib
import io
from unittest import mock
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
SOURCE = (HERE / "actor.sh.in").read_text().split("# Main-only execution boundary", 1)[0]


def render_fixture(root, text):
    text = text.replace("@ROOT@", str(root)).replace("@NONCE@", "1" * 32)
    text = text.replace("@STOCKPID@", "42").replace("@STOCKSTART@", "561")
    text = text.replace("/sys/fs/cgroup/systemd/system.slice/xochitl.service/cgroup.procs", str(root / "cgroup.procs"))
    text = text.replace("parent=/run/systemd/system/xochitl.service.d", 'parent="$root/dropins"')
    text = text.replace("unit_shadow=/run/systemd/system/xochitl.service", 'unit_shadow="$root/unit-shadow"')
    return text


def shell(root, body, overrides="", expected=0):
    text = render_fixture(root, SOURCE)
    result = subprocess.run(["/bin/sh", "-c", text + "\n" + overrides + "\n" + body],
                            capture_output=True, text=True, timeout=8)
    if result.returncode != expected:
        raise AssertionError(f"exit {result.returncode}, expected {expected}: {result.stderr}")
    return result


def fixture_guard_shadows(root, parent):
    (root / "guard.directory-created").write_text("1" * 32 + "\n")
    (root / "guard.directory-created").chmod(0o600)
    for source, destination, data, intent in (
            ("unit-shadow.service", root / "unit-shadow", "unit", "unit-shadow.install-intent"),
            ("vendor-shadow.conf", parent / "xochitl-service-override.conf", "vendor", "vendor-shadow.install-intent")):
        (root / source).write_text(data)
        destination.write_text(data)
        (root / source).chmod(0o600)
        destination.chmod(0o600)
        (root / intent).write_text("1" * 32 + "\n")
        (root / intent).chmod(0o600)
    (root / "guard-file.install-intent").write_text("1" * 32 + "\n")
    (root / "guard-file.install-intent").chmod(0o600)


class Actor(unittest.TestCase):
    def test_partial_shadow_installation_cleanup_and_foreign_refusal(self):
        for stage in range(5):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                parent = root / "dropins"
                if stage >= 1:
                    parent.mkdir(mode=0o700)
                    (root / "guard.directory-created").write_text("1" * 32 + "\n")
                    (root / "guard.directory-created").chmod(0o600)
                for source, destination, data, intent, threshold in (
                        ("unit-shadow.service", root / "unit-shadow", "unit", "unit-shadow.install-intent", 2),
                        ("vendor-shadow.conf", parent / "xochitl-service-override.conf", "vendor", "vendor-shadow.install-intent", 3),
                        ("guard.conf", parent / "guard", "guard", "guard-file.install-intent", 4)):
                    (root / source).write_text(data)
                    (root / source).chmod(0o600)
                    # Persisted intent with absent file simulates interruption before creation.
                    if stage >= threshold - 1:
                        (root / intent).write_text("1" * 32 + "\n")
                        (root / intent).chmod(0o600)
                    if stage >= threshold:
                        destination.write_text(data)
                        destination.chmod(0o600)
                overrides = '''
guard="$parent/guard"
reserve() { return 0; }
query() { printf '%s\n' "$*" >> "$root/events"; }
policy() { :; }
value() { VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'; }
hashes() { :; }
verify_stock() { test ! -e "$unit_shadow"; test ! -e "$parent"; printf healthy > "$root/stock.restored"; }
'''
                shell(root, 'guard_installed=yes; restore', overrides)
                self.assertTrue((root / "stock.restored").exists())
                self.assertNotIn("start", (root / "events").read_text())
                self.assertNotIn("stop", (root / "events").read_text())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            parent = root / "dropins"
            parent.mkdir(mode=0o700)
            fixture_guard_shadows(root, parent)
            (root / "guard.conf").write_text("guard")
            (parent / "guard").write_text("guard")
            (parent / "guard").chmod(0o600)
            (parent / "xochitl-service-override.conf").write_text("foreign")
            shell(root, 'guard="$parent/guard"; guard_installed=yes; restore', 'reserve() { return 0; }', expected=90)
            self.assertTrue((parent / "guard").exists())
            self.assertTrue((root / "unit-shadow").exists())
            self.assertFalse((root / "stock-start.claim").exists())

    def test_guard_policy_requires_exact_fragment_dropins_and_owned_files(self):
        for mode in ("valid", "wrong-fragment", "vendor-leak", "missing-unit", "wrong-exec"):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                parent = root / "dropins"
                parent.mkdir(mode=0o700)
                fixture_guard_shadows(root, parent)
                (root / "guard.conf").write_text("guard")
                (parent / "guard").write_text("guard")
                (parent / "guard").chmod(0o600)
                (root / "guard.policy").write_text("OnFailure=\nRestart=no\n")
                overrides = '''
guard="$parent/guard"
query() {
 fragment=$unit_shadow; dropins="$vendor_shadow $guard"; execution='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'
 if test "$mode" = wrong-fragment; then fragment=/usr/lib/systemd/system/xochitl.service; fi
 if test "$mode" = vendor-leak; then dropins="/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf $guard"; fi
 if test "$mode" = wrong-exec; then execution='path=/bin/false ; argv[]=/bin/false ;'; fi
 printf 'OnFailure=\nRestart=no\nFragmentPath=%s\nDropInPaths=%s\nExecStart={ %s }\n' "$fragment" "$dropins" "$execution" > "$root/query.stdout"
}
'''
                body = f'''mode={mode}
if test "$mode" = missing-unit; then rm "$unit_shadow"; fi
if policy "$root/guard.policy"; then test "$mode" = valid; else test "$mode" != valid; fi
test ! -e "$root/stock-stop.claim"
'''
                shell(root, body, overrides)

    def test_shadow_derivation_removes_only_exact_scoped_dependency(self):
        spec = importlib.util.spec_from_file_location("prepare", HERE / "prepare_packet.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        original = b"[Unit]\nOnFailure=remarkable-fail.service\nOnFailureJobMode=replace\nDefaultDependencies=no\n[Service]\nRestartMode=direct\nWatchdogSec=60\n"
        self.assertEqual(module.remove_owned_onfailure(original), original.replace(b"OnFailure=remarkable-fail.service\n", b""))
        for invalid in (original.replace(b"[Unit]", b"[Service]"), original.replace(b"remarkable-fail", b"foreign"),
                        original.replace(b"OnFailure=remarkable-fail.service\n", b""),
                        original.replace(b"[Service]", b"OnFailure=remarkable-fail.service\n[Service]")):
            with self.assertRaises(ValueError): module.remove_owned_onfailure(invalid)

    def test_failed_guard_policy_snapshot_survives_stock_recovery_policy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "guard.policy").write_text("OnFailure=\nRestart=no\n")
            (root / "stock.policy").write_text("OnFailure=remarkable-fail.service\nRestart=on-failure\n")
            overrides = '''
query() {
 if test "$mode" = guard; then printf 'OnFailure=remarkable-fail.service\nRestart=no\n';
 else printf 'OnFailure=remarkable-fail.service\nRestart=on-failure\nFragmentPath=/usr/lib/systemd/system/xochitl.service\nDropInPaths=/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf\n'; fi > "$root/query.stdout"
}
'''
            shell(root, '''
mode=guard
if policy "$root/guard.policy"; then exit 99; fi
mode=stock
policy "$root/stock.policy"
''', overrides)
            self.assertEqual((root / "guard.policy.observed").read_text(), "OnFailure=remarkable-fail.service\nRestart=no\n")
            self.assertIn("OnFailure=remarkable-fail.service\nRestart=on-failure\n", (root / "stock.policy.observed").read_text())

    def test_one_active_two_failed_cannot_verify_stock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "clock").write_text("100\n")
            overrides = '''
now() { cat "$root/clock"; }
sleep() { tick=$(cat "$root/clock"); printf '%s\n' "$((tick+40))" > "$root/clock"; }
query() { test "$*" = 'is-active --quiet xochitl.service'; }
value() { VALUE=''; }
'''
            shell(root, 'deadline=460; verify_stock', overrides, expected=1)
            self.assertFalse((root / "stock.restored").exists())

    def test_main_host_loss_before_activation_restores_guard_independently(self):
        for request_guard in (False, True):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                root.chmod(0o700)
                (root / "clock").write_text("100\n")
                (root / "cgroup.procs").write_text("42\n")
                (root / "guard.conf").write_text("guard")
                (root / "guard.conf").chmod(0o600)
                for name, data in (("unit-shadow.service", "unit"), ("vendor-shadow.conf", "vendor")):
                    (root / name).write_text(data)
                    (root / name).chmod(0o600)
                if request_guard:
                    (root / "guard.request").write_text("1" * 32)
                    (root / "guard.request").chmod(0o600)
                overrides = '''
parent="$root/dropins"; guard="$parent/guard"; activation="$parent/activation"
now() { cat "$root/clock"; }
sleep() { tick=$(cat "$root/clock"); printf '%s\n' "$((tick+1))" > "$root/clock"; }
validate_root() { deadline=460; }
hashes() { :; }
policy() { printf 'policy %s\n' "$1" >> "$root/events"; }
same_process() { return 0; }
sha256sum() { case "$1" in /proc/*/exe) printf '@EXEHASH@  exe\n';; *) command sha256sum "$@";; esac; }
value() { case "$1" in MainPID) VALUE=42;; ExecStart) VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;';; DropInPaths) VALUE=/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf;; ControlGroup) VALUE=/system.slice/xochitl.service;; Job) VALUE='';; *) return 90;; esac; }
query() { printf 'query %s\n' "$*" >> "$root/events"; }
verify_stock() { test ! -e "$guard"; test ! -e "$activation"; printf 'healthy\n' > "$root/stock.restored"; }
'''
                full = (HERE / "actor.sh.in").read_text()
                prefix, boundary = full.split("# Main-only execution boundary", 1)
                text = prefix + overrides + "\n# Main-only execution boundary" + boundary
                for key, replacement in (("@ROOT@", str(root)), ("@NONCE@", "1" * 32),
                                         ("@STOCKPID@", "42"), ("@STOCKSTART@", "561")):
                    text = text.replace(key, replacement)
                text = text.replace("/sys/fs/cgroup/systemd/system.slice/xochitl.service/cgroup.procs", str(root / "cgroup.procs"))
                text = render_fixture(root, text)
                (root / "actor.sh").write_text(text)
                (root / "actor.sh").chmod(0o600)
                result = subprocess.run(["/bin/sh", str(root / "actor.sh")], capture_output=True, text=True, timeout=8)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue((root / "diagnostic.failed").exists())
                self.assertTrue((root / "stock.restored").exists(), result.stderr)
                self.assertFalse((root / "stock-stop.claim").exists())
                self.assertFalse((root / "candidate-start.claim").exists())
                self.assertFalse((root / "dropins").exists())
                events = (root / "events").read_text()
                self.assertNotIn("query stop", events)
                self.assertNotIn("query start", events)

    def test_packet_preparation_is_bound_and_prepare_only(self):
        spec = importlib.util.spec_from_file_location("prepare", HERE / "prepare_packet.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = root / "fixture.so"
            payload.write_bytes(b"fixture only\r\n; never load")
            original_unit = b"[Unit]\nDescription=fixture\nOnFailure=remarkable-fail.service\n[Service]\nWatchdogSec=60\n"
            original_vendor = b"[Unit]\nOnFailure=remarkable-fail.service\n[Service]\nRestartMode=direct\n"
            unit_input, vendor_input = root / "unit-input", root / "vendor-input"
            unit_input.write_bytes(original_unit)
            vendor_input.write_bytes(original_vendor)
            # Original-byte inputs are synthetic; native source hashes are not claimed.
            fixture_hashes = {key: (value[0], hashlib.sha256(data).hexdigest()) for key, value, data in
                              (("stock_unit_sha256", module.SERVICE_HASHES["stock_unit_sha256"], original_unit),
                               ("vendor_dropin_sha256", module.SERVICE_HASHES["vendor_dropin_sha256"], original_vendor))}
            selection = {"nonce": "1" * 32, "budget_seconds": 360, "stock_pid": "42", "stock_start": "561",
                         "original_policy": module.POLICY, "executable_sha256": module.EXE_HASH,
                         "payload_sha256": hashlib.sha256(payload.read_bytes()).hexdigest(),
                         "protected_files": [{"path": f"/fixture/{i}", "sha256": "a" * 64} for i in range(24)]}
            selection.update({key: value[1] for key, value in fixture_hashes.items()})
            selection["protected_files"][0]["path"] = "/usr/lib/libstdc++.so.6"
            selected = root / "selection.json"
            selected.write_text(json.dumps(selection))
            output = root / "packet"
            def prepare(destination):
                arguments = ["prepare", "--selection", str(selected), "--payload", str(payload),
                             "--stock-unit", str(unit_input), "--vendor-dropin", str(vendor_input), "--output", str(destination)]
                captured = io.StringIO()
                with mock.patch.object(module, "SERVICE_HASHES", fixture_hashes), mock.patch.object(os.sys, "argv", arguments), contextlib.redirect_stdout(captured):
                    module.main()
                return json.loads(captured.getvalue())
            receipt = prepare(output)
            self.assertEqual(receipt["evidence_class"], "prepared only; no native execution")
            self.assertFalse((output / "deadline").exists())
            self.assertEqual((output / "owner").read_bytes(), b"1" * 32)
            for name, digest in receipt["files"].items():
                self.assertEqual(hashlib.sha256((output / name).read_bytes()).hexdigest(), digest)
            self.assertNotIn("@ROOT@", (output / "actor.sh").read_text())
            self.assertIn("  /usr/lib/libstdc++.so.6\n", (output / "baseline.files").read_text())
            for generated in output.iterdir():
                if generated.name != "payload.so":
                    self.assertNotIn(b"\r", generated.read_bytes(), generated.name)
            self.assertEqual((output / "payload.so").read_bytes(), payload.read_bytes())
            self.assertEqual((output / "unit-shadow.service").read_bytes(), original_unit.replace(b"OnFailure=remarkable-fail.service\n", b""))
            self.assertEqual((output / "vendor-shadow.conf").read_bytes(), original_vendor.replace(b"OnFailure=remarkable-fail.service\n", b""))
            # Simulate Windows default CRLF writes plus a CRLF source checkout.
            # Explicit newline='\n' must produce byte-identical Linux packets.
            crlf_source = root / "crlf-source"
            crlf_source.mkdir()
            for name in ("actor.sh.in", "launch.sh.in", "trace-stop-proof.awk"):
                (crlf_source / name).write_bytes((HERE / name).read_text().replace("\n", "\r\n").encode("ascii"))
            simulated = root / "windows-simulated"
            original_open = Path.open
            def windows_open(path, mode="r", buffering=-1, encoding=None, errors=None, newline=None):
                if "w" in mode and "b" not in mode and newline is None:
                    newline = "\r\n"
                return original_open(path, mode, buffering, encoding, errors, newline)
            with mock.patch.object(module, "HERE", crlf_source), mock.patch.object(Path, "open", windows_open):
                prepare(simulated)
            for generated in output.iterdir():
                self.assertEqual((simulated / generated.name).read_bytes(), generated.read_bytes(), generated.name)
            # Provider '+' is literal; shell metacharacters and traversal remain refused.
            for index, unsafe in enumerate(("/usr/lib/libstdc++;touch", "/usr/lib/$(id)",
                                             "/usr/lib/`id`", "/usr/lib/lib*.so", "/usr/lib/../escape",
                                             "/usr/lib/lib stdc++.so", "/usr/lib/lib.so\n/escape")):
                selection["protected_files"][0]["path"] = unsafe
                selected.write_text(json.dumps(selection))
                rejected = root / f"rejected-{index}"
                with self.assertRaises(ValueError): prepare(rejected)
                self.assertFalse(rejected.exists())

    def test_prearm_host_loss_expires_without_policy_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            overrides = '''
now() { printf '%s\n' "$(cat "$root/clock")"; }
sleep() { tick=$(cat "$root/clock"); printf '%s\n' "$((tick+1))" > "$root/clock"; }
'''
            (root / "clock").write_text("100\n")
            shell(root, 'deadline=460; wait_request guard.request', overrides, expected=1)
            self.assertEqual((root / "clock").read_text(), "110\n")
            self.assertFalse((root / "guard.install-intent").exists())
            self.assertFalse((root / "stock-stop.claim").exists())

    def test_claim_and_lock_are_singleton(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shell(root, '''
claim actor.claim
if claim actor.claim; then exit 99; fi
: > "$root/actor.lock"
exec 9<> "$root/actor.lock"
flock -n 9
if /bin/sh -c 'exec 9<> "$1"; flock -n 9' sh "$root/actor.lock"; then exit 99; fi
''')

    def test_gone_refuses_failed_job_active_job_and_live_pid_with_empty_cgroup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "cgroup.procs").write_text("")
            for mode in ("failed-job", "active-job", "live-pid", "failed-cgroup"):
                overrides = '''
query() {
 case "$*" in
  *property=Job*) test "$mode" != failed-job || return 91
                  if test "$mode" = active-job; then printf '123 /job\n'; fi;;
  *property=MainPID*) if test "$mode" = live-pid; then printf '42\n'; else printf '0\n'; fi;;
  *property=ControlGroup*) test "$mode" != failed-cgroup || return 91
                          printf '/system.slice/xochitl.service\n';;
 esac > "$root/query.stdout"
}
'''
                shell(root, f'mode={mode}; if cgroup_gone; then exit 99; fi; test ! -e "$root/stock-start.claim"', overrides)

    def test_restore_order_crash_and_one_stock_start(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            parent = root / "dropins"
            parent.mkdir()
            for name, value in (("attempt.identity", "42 561\n"), ("guard.conf", "guard"),
                                ("activation.conf", "activation")):
                (root / name).write_text(value)
                (root / name).chmod(0o600)
            (parent / "guard").write_text("guard")
            (parent / "activation").write_text("activation")
            for file in parent.iterdir():
                file.chmod(0o600)
            parent.chmod(0o700)
            fixture_guard_shadows(root, parent)
            overrides = '''
log() { printf '%s\n' "$*" >> "$root/events"; }
reserve() { return 0; }
policy() { log policy; }
query() { log "query $*"; printf 'Result=signal\n' > "$root/query.stdout"; }
value() { VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'; }
stop_unit() { test -f "$guard"; test -f "$activation"; log stop-under-guard; }
process_gone() { log "gone $*"; }
cgroup_gone() { log cgroup-gone; }
hashes() { log hashes; }
verify_stock() { test ! -e "$guard"; test ! -e "$activation"; log verified-stock; printf 'healthy\n' > "$root/stock.restored"; }
'''
            body = '''
parent="$root/dropins"; guard="$parent/guard"; activation="$parent/activation"
stock_stopped=yes; candidate_started=yes; guard_installed=yes; activation_installed=yes
restore
test -f "$root/diagnostic.failed"; test -f "$root/stock.restored"
if restore; then exit 99; fi
'''
            shell(root, body, overrides)
            events = (root / "events").read_text().splitlines()
            self.assertLess(events.index("stop-under-guard"), events.index("gone 42 561"))
            self.assertLess(events.index("cgroup-gone"), events.index("query daemon-reload"))
            self.assertEqual(events.count("query start --no-block xochitl.service"), 1)
            self.assertEqual(events.count("verified-stock"), 1)

    def test_primary_failure_then_candidate_crash_restores_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            parent = root / "dropins"
            parent.mkdir()
            parent.chmod(0o700)
            for name, data in (("attempt.identity", "42 561\n"), ("guard.conf", "guard"), ("activation.conf", "activation")):
                (root / name).write_text(data)
                (root / name).chmod(0o600)
            for name in ("guard", "activation"):
                (parent / name).write_text(name)
                (parent / name).chmod(0o600)
            fixture_guard_shadows(root, parent)
            overrides = '''
parent="$root/dropins"; guard="$parent/guard"; activation="$parent/activation"
log() { printf '%s\n' "$*" >> "$root/events"; }
validate_root() { :; }
reserve() { return 0; }
policy() { log policy; }
query() { log "query $*"; printf 'Result=signal\n' > "$root/query.stdout"; }
value() { VALUE='path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ;'; }
stop_unit() { test -f "$guard"; test -f "$activation"; log stop-under-guard; }
process_gone() { log "gone $*"; }
cgroup_gone() { log cgroup-gone; }
hashes() { log hashes; }
verify_stock() { test ! -e "$guard"; test ! -e "$activation"; log verified-stock; printf 'healthy\n' > "$root/stock.restored"; }
'''
            _, boundary = (HERE / "actor.sh.in").read_text().split("# Main-only execution boundary", 1)
            child = SOURCE + overrides + "\n# Main-only execution boundary" + boundary
            for key, value in (("@ROOT@", str(root)), ("@NONCE@", "1" * 32), ("@STOCKPID@", "42"), ("@STOCKSTART@", "561")):
                child = child.replace(key, value)
            child = render_fixture(root, child)
            (root / "actor.sh").write_text(child)
            (root / "actor.sh").chmod(0o600)
            body = '''
: > "$root/actor.lock"; exec 9<> "$root/actor.lock"; flock -n 9
for name in actor.claim stock-stop.claim guard.install-intent activation.install-intent candidate-start.claim; do claim "$name"; done
phase=observe-first-render
trap finish EXIT
exit 93
'''
            shell(root, body, overrides, expected=93)
            self.assertEqual((root / "diagnostic.failed").read_bytes(), b"1" * 32 + b"\n")
            self.assertTrue((root / "stock.restored").exists())
            self.assertFalse((root / "recovery.failed").exists())
            events = (root / "events").read_text().splitlines()
            self.assertEqual(events.count("query start --no-block xochitl.service"), 1)
            self.assertEqual(events.count("verified-stock"), 1)
            self.assertFalse(parent.exists())

    def test_failure_marker_foreign_malformed_or_link_refuses_preservation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "diagnostic.failed"
            for data in (b"2" * 32 + b"\n", b"1" * 32, b"1" * 32 + b"\n\n", b"1" * 32 + b"\0"):
                marker.write_bytes(data)
                marker.chmod(0o600)
                shell(root, 'if preserve_failure; then exit 99; fi; test ! -e "$root/stock-start.claim"')
                self.assertEqual(marker.read_bytes(), data)
            marker.unlink()
            target = root / "foreign"
            target.write_bytes(b"1" * 32 + b"\n")
            target.chmod(0o600)
            marker.symlink_to(target)
            shell(root, 'if preserve_failure; then exit 99; fi')
            self.assertTrue(marker.is_symlink())

    def test_deadline_refuses_cleanup_and_retain_guard(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "guard").write_text("guard")
            overrides = 'reserve() { return 90; }; policy() { return 0; }; hashes() { return 0; }'
            shell(root, 'guard="$root/guard"; guard_installed=yes; restore', overrides, expected=90)
            self.assertTrue((root / "guard").exists())
            self.assertFalse((root / "stock-start.claim").exists())

    def test_process_read_failure_is_not_gone(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shell(root, 'if process_gone $$ 1; then exit 99; fi', 'start_of() { return 91; }')

    def test_query_hang_is_bounded_to_owned_child(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            client = root / "client"
            client.write_text(f'#!/bin/sh\nprintf "%s\\n" "$$" > "{root}/child.pid"\nexec sleep 20\n')
            client.chmod(0o700)
            source = SOURCE.replace("@ROOT@", str(root)).replace("/usr/bin/systemctl", str(client))
            begun = time.monotonic()
            result = subprocess.run(["/bin/sh", "-c", source + '\ndeadline=$(($(now)+360)); set +e; query show; rc=$?; test "$rc" = 91 || test "$rc" = 94'],
                                    capture_output=True, text=True, timeout=6)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertLess(time.monotonic() - begun, 5)
            child = int((root / "child.pid").read_text())
            # Killed/reaped or zombie is proof of no executing owned query child.
            path = Path(f"/proc/{child}/stat")
            if path.exists():
                self.assertEqual(path.read_text().rsplit(") ", 1)[1].split()[0], "Z")

    def test_trace_identity_and_invalid_drop_refuse(self):
        valid = "v1 1 startup 100 42 42 561 0 0 0\nv1 2 before-render 101 42 42 561 0 0 1\n"
        for text, expected in ((valid, 0), (valid.replace(" 561 ", " 562 "), 90),
                               (valid.replace("0 0 1", "0 1 1"), 90),
                               (valid.replace("v1 2", "v1 3"), 90)):
            result = subprocess.run(["awk", "-v", "pid=42", "-v", "start=561", "-f", str(HERE / "trace-stop-proof.awk")],
                                    input=text, text=True, capture_output=True)
            self.assertEqual(result.returncode, expected)


if __name__ == "__main__":
    if os.name != "posix":
        raise SystemExit("Run in the owned Linux fixture container; no native result implied")
    unittest.main()
