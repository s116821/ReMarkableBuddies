"""Capture acceptance faults and publication order; no device access."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from owned_frame_capture import CONTRACT, SOURCES, TOOLS, capture_script, validate_receipt


class CaptureTests(unittest.TestCase):
    def setUp(self):
        # Synthetic pins enable script testing without selecting an executable build.
        self.pins = [patch.dict(TOOLS, native_frame_acquire='a' * 64),
                     patch.dict(SOURCES, private_acquisition='b' * 64)]
        for pin in self.pins:
            pin.start()
            self.addCleanup(pin.stop)

    def test_unbound_scoped_producer_refuses_rendering(self):
        for target, key in ((TOOLS, 'native_frame_acquire'), (SOURCES, 'private_acquisition')):
            for invalid in (None, '', 'a' * 63, 'A' * 64):
                with patch.dict(target, {key: invalid}), self.assertRaises(ValueError):
                    capture_script('host-ready', '1' * 32, '02b2347b-03aa-4a85-8dd7-d731aabf6f3e')

    @unittest.skipUnless(os.name == 'posix' and shutil.which('sh'), 'POSIX shell controls')
    def test_actual_shell_admission_keeps_failure_gates_and_context_maps(self):
        script = capture_script('host-ready', '1' * 32, '02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
        admission = script[script.index('set +e'):script.index('test "$(now_upper)" -le $((cutoff-6))')]
        # Execute the rendered producer admission section, preserving its real
        # status/stderr/size gates. Native closure policy belongs to the producer.
        for status, stderr, size, admitted in ((0, '', 10513468, True),
                                             (2, '', 10513468, False),
                                             (0, 'diagnostic', 10513468, False),
                                             (0, '', 10513467, False)):
            with self.subTest(status=status, stderr=stderr, size=size), tempfile.TemporaryDirectory() as root:
                path = Path(root)
                (path / 'observed.maps').write_text('changed contextual map\n')
                (path / 'host-ready.maps-before').write_text('initial contextual map\n')
                producer = path / 'native_frame_acquire'
                producer.write_text(f'#!/bin/sh\nhead -c {size} /dev/zero\nprintf %s "{stderr}" >&2\nexit {status}\n')
                producer.chmod(0o700)
                selected = admission.replace('cat /proc/$pid/maps', 'cat "$root/observed.maps"')
                run = subprocess.run(['sh', '-c', 'set -e\nprivate() { test -f "$1"; }\n' + selected + '\nprintf admitted > "$root/admitted"\n'],
                                     env=dict(os.environ, root=root, pid='123', start='456'), capture_output=True)
                self.assertEqual(run.returncode == 0, admitted, run.stderr)
                self.assertEqual((path / 'admitted').exists(), admitted)
                if admitted:
                    self.assertNotEqual((path / 'host-ready.maps-before').read_bytes(),
                                        (path / 'host-ready.maps-after').read_bytes())

    def test_producer_failure_cannot_run_converter_and_closing_hash_precedes_conversion(self):
        s = capture_script('host-open-000', '1' * 32, '02b2347b-03aa-4a85-8dd7-d731aabf6f3e')
        self.assertLess(s.index('producer=$?'), s.index('frame_hash='))
        self.assertLess(s.index('test "$producer" = 0'), s.index('"$root/owned_frame_alarm"'))
        self.assertLess(s.index('frame_hash='), s.index('"$root/owned_frame_alarm"'))
        self.assertLess(s.index('test "$converter" = 0'), s.index('cp "$root/host-open-000.images/overview.png"'))
        self.assertNotIn('bounded ', s)
        self.assertNotIn(' | "$root/owned_frame"', s)
        self.assertEqual(s.count('"$root/native_frame_acquire"'), 1)
        self.assertEqual(s.count('"$root/owned_frame_alarm"'), 1)
        self.assertNotIn('/home/root/rem9-validation/screenshot', s)
        self.assertNotIn('cmp ', s)
        self.assertIn('maps-before', s)
        self.assertIn('maps-after', s)
        self.assertEqual(CONTRACT['kind'], 'native-owned-rgb32-scoped-v1')
        self.assertEqual(CONTRACT['wrapper_maps'], 'bounded-context-only')

    def test_receipt_mismatch_partial_and_false_native_authority_refuse(self):
        nonce, frame, png = '1' * 32, 'a' * 64, b'actual PNG bytes'
        receipt = f'nonce={nonce} frame_sha256={frame} native_sha256={"b" * 64} overview_sha256={hashlib.sha256(png).hexdigest()} conversion_complete=true native_authority=false\n'
        self.assertFalse(validate_receipt(receipt, nonce, frame, png, "b" * 64)['native_authority'])
        for changed in (receipt[:-1], receipt + 'extra\n', receipt.replace(nonce, '2' * 32), receipt.replace(frame, 'c' * 64), receipt.replace('b' * 64, 'd' * 64), receipt.replace('complete=true', 'complete=false'), receipt.replace('authority=false', 'authority=true')):
            with self.assertRaises(ValueError):
                validate_receipt(changed, nonce, frame, png, "b" * 64)
        with self.assertRaises(ValueError):
            validate_receipt(receipt, nonce, frame, png + b'changed', 'b' * 64)

    def test_unknown_prefix_nonce_and_boot_refuse_before_script(self):
        for prefix, nonce, boot in [('host-open-000; touch /tmp/a', '1' * 32, 'boot'), ('host-ready', 'short', 'boot'), ('host-ready', '1' * 32, 'boot')]:
            with self.assertRaises(ValueError):
                capture_script(prefix, nonce, boot)
        self.assertEqual(CONTRACT['frame_bytes'], 10513468)
        self.assertEqual(set(TOOLS), {'native_frame_acquire', 'owned_frame', 'owned_frame_alarm'})


if __name__ == '__main__':
    unittest.main()
