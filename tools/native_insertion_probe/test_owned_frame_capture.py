"""Capture acceptance faults and publication order; no device access."""
import hashlib
import unittest
from owned_frame_capture import CONTRACT, TOOLS, capture_script, validate_receipt


class CaptureTests(unittest.TestCase):
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
