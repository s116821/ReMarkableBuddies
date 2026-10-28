"""Offline transport refusal and cleanup scope checks; no device access."""
from pathlib import Path
import base64
import json
import hashlib
import tempfile
import os
import shutil
import subprocess
import unittest
from unittest.mock import patch
from open_document_host import Refused
from open_document_transport import SSHAdapter
from owned_frame_capture import TOOLS as CAPTURE_TOOLS


class AdapterTests(unittest.TestCase):
    def guardian(self, adapter, status):
        return {"nonce": adapter.profile["nonce"], "boot_id": adapter.profile["boot_id"], "pid": 24,
                "start_ticks": 33, "slot": 0, "tracking_id": 123, "native_point": adapter.selected_tap()[1],
                "lease_end_ms": 99000, "status": status, "exclusive_kernel_ownership": False}

    def test_v2_guardian_fixed_point_and_command_preserve_legacy(self):
        for variant in (None, "primary-1641396", "alternate-2011360"):
            v2 = variant is not None
            with self.subTest(v2=v2), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, {"timeout": True, "exit": None})
                if v2:
                    from open_document_timing import PRODUCTION_V2, FIXTURE_DOCUMENT, ORIGINAL_PAGES
                    adapter.profile["experiment_kind"] = PRODUCTION_V2
                    adapter.profile.update(document_id=FIXTURE_DOCUMENT, page_ids=list(ORIGINAL_PAGES))
                    adapter.profile["library_tap_variant"] = adapter.receipt["library_tap_variant"] = variant
                    # Exercise dispatch without live context/timing; those guards
                    # have their own executed-source controls.
                    adapter.production_closure_checks = lambda: ""
                    adapter.prelude = lambda *args: ""
                    adapter.action_budget = lambda *args: (3, None)
                expected = [164, 1396] if variant == "primary-1641396" else [201, 1360]
                receipt = self.guardian(adapter, "guardian-released") | {"native_point": expected}
                self.assertEqual(adapter.tap_receipt((json.dumps(receipt)+"\n").encode(),
                                                     "guardian-released", 100)["native_point"], expected)
                for point in ([201, 1360] if expected == [164, 1396] else [164, 1396], [164, 1360], [201, 1396], [float(axis) for axis in expected], str(expected), None):
                    with self.assertRaises(Refused):
                        adapter.tap_receipt((json.dumps(receipt | {"native_point": point})+"\n").encode(),
                                            "guardian-released", 100)
                with self.assertRaises(Refused):
                    adapter.open_once(("boot", 2, 3), 100)
                self.assertEqual(len(calls), 1)
                command = "--tap-library-v2" if variant == "primary-1641396" else "--tap-library-v2-alt" if v2 else "--tap"
                self.assertIn(f'owned_tap_guardian" {command} ', calls[0])

    def test_v2_missing_unknown_and_conflicting_tap_selection_refuses_before_attempt(self):
        from open_document_timing import PRODUCTION_V2
        for variant in (None, "", "unknown", 1, [], {}):
            with self.subTest(variant=variant), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, {})
                adapter.profile["experiment_kind"] = PRODUCTION_V2
                adapter.profile["library_tap_variant"] = variant
                with self.assertRaises(Refused): adapter.open_once(("boot",2,3),100)
                self.assertFalse(adapter.tap_attempted)
                with self.assertRaises(Refused):adapter.arm_once(None,None,{})
                self.assertFalse(adapter.arm_attempted)
                self.assertFalse(calls)
        for receipt_variant in (None, "unknown", 1, [], {}, "alternate-2011360"):
            with tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root,{})
                adapter.profile["experiment_kind"] = PRODUCTION_V2
                adapter.receipt["library_tap_variant"] = receipt_variant
                with self.assertRaises(Refused): adapter.open_once(("boot",2,3),100)
                self.assertFalse(adapter.tap_attempted)
                with self.assertRaises(Refused):adapter.arm_once(None,None,{})
                self.assertFalse(adapter.arm_attempted)
                self.assertFalse(calls)
        for missing in ("profile","receipt"):
            with tempfile.TemporaryDirectory() as root:
                adapter,calls=self.adapter(root,{})
                adapter.profile["experiment_kind"]=PRODUCTION_V2
                (adapter.profile if missing=="profile" else adapter.receipt).pop("library_tap_variant")
                with self.assertRaises(Refused):adapter.open_once(("boot",2,3),100)
                with self.assertRaises(Refused):adapter.arm_once(None,None,{})
                self.assertFalse(adapter.tap_attempted);self.assertFalse(adapter.arm_attempted)
                self.assertFalse(calls)

    def test_validated_v2_selection_cannot_switch_during_adapter_lifetime(self):
        from open_document_timing import PRODUCTION_V2
        with tempfile.TemporaryDirectory() as root:
            adapter,calls=self.adapter(root,{})
            adapter.profile["experiment_kind"]=PRODUCTION_V2
            self.assertEqual(adapter.selected_tap()[1],[164,1396])
            adapter.profile["library_tap_variant"]=adapter.receipt["library_tap_variant"]="alternate-2011360"
            with self.assertRaises(Refused):adapter.selected_tap()
            with self.assertRaises(Refused):adapter.open_once(("boot",2,3),100)
            self.assertFalse(calls)

    def test_guardian_source_and_artifact_drift_refuse_before_transport(self):
        import coordinator
        from test_host_profile import ProfileTests
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            receipt, profile = ProfileTests().fixture(root)
            for changed in (
                profile | {"source_sha256": profile["source_sha256"] | {"owned_tap_guardian.c": "0"*64}},
                profile | {"tools": profile["tools"] | {"owned_tap_guardian": "0"*64}},
            ):
                (root/"host-profile.json").write_text(json.dumps(changed))
                with self.assertRaisesRegex(ValueError, "execution refused"):
                    coordinator.load_host_profile(root, receipt)

    def test_guardian_ready_release_is_bound_and_go_is_single_shot(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            ready = (json.dumps(self.guardian(adapter, "guardian-ready-no-down")) + "\n").encode()
            release = (json.dumps(self.guardian(adapter, "guardian-released")) + "\n").encode()
            supervisor = (json.dumps(self.guardian(adapter, "normal-child-exit")) + "\n").encode()
            encode = lambda data: base64.b64encode(data).decode()
            output = iter([json.dumps({"ready": encode(ready), "setup": encode(b"setup bytes"), "selected_cutoff_ms": 100000, "uptime": encode(b"98.00 0.00\n")}),
                           json.dumps({"released": encode(release), "supervisor": encode(supervisor), "lease": encode(b"lease bytes"),
                                       "publication": encode((json.dumps({"nonce": adapter.profile["nonce"], "published": True, "retry_permitted": False, "input_events": 0}) + "\n").encode())})])
            def transport(arguments, script, timeout):
                calls.append(script)
                return {"timeout": False, "exit": 0, "stdout": next(output), "stderr": ""}
            adapter.transport = transport
            adapter.open_once(("boot", 2, 3), 100)
            self.assertTrue(adapter.tap_released)
            self.assertEqual(len(calls), 2)
            self.assertIn('nohup "$root/owned_tap_guardian"', calls[0])
            self.assertNotIn('> "$root/tap.go"', calls[0])
            self.assertEqual(calls[1].count('--publish-go'), 1)
            self.assertNotIn('> "$root/tap.go"', calls[1])
            self.assertNotIn('bounded "$root/owned_tap_guardian"', calls[1])
            self.assertNotIn('/tmp/reader-buddy-probe.png', "".join(calls))
            with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
            self.assertEqual(len(calls), 2)

    def test_takeover_receipt_or_identity_change_never_admits_arm(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            for changed in (self.guardian(adapter, "conditional-takeover-released"),
                            self.guardian(adapter, "guardian-released") | {"nonce": "2" * 32},
                            self.guardian(adapter, "guardian-released") | {"lease_end_ms": 100001}):
                with self.assertRaises(Refused):
                    adapter.tap_receipt((json.dumps(changed) + "\n").encode(), "guardian-released", 100)
            with self.assertRaises(Refused): adapter.arm_once(None, None, {})
            self.assertFalse(calls)

    def test_bad_atomic_permission_receipt_never_admits_release_or_replay(self):
        changes = ({"nonce": "2" * 32}, {"published": False}, {"published": 1},
                   {"retry_permitted": True}, {"input_events": 1}, {"input_events": False})
        for changed in changes:
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, {})
                encode = lambda data: base64.b64encode(data).decode()
                receipt = lambda status: (json.dumps(self.guardian(adapter, status)) + "\n").encode()
                publication = {"nonce": adapter.profile["nonce"], "published": True, "retry_permitted": False, "input_events": 0} | changed
                responses = iter([
                    json.dumps({"ready": encode(receipt("guardian-ready-no-down")), "setup": encode(b"setup bytes"), "selected_cutoff_ms": 100000, "uptime": encode(b"98.00 0.00\n")}),
                    json.dumps({"released": encode(receipt("guardian-released")), "supervisor": encode(receipt("normal-child-exit")), "lease": encode(b"lease bytes"), "publication": encode((json.dumps(publication) + "\n").encode())})])
                def transport(arguments, script, timeout):
                    calls.append(script)
                    return {"timeout": False, "exit": 0, "stdout": next(responses)}
                adapter.transport = transport
                with self.assertRaisesRegex(Refused, "publication not confirmed"):
                    adapter.open_once(("boot", 2, 3), 100)
                self.assertTrue((Path(root) / "tap-go-publication.json").exists())
                self.assertFalse(adapter.tap_released)
                with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
                self.assertEqual(len(calls), 2)

    def adapter(self, root, result):
        calls = []
        def transport(arguments, script, timeout):
            calls.append(script)
            if script == 'set -eu\ncat /proc/uptime\n':
                return {"timeout": False, "exit": 0, "stdout": "51.00 0.00\n"}
            return result
        adapter = SSHAdapter([], {"remote_root": "/run/rmb-qt-probe-" + "1" * 32, "library_tap_variant": "primary-1641396"},
                             {"nonce": "1" * 32, "document_id": "fixture", "library_tap_variant": "primary-1641396",
                              "boot_id": "boot", "tools": CAPTURE_TOOLS | {name: 'a' * 64 for name in ('owned_tap_guardian', 'observe_input', 'publish_creation_arm')},
                              "encoder_hashes": {path: 'b' * 64 for path in ('/usr/bin/openssl', '/usr/lib/libssl.so.3', '/usr/lib/libcrypto.so.3')}}, root, transport, b"")
        adapter.root_inode = "1 2"
        return adapter, calls

    def test_uncertain_open_is_never_replayed(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": True, "exit": None})
            with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
            with self.assertRaises(Refused): adapter.open_once(("boot", 2, 3), 100)
            self.assertEqual(len(calls), 1)
            self.assertNotIn('kill -KILL "$child"', calls[0])
            self.assertIn('nohup "$root/owned_tap_guardian" --tap', calls[0])
            self.assertNotIn('bounded "$root/owned_tap_guardian"', calls[0])
            self.assertTrue((Path(root) / "transport-001.json").exists())

    def test_guardian_sequence_never_mutates_shared_debug_image(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": False, "exit": 0, "stdout": ""})
            adapter.restore_probe()
            self.assertFalse(calls)
            adapter.tap_attempted = True
            adapter.restore_probe()
            self.assertFalse(calls)

    def test_stop_is_one_attempt_even_on_uncertainty(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": True, "exit": None})
            with self.assertRaises(Refused): adapter.stop_once(("boot", 2, 3), 100)
            adapter.stop_once(("boot", 2, 3), 100)
            self.assertEqual(len(calls), 1)

    def raw_response(self):
        encode = lambda value: base64.b64encode(value).decode()
        raw = {"pid": 2, "start_ticks": 3, "root_inode": "1 2", "window": encode(("1" * 32 + " 1 361 50 120\n").encode()),
               "waiting": encode(b"waiting"), "uptime": encode(b"51.00 0.00\n"), "last_open": encode(b"unknown"),
               "content": encode(b"{}"), "metadata": encode(b"{}"), "png": encode(b"raw image"),
               "hashes": encode(b""), "idle": encode(b""), "diagnostics": "",
               "frame_hash": "a" * 64, "native_hash": "b" * 64, "capture_receipt": encode(("nonce=" + "1" * 32 + " frame_sha256=" + "a" * 64 + " native_sha256=" + "b" * 64 + " overview_sha256=" + hashlib.sha256(b"raw image").hexdigest() + " conversion_complete=true native_authority=false\n").encode())}
        return raw

    def test_open_frames_are_exclusive_bounded_and_do_not_renew_cutoff(self):
        raw = self.raw_response()
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {"timeout": False, "exit": 0, "stdout": json.dumps(raw)})
            adapter.profile.update(immutable_hashes={}, boot_id="02b2347b-03aa-4a85-8dd7-d731aabf6f3e")
            for _ in range(5):
                adapter.raw_snapshot("open", 100, False)
            with self.assertRaisesRegex(Refused, "frame count exhausted"):
                adapter.raw_snapshot("open", 200, False)
            captures = [script for script in calls if 'host-open-' in script]
            self.assertEqual(len(captures), 5)
            for index, script in enumerate(captures):
                self.assertIn(f'host-open-{index:03}.png', script)
                self.assertIn("cutoff=100\n", script)
                self.assertNotIn("cutoff=200\n", script)
            self.assertFalse(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)

    def test_ready_attempts_have_distinct_prefixes_fixed_cutoff_and_full_idle(self):
        for fault in ('cap', 'renew', 'idle'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, {'timeout': False, 'exit': 0, 'stdout': json.dumps(self.raw_response())})
                adapter.profile.update(immutable_hashes={}, boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
                count = 3 if fault == 'cap' else 1
                for _ in range(count): adapter.raw_snapshot('ready', 100, True)
                before = len(calls)
                with self.assertRaises(Refused):
                    adapter.raw_snapshot('ready', 200 if fault == 'renew' else 100, fault != 'idle')
                self.assertEqual(len(calls), before)
                captures = [s for s in calls if 'host-ready-' in s]
                for index, script in enumerate(captures):
                    self.assertIn(f'host-ready-{index:03}.claim', script)
                    self.assertIn('--observe "$root/observe_input" 2', script)
                    self.assertIn('cutoff=100\n', script)

    def test_capture_transport_failure_or_partial_json_is_terminal(self):
        for response in ({'timeout': True, 'exit': None, 'stdout': '{"png":'},
                         {'timeout': False, 'exit': 0, 'stdout': '{"png":'}):
            with self.subTest(response=response), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, response)
                adapter.profile.update(immutable_hashes={}, boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
                with self.assertRaises((Refused, json.JSONDecodeError)): adapter.raw_snapshot('ready', 100, True)
                before = len(calls)
                with self.assertRaisesRegex(Refused, 'Prior capture failed'): adapter.raw_snapshot('ready', 100, True)
                self.assertEqual(len(calls), before)
                self.assertEqual(adapter.ready_frames, 1)

    def test_clock_query_time_is_charged_and_cap_never_renews_stage(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            for clock, cutoff, expected in (([10, 12], 60, 6.99), ([10, 11], 100, 20)):
                # Tight stage has 6.99 seconds left after the 2-second query.
                with patch('open_document_transport.time.monotonic', side_effect=clock):
                    timeout, deadline = adapter.snapshot_timeout(cutoff)
                self.assertAlmostEqual(timeout, expected)
                self.assertAlmostEqual(deadline, clock[-1] + timeout)
            with patch('open_document_transport.time.monotonic', side_effect=[10, 20]), self.assertRaises(Refused):
                adapter.snapshot_timeout(60)

    def test_bad_or_timed_out_clock_query_never_claims_remote_capture(self):
        for response in ({'timeout': False, 'exit': 0, 'stdout': 'not a clock'},
                         {'timeout': True, 'exit': None, 'stdout': '51.'}):
            with self.subTest(response=response), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, response)
                adapter.profile.update(immutable_hashes={}, boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
                adapter.transport = lambda arguments, script, timeout: calls.append(script) or response
                with self.assertRaises(ValueError): adapter.raw_snapshot('ready', 100, True)
                self.assertTrue(adapter.capture_failed)
                self.assertEqual(calls, ['set -eu\ncat /proc/uptime\n'])

    def test_snapshot_batch_is_claimed_before_outputs_and_other_preludes_unchanged(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {'timeout': False, 'exit': 0, 'stdout': json.dumps(self.raw_response())})
            adapter.profile.update(immutable_hashes={}, boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
            regular = adapter.prelude(100, None)
            snapshot = adapter.prelude(100, None, snapshot=True)
            for path in adapter.profile['encoder_hashes']:
                self.assertIn(f'sha256sum {path}', regular)
                self.assertNotIn(f'sha256sum {path}', snapshot)
                self.assertIn(f'test -f {path}', snapshot)
            for name in adapter.profile['tools']:
                self.assertIn(f'sha256sum "$root/{name}"', regular)
                self.assertNotIn(f'sha256sum "$root/{name}"', snapshot)
                self.assertIn(f'test -f "$root/{name}"', snapshot)
            adapter.raw_snapshot('ready', 100, True)
            script = calls[-1]
            self.assertLess(script.index('> "$root/host-ready-000.claim"'), script.index('.artifact-hashes"'))
            self.assertLess(script.index('initial_content_result='), script.index('--observe "$root/observe_input" 2'))
            self.assertLess(script.index('initial_metadata_result='), script.index('"$root/native_frame_acquire" "$pid"'))
            self.assertLess(script.index('.pins-after"'), script.index('closing_content_result='))
            self.assertLess(script.index('closing_metadata_result='), script.index('> "$root/host-ready-000.hashes"'))
            self.assertEqual((adapter.sidecar_prepare('host-ready-000') + adapter.sidecar_close('host-ready-000')).count('sha256sum'), 6)

    @unittest.skipUnless(os.name == 'posix' and shutil.which('sh'), 'POSIX shell controls')
    def test_executed_snapshot_batch_checks_all_nine_actual_rendered_pins(self):
        import test_owned_frame_capture as capture_tests
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            entries = [adapter.profile['encoder_hashes'][path] + '  ' + path for path in sorted(adapter.profile['encoder_hashes'])]
            entries += [adapter.profile['tools'][name] + '  $root/' + name for name in sorted(adapter.profile['tools'])]
            self.assertEqual(len(entries), 9)
            capture_tests.CaptureTests().exercise_pin_batch(entries, lambda: adapter.snapshot_artifact_checks('host-ready-000'), 'host-ready-000.artifact-hashes')

    @unittest.skipUnless(os.name == 'posix' and shutil.which('sh'), 'POSIX shell controls')
    def test_executed_sidecar_bracket_refuses_copy_hash_and_during_plane_changes(self):
        faults = ('equal', 'content-change', 'metadata-change', 'collision', 'copy-error', 'copy-wrong',
                  'initial-hash-error', 'copied-hash-error', 'closing-hash-error', 'bad-hash', 'bad-path',
                  'metadata-collision', 'metadata-copy-error', 'metadata-copy-wrong',
                  'metadata-initial-hash-error', 'metadata-copied-hash-error', 'metadata-closing-hash-error')
        for fault in faults:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as root:
                path = Path(root)
                doc = path / 'document'
                for ext in ('content', 'metadata'): (path / ('document.' + ext)).write_text(ext + ' bytes')
                if fault == 'collision': (path / 'host-ready-000.content').write_text('prior evidence')
                if fault == 'metadata-collision': (path / 'host-ready-000.metadata').write_text('prior evidence')
                adapter, calls = self.adapter(root, {})
                prelude = '''set -eu
umask 077
phase=initial
private() { test -f "$1" && test ! -L "$1"; test "$(stat -c '%a %h' "$1")" = '600 1'; }
sha256sum() {
 case "$fault:$phase:$1" in
  initial-hash-error:initial:*document.content|copied-hash-error:initial:*host-ready-000.content|closing-hash-error:closing:*document.content|metadata-initial-hash-error:initial:*document.metadata|metadata-copied-hash-error:initial:*host-ready-000.metadata|metadata-closing-hash-error:closing:*document.metadata) return 2;;
  bad-hash:initial:*document.content) printf 'invalid  %s\\n' "$1"; return;;
  bad-path:initial:*document.content) printf '%s  /wrong/path\\n' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; return;;
 esac
 command sha256sum "$@"
}
cat() {
 case "$fault:$1" in
  copy-error:*document.content|metadata-copy-error:*document.metadata) return 2;;
  copy-wrong:*document.content|metadata-copy-wrong:*document.metadata) printf corrupted; return;;
 esac
 command cat "$@"
}
'''
                middle = 'phase=closing\nprintf plane > "$root/plane"\n'
                if fault == 'content-change': middle += 'printf changed > "$doc.content"\n'
                if fault == 'metadata-change': middle += 'printf changed > "$doc.metadata"\n'
                script = prelude + adapter.sidecar_prepare('host-ready-000') + middle + adapter.sidecar_close('host-ready-000') + 'printf accepted > "$root/accepted"\n'
                result = subprocess.run(['sh', '-c', script], env=dict(os.environ, root=root, doc=str(doc), fault=fault), capture_output=True)
                self.assertEqual(result.returncode == 0, fault == 'equal', result.stderr)
                self.assertEqual((path / 'accepted').exists(), fault == 'equal')
                if fault in ('content-change', 'metadata-change', 'closing-hash-error', 'metadata-closing-hash-error'):
                    self.assertTrue((path / 'plane').exists())
                elif fault != 'equal':
                    self.assertFalse((path / 'plane').exists())

    def test_delayed_complete_response_and_decode_cannot_escape_local_deadline(self):
        for at in ('response', 'decode'):
            with self.subTest(at=at), tempfile.TemporaryDirectory() as root:
                adapter, calls = self.adapter(root, {'timeout': False, 'exit': 0, 'stdout': json.dumps(self.raw_response())})
                adapter.profile.update(immutable_hashes={}, boot_id='02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
                with patch.object(adapter, 'snapshot_timeout', return_value=(5, 10)), patch('open_document_transport.time.monotonic', side_effect=[10] if at == 'response' else [9, 10]):
                    with self.assertRaises(Refused): adapter.raw_snapshot('ready', 100, True)
                self.assertTrue(adapter.capture_failed)
                self.assertFalse(adapter.tap_attempted)

    def test_partial_diagnostic_read_waits_without_replaying_action(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            observations = iter(['{"creation_trial":', '{"creation_trial":{"returned":true}}'])
            adapter.call = lambda script, timeout: next(observations)
            captures = []
            adapter.raw_snapshot = lambda stage, cutoff, idle: captures.append((stage, cutoff, idle)) or {"captured": True}
            with patch("open_document_transport.time.sleep"):
                result = adapter.result_snapshot(("boot", 2, 3), 100)
            self.assertEqual(result, {"captured": True})
            self.assertEqual(captures, [("result", 100, False)])
            self.assertFalse(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)

    def test_persistent_partial_diagnostics_have_fixed_poll_limit(self):
        with tempfile.TemporaryDirectory() as root:
            adapter, calls = self.adapter(root, {})
            observations = []
            adapter.call = lambda script, timeout: observations.append(script) or '{"partial":'
            with patch("open_document_transport.time.sleep"), self.assertRaises(Refused):
                adapter.result_snapshot(("boot", 2, 3), 100)
            self.assertEqual(len(observations), 30)
            self.assertFalse(adapter.tap_attempted)
            self.assertFalse(adapter.arm_attempted)


if __name__ == "__main__":
    unittest.main()
