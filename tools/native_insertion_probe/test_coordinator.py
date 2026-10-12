"""Owned source-local negative fixtures; no SSH, systemd or selected native packet."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
from unittest import mock
import subprocess
import tempfile
import unittest
from open_document_timing import SOURCE5, PRODUCTION_V2, FIXTURE_DOCUMENT, PRESERVED_PAGES

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("coordinator", HERE / "coordinator.py")
coordinator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coordinator)
SOURCE = (HERE / "arm.sh.in").read_text().split("# Fixture source boundary", 1)[0].replace("@CAPTURESPACE@", ":").replace('@CONTROL_POLICY@', 'legacy').replace('@CONTROL_SHELL_SHA256@', '0' * 64)


def run(root, body, overrides="", expected=0):
    source = SOURCE.replace("@ROOT@", str(root)).replace("@NONCE@", "1" * 32)
    result = subprocess.run(["/bin/sh", "-c", source + "\n" + overrides + "\n" + body],
                            capture_output=True, text=True, timeout=8)
    if result.returncode != expected:
        raise AssertionError(f"rc={result.returncode} expected={expected}: {result.stderr}")
    return result


class Coordinator(unittest.TestCase):
    def test_rendered_open_packet_inventory_requires_exact_helper_and_record_modes(self):
        helpers = {"owned_tap_guardian", "observe_input", "publish_creation_arm", "native_frame_acquire", "owned_frame", "owned_frame_alarm"}
        variants = [(None, "valid"), (None, "source5-valid"), ("owned_relative_gesture", "source5-600")]
        for helper in sorted(helpers):
            variants += [(helper, mode) for mode in ("600", "755", "symlink", "hardlink", "wrong-owner")]
        variants += [("owner", "700"), ("payload.so", "700"), ("extra", "extra"), ("owned_tap_guardian", "unselected-helper")]
        for name, mode in variants:
            with self.subTest(name=name, mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)/"packet";root.mkdir(mode=0o700)
                kind = "caller-selected-insertion-v1" if mode=="unselected-helper" else "open-document-insertion-v1"
                receipt = {"nonce": "1"*32, "remote_root": str(root), "experiment_kind": kind, "files": {}}
                selected_helpers = helpers
                if mode.startswith("source5-"):
                    receipt.update(experiment_kind=PRODUCTION_V2, fixture_inventory_variant=SOURCE5,
                                   document_id=FIXTURE_DOCUMENT, before_page_ids=list(PRESERVED_PAGES),
                                   source_index=5, source_page_id=PRESERVED_PAGES[5],
                                   neighbor_index=6, neighbor_page_id=PRESERVED_PAGES[6], insertion_index=6)
                    selected_helpers = helpers | {"owned_relative_gesture"}
                for item in sorted(coordinator.packet_files(receipt)):
                    data = ("synthetic " + item).encode();path = root / item;path.write_bytes(data)
                    path.chmod(0o700 if item in selected_helpers else 0o600)
                    receipt["files"][item] = coordinator.sha(data)
                manifest = "".join(f"{digest}  {item}\n" for item,digest in sorted(receipt["files"].items()))
                (root / "packet.files").write_text(manifest);(root / "packet.files").chmod(0o600)
                receipt["packet_manifest_sha256"] = coordinator.sha(manifest.encode())
                rendered = coordinator.render(receipt, "0"*64)
                # Execute the exact rendered inventory AND subsequent per-file
                # mode checks, stopping before hashing/claim/systemd admission.
                private = rendered[rendered.index("private() {"):rendered.index("now() {")]
                begin = rendered.index('    for file in "$root"/*')
                end = rendered.index('    (cd "$root"; sha256sum -c packet.files)',begin)
                admission = rendered[begin:end]
                if mode == "source5-600": (root / name).chmod(0o600)
                elif mode.isdigit(): (root / name).chmod(int(mode,8))
                elif mode == "symlink":
                    (root / name).unlink();(root / name).symlink_to(root / "payload.so")
                elif mode == "hardlink": os.link(root / name,Path(directory) / "linked-helper")
                elif mode == "wrong-owner": os.chown(root / name,1,0)
                elif mode == "extra": (root / "unrelated").write_bytes(b"extra")
                elif mode == "unselected-helper":
                    (root / name).write_bytes(b"unselected");(root / name).chmod(0o700)
                result = subprocess.run(["/bin/sh","-c", "set -eu\n"+private+f"root='{root}'\n"+admission],
                                        capture_output=True,text=True,timeout=8)
                self.assertEqual(result.returncode==0,mode in {"valid", "source5-valid"},result.stderr)
                for marker in ("host-arm.claim","deadline","guard.request","start.request"):
                    self.assertFalse((root / marker).exists())

    @unittest.skipUnless(os.environ.get("REM25_SOURCE5_PREPARED_PACKET"), "prepared packet not supplied")
    def test_actual_prepared_source5_packet_inventory_modes(self):
        packet = Path(os.environ["REM25_SOURCE5_PREPARED_PACKET"])
        receipt_bytes = (packet / "preparation.json").read_bytes()
        receipt = json.loads(receipt_bytes)
        profile = json.loads((packet / "host-profile.json").read_bytes())
        self.assertEqual(receipt["fixture_inventory_variant"], SOURCE5)
        self.assertEqual(set(profile["tools"]), {
            "owned_tap_guardian", "observe_input", "publish_creation_arm",
            "native_frame_acquire", "owned_frame", "owned_frame_alarm", "owned_relative_gesture"})
        expected = coordinator.packet_files(receipt) | {"packet.files", "preparation.json"}
        self.assertEqual({p.name for p in packet.iterdir()}, expected)
        for name, digest in receipt["files"].items():
            self.assertEqual(coordinator.sha((packet / name).read_bytes()), digest, name)
        for changed, mode in ((None, None), ("owned_relative_gesture", 0o600),
                              ("owned_relative_gesture", 0o755), ("gesture-paths-next.json", 0o700)):
            with self.subTest(changed=changed, mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "packet"
                root.mkdir(mode=0o700)
                for name in expected:
                    path = root / name
                    path.write_bytes((packet / name).read_bytes())
                    path.chmod(0o700 if name in profile["tools"] else 0o600)
                if changed:
                    (root / changed).chmod(mode)
                local_receipt = dict(receipt, remote_root=str(root))
                rendered = coordinator.render(local_receipt, coordinator.sha(receipt_bytes))
                private = rendered[rendered.index("private() {"):rendered.index("now() {")]
                begin = rendered.index('    for file in "$root"/*')
                end = rendered.index('    (cd "$root"; sha256sum -c packet.files)', begin)
                result = subprocess.run(["/bin/sh", "-c", "set -eu\n" + private +
                                         f"root='{root}'\n" + rendered[begin:end]],
                                        capture_output=True, text=True, timeout=8)
                self.assertEqual(result.returncode == 0, changed is None, result.stderr)
                for marker in ("host-arm.claim", "deadline", "guard.request", "start.request"):
                    self.assertFalse((root / marker).exists())

    @unittest.skipUnless(os.environ.get("REM25_SOURCE5_PREPARED_PACKET"), "prepared packet not supplied")
    def test_prepared_packet_hash_preflight_fits_bounded_arming_reply(self):
        packet = Path(os.environ["REM25_SOURCE5_PREPARED_PACKET"])
        receipt_bytes = (packet / "preparation.json").read_bytes()
        receipt = json.loads(receipt_bytes)
        tools = json.loads((packet / "host-profile.json").read_bytes())["tools"]
        for corrupt in (None, "payload.so", "baseline", "service"):
            with self.subTest(corrupt=corrupt), tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "packet"
                root.mkdir(mode=0o700)
                for member in packet.iterdir():
                    path = root / member.name
                    path.write_bytes(member.read_bytes())
                    path.chmod(0o700 if member.name in tools else 0o600)
                # Preserve the actual packet hashes. Relocate external hash
                # targets to synthetic local files, keeping exact row counts
                # and long paths; no live service or device paths are accessed.
                targets = {}
                manifests = {}
                for kind, name in (("baseline", "baseline.files"), ("service", "service.files")):
                    rows = []
                    for index, row in enumerate((packet / name).read_text().splitlines()):
                        target = Path(directory) / (kind + "-" + "x" * 100 + str(index))
                        data = row.encode()
                        target.write_bytes(data)
                        rows.append(f"{coordinator.sha(data)}  {target}\n")
                        targets[kind] = target
                    manifests[kind] = Path(directory) / name
                    manifests[kind].write_text("".join(rows))
                if corrupt:
                    target = root / corrupt if corrupt == "payload.so" else targets[corrupt]
                    target.write_bytes(b"changed after preparation")
                rendered = coordinator.render(dict(receipt, remote_root=str(root)), coordinator.sha(receipt_bytes))
                private = rendered[rendered.index("private() {"):rendered.index("now() {")]
                begin = rendered.index('    for file in "$root"/*')
                end = rendered.index('    claim host-arm.claim', begin)
                preflight = rendered[begin:end]
                for kind, name in (("baseline", "baseline.files"), ("service", "service.files")):
                    preflight = preflight.replace(f'sha256sum -c "$root/{name}"',
                                                  f"sha256sum -c '{manifests[kind]}'")
                # Capture-space qualification is a separate device check.
                preflight = preflight[:preflight.index('    test "$(df -Pk /run')]
                report = next(line for line in rendered.splitlines() if "admitted-once" in line)
                script = ("set -eu\n" + private + f"root='{root}'\nnonce='{receipt['nonce']}'\n" +
                          preflight + "saved_actor_pid=2147483647; saved_actor_start=999999999999; deadline=999999999999\n" + report)
                result = coordinator.bounded_control_transport(["/bin/sh"], script, 8)
                self.assertFalse(result.get("output_limit_exceeded", False), result)
                self.assertFalse(result.get("uncertain_remote_action", False), result)
                self.assertEqual(result["exit"] == 0, corrupt is None, result)
                self.assertLessEqual(len((result["stdout"] + result["stderr"]).encode()), 1024)
                self.assertEqual("admitted-once" in result["stdout"], corrupt is None)
                if corrupt:
                    self.assertTrue(result["stderr"], "hash failure diagnostic was lost")
                for marker in ("host-arm.claim", "deadline", "guard.request", "start.request"):
                    self.assertFalse((root / marker).exists())

    def test_actual_nine_second_preflight_bounded_ready_and_thirty_second_refusal(self):
        for source_template, ready_at, success in ((SOURCE,109,True), (SOURCE,124,True), (SOURCE,130,False), (SOURCE,999,False)):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                root.chmod(0o700)
                for name, data in (("owner", "1" * 32), ("packet.files", "fixture")):
                    (root / name).write_text(data)
                    (root / name).chmod(0o600)
                source = source_template.replace("@ROOT@", str(root)).replace("@NONCE@", "1" * 32)
                source = source.replace("@MANIFESTHASH@", coordinator.sha(b"fixture"))
                source = source.replace("@ALLOWEDFILES@", "owner|packet.files").replace("@PRIVATEFILES@", ":")
                source = source.replace("/run/systemd/system/xochitl.service.d", str(root / "dropins"))
                overrides = '''
tick=100
now() { printf '%s\n' "$tick"; }
sha256sum() { if test "${1-}" = -c; then return 0; fi; command sha256sum "$@"; }
bounded() { test -f "$root/deadline"; tick=101; }
actor_proof() { test -f "$root/actor.ready"; saved_actor_pid=42; saved_actor_start=561; }
guard_proof() { test -f "$root/guard.ready"; }
sleep() {
 tick=$((tick+1))
 if test "$tick" -ge "$ready_at" && ! test -e "$root/actor.ready"; then claim actor.ready; fi
 if test -e "$root/guard.request" && ! test -e "$root/guard.ready"; then claim guard.ready; fi
}
'''
                result = subprocess.run(["/bin/sh", "-c", source + overrides + f'\nready_at={ready_at}; main'],
                                        capture_output=True, text=True, timeout=8)
                self.assertEqual(result.returncode == 0, success, result.stderr)
                self.assertEqual((root / "guard.request").exists(), success)
                self.assertEqual((root / "start.request").exists(), success)
                self.assertEqual((root / "deadline").read_text(), "100 460\n")

    def test_explicit_usb_and_pinned_key_no_config_proxy_or_alias(self):
        args = coordinator.ssh_args(Path("/fixture/pinned-hosts"))
        joined = " ".join(args)
        self.assertIn("HostName=10.11.99.1", joined)
        self.assertIn("HostKeyAlias=rem25-usb-rm2", joined)
        self.assertIn("StrictHostKeyChecking=yes", joined)
        self.assertIn("-F none", joined)
        self.assertIn("root@10.11.99.1", args)
        self.assertNotIn("RM2", args)
        self.assertNotIn("scp", args)

    def test_whole_local_packet_and_host_pin_reject_changed_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            receipt = {"nonce": "1" * 32, "remote_root": "/run/rmb-qt-probe-" + "1" * 32,
                       "experiment_kind": "caller-selected-insertion-v1", "absolute_budget_seconds": 360, "evidence_class": "prepared only; no native execution", "files": {}}
            manifest = ""
            for name in sorted(coordinator.FILES):
                data = ("synthetic " + name).encode()
                (root / name).write_bytes(data)
                digest = coordinator.sha(data)
                receipt["files"][name] = digest
                manifest += f"{digest}  {name}\n"
            (root / "packet.files").write_text(manifest)
            receipt["packet_manifest_sha256"] = coordinator.sha(manifest.encode())
            (root / "preparation.json").write_text(json.dumps(receipt))
            digest = coordinator.sha((root / "preparation.json").read_bytes())
            coordinator.load_packet(root, digest)
            for nonce in ("2b34ff126957497285b66f77311df07e", "8287c096cca84cf6a0998b8d5f178c0d", "4459cae2f852426b8262f658c6aa59ed", "a22fc58d4c9948098f7d2ccc8a9de8c4", "94f4037a4f3c40b881fe491b9b404bc3", "94774266c10f41e2aa8d410012fee601", "026e74d06f7248ae809bf1b1d6a417ba", "ff497d18868042319864c82bb34c0e5e"):
                spent = dict(receipt)
                spent["nonce"] = nonce
                spent["remote_root"] = "/run/rmb-qt-probe-" + spent["nonce"]
                (root / "preparation.json").write_text(json.dumps(spent))
                with self.assertRaises(ValueError):
                    coordinator.load_packet(root, coordinator.sha((root / "preparation.json").read_bytes()))
            (root / "preparation.json").write_text(json.dumps(receipt))
            # Default CLI renders selected synthetic bytes and cannot open transport.
            pinned = root / "known-hosts"
            pinned.write_text("rem25-usb-rm2 ssh-ed25519 Zml4dHVyZQ==\n")
            identity = root / "identity"
            identity.write_text("synthetic identity; do not read")
            original_read_bytes = Path.read_bytes
            original_read_text = Path.read_text
            def checked_bytes(path, *args, **kwargs):
                if path == identity: raise AssertionError("Identity bytes were read")
                return original_read_bytes(path, *args, **kwargs)
            def checked_text(path, *args, **kwargs):
                if path == identity: raise AssertionError("Identity text was read")
                return original_read_text(path, *args, **kwargs)
            args = ["coordinator", "--packet", str(root), "--receipt-sha256", digest,
                    "--known-hosts", str(pinned), "--known-hosts-sha256", coordinator.sha(pinned.read_bytes()),
                    "--identity-file", str(identity), "--output", str(root / "rendered")]
            with mock.patch.object(os.sys, "argv", args), mock.patch.object(coordinator, "transport", side_effect=AssertionError("Transport forbidden")), \
                    mock.patch.object(Path, "read_bytes", checked_bytes), mock.patch.object(Path, "read_text", checked_text):
                coordinator.main()
                ssh = coordinator.ssh_args(pinned, identity)
                self.assertIn("-i", ssh)
                self.assertIn("IdentitiesOnly=yes", ssh)
            self.assertNotIn("synthetic identity", (root / "rendered" / "bindings.json").read_text())
            subprocess.run(["/bin/sh", "-n", str(root / "rendered" / "arm.sh")], check=True)
            (root / "payload.so").write_bytes(b"changed")
            with self.assertRaises(ValueError): coordinator.load_packet(root, digest)
            with self.assertRaises(ValueError): coordinator.load_packet(root, "0" * 64)
            pinned.write_text("rem25-usb-rm2 ssh-ed25519 Zml4dHVyZQ==\n")
            coordinator.pin_hosts(pinned, coordinator.sha(pinned.read_bytes()))
            with self.assertRaises(ValueError): coordinator.pin_hosts(pinned, "0" * 64)
            pinned.write_text("RM2 ssh-ed25519 Zml4dHVyZQ==\n")
            with self.assertRaises(ValueError): coordinator.pin_hosts(pinned, coordinator.sha(pinned.read_bytes()))

    def test_failed_query_or_wrong_actor_property_state_identity_refuses_requests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("actor.ready", "actor.claim", "actor.lock"):
                (root / name).write_text("1" * 32 + "\n")
                (root / name).chmod(0o600)
            for mode in ("failed-query", "wrong-property", "wrong-state", "wrong-identity", "wrong-fd"):
                overrides = '''
exec 9<> "$root/actor.lock"
tr() { printf '/bin/sh\n%s/actor.sh\n' "$root"; }
show() {
 test "$mode" != failed-query || return 91
 {
 for line in Type=oneshot TimeoutStartUSec=6min Restart=no OnFailure= FailureAction=none StartLimitAction=none KillMode=control-group ActiveState=activating SubState=start BindsTo= PartOf= StopWhenUnneeded=no; do
  if test "$mode" = wrong-property && test "$line" = Restart=no; then printf 'Restart=on-failure\n'
  elif test "$mode" = wrong-state && test "$line" = ActiveState=activating; then printf 'ActiveState=failed\n'
  else printf '%s\n' "$line"; fi
 done
 printf 'MainPID=%s\nExecMainPID=%s\nExecStart={ path=/bin/sh ; argv[]=/bin/sh %s/actor.sh ; }\n' "$$" "$$" "$root"
 } > "$root/host-query.stdout"
}
'''
                body = f'''mode={mode}
if test "$mode" = wrong-identity; then saved_actor_pid=42; saved_actor_start=1; fi
if test "$mode" = wrong-fd; then exec 9>&-; fi
if actor_proof; then claim guard.request; claim start.request; exit 99; fi
test ! -e "$root/guard.request"; test ! -e "$root/start.request"
'''
                run(root, body, overrides)

    def test_wrong_guard_policy_or_failed_query_cannot_admit_start(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "guard.ready").write_text("1" * 32 + "\n")
            (root / "guard.ready").chmod(0o600)
            (root / "guard.policy").write_text("OnFailure=\nRestart=no\n")
            # Redirect only the OS guard path boundary to owned fixture files.
            (root / "guard").write_text("guard")
            (root / "guard").chmod(0o600)
            (root / "unit-shadow").write_text("unit")
            (root / "vendor-shadow").write_text("vendor")
            (root / "unit-shadow").chmod(0o600)
            (root / "vendor-shadow").chmod(0o600)
            source = SOURCE.replace('guard="/run/systemd/system/xochitl.service.d/zz-rmb-insertion-$nonce-guard.conf"', 'guard="$root/guard"')
            source = source.replace("@ROOT@", str(root)).replace("@NONCE@", "1" * 32).replace("@GUARDHASH@", coordinator.sha(b"guard"))
            source = source.replace("unit_shadow=/run/systemd/system/xochitl.service", 'unit_shadow="$root/unit-shadow"')
            source = source.replace("vendor_shadow=/run/systemd/system/xochitl.service.d/xochitl-service-override.conf", 'vendor_shadow="$root/vendor-shadow"')
            source = source.replace("@UNITSHADOWHASH@", coordinator.sha(b"unit")).replace("@VENDORSHADOWHASH@", coordinator.sha(b"vendor"))
            for mode in ("failed-query", "wrong-policy", "wrong-fragment", "vendor-leak"):
                script = source + '''
show() {
 test "$mode" != failed-query || return 91
 fragment=$unit_shadow; dropins="$vendor_shadow $guard"
 if test "$mode" = wrong-fragment; then fragment=/usr/lib/systemd/system/xochitl.service; fi
 if test "$mode" = vendor-leak; then dropins="/usr/lib/systemd/system/xochitl.service.d/xochitl-service-override.conf $guard"; fi
 if test "$mode" = wrong-policy; then printf 'OnFailure=remarkable-fail.service\nRestart=on-failure\n'; else printf 'OnFailure=\nRestart=no\n'; fi > "$root/host-query.stdout"
 printf 'FragmentPath=%s\nDropInPaths=%s\nExecStart={ path=/usr/bin/xochitl ; argv[]=/usr/bin/xochitl --system ; }\n' "$fragment" "$dropins" >> "$root/host-query.stdout"
}
''' + f'mode={mode}; if guard_proof; then claim start.request; exit 99; fi; test ! -e "$root/start.request"'
                result = subprocess.run(["/bin/sh", "-c", script], capture_output=True, text=True, timeout=8)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_spent_root_and_wrong_remote_manifest_refuse_deadline_or_unit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            (root / "owner").write_text("1" * 32)
            (root / "owner").chmod(0o600)
            (root / "packet.files").write_text("wrong")
            (root / "packet.files").chmod(0o600)
            overrides = 'bounded() { printf invoked > "$root/unit.called"; }'
            # Initially absent real runtime directory; source-local path injection.
            local = SOURCE.replace("/run/systemd/system/xochitl.service.d", str(root / "absent-dropins"))
            local = local.replace("@ROOT@", str(root)).replace("@NONCE@", "1" * 32)
            for spent in (False, True):
                if spent: (root / "host-arm.claim").write_text("spent")
                result = subprocess.run(["/bin/sh", "-c", local + "\n" + overrides + "\nmain"], capture_output=True, text=True, timeout=8)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((root / "deadline").exists())
                self.assertFalse((root / "unit.called").exists())

    def test_template_order_and_existing_request_window_are_fixed(self):
        text = SOURCE
        entry = text.split("main() {", 1)[1]
        self.assertLess(entry.index('sha256sum -c packet.files'), entry.index('claim host-arm.claim'))
        self.assertLess(entry.index('> "$root/deadline"'), entry.index('bounded /usr/bin/systemd-run'))
        self.assertLess(entry.index('actor_proof'), entry.index('claim guard.request'))
        self.assertLess(entry.index('guard_proof'), entry.index('claim start.request'))
        self.assertIn('stage_deadline=$((guard_requested_at+9))', entry)
        self.assertIn('ready_end=$((armed_at+30))', entry)
        self.assertIn('setup_cutoff=$((armed_at+48))', entry)
        self.assertNotIn('ready_end=$(($(now)+', entry)
        self.assertIn('test "$(now)" -lt "$stage_deadline"', entry)
        self.assertNotIn('systemctl stop', text)
        self.assertNotIn('systemctl start', text)

    def test_actual_entry_order_and_failed_actor_proof_never_requests_guard(self):
        for mode in ("none", "actor", "guard", "late"):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                root.chmod(0o700)
                for name, content in (("owner", "1" * 32), ("packet.files", "fixture")):
                    (root / name).write_text(content)
                    (root / name).chmod(0o600)
                source = SOURCE.replace("@ROOT@", str(root)).replace("@NONCE@", "1" * 32)
                source = source.replace("@MANIFESTHASH@", coordinator.sha(b"fixture"))
                source = source.replace("@ALLOWEDFILES@", "owner|packet.files").replace("@PRIVATEFILES@", ":")
                source = source.replace("/run/systemd/system/xochitl.service.d", str(root / "dropins"))
                overrides = '''
tick=100
now() { printf '%s\n' "$tick"; }
sha256sum() { if test "${1-}" = -c; then return 0; fi; command sha256sum "$@"; }
bounded() { test -f "$root/deadline"; test ! -e "$root/guard.request"; printf unit >> "$root/events"; claim actor.ready; }
actor_proof() { printf actor >> "$root/events"; test "$mode" != actor || return 90; saved_actor_pid=42; saved_actor_start=561; }
guard_proof() { test -f "$root/guard.request"; test ! -e "$root/start.request"; printf guard >> "$root/events"; test "$mode" != guard || return 90; if test "$mode" = late; then tick=109; fi; }
sleep() { claim guard.ready; }
'''
                result = subprocess.run(["/bin/sh", "-c", source + "\n" + overrides + f'\nmode={mode}; main'], capture_output=True, text=True, timeout=8)
                if mode != "none":
                    self.assertEqual(result.returncode, 1 if mode == "late" else 90, result.stderr)
                    self.assertEqual((root / "guard.request").exists(), mode != "actor")
                    self.assertFalse((root / "start.request").exists())
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual((root / "events").read_text(), "unitactorguardactor")
                    self.assertTrue((root / "start.request").exists())
                self.assertEqual((root / "deadline").read_text(), "100 460\n")

    def test_monitor_is_read_only_bounded_and_never_infers_success(self):
        script = coordinator.monitor_script({"remote_root": "/run/rmb-qt-probe-" + "1" * 32})
        self.assertIn('bs=18433 count=1', script)
        self.assertNotIn("systemctl", script)
        self.assertNotIn(" > ", script)
        self.assertNotIn("rm ", script)


if __name__ == "__main__":
    if os.name != "posix": raise SystemExit("Owned Linux fixture required")
    unittest.main()
