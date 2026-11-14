"""Offline process/receipt controls; these do not substitute for old-library CI."""
from contextlib import contextmanager
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

from old_library_outer import await_closed_receipt, preserve_stdout


class ClosedReceiptTests(unittest.TestCase):
    @contextmanager
    def child(self, final=b'', exit_code=0):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            stderr = b'owned child stderr retained verbatim\n'
            program = (
                "import sys\n"
                "sys.stdout.buffer.write(b'ready\\n'); sys.stdout.buffer.flush()\n"
                "assert sys.stdin.buffer.readline() == b'release\\n'\n"
                f"sys.stderr.buffer.write({stderr!r}); sys.stderr.buffer.flush()\n"
                f"sys.stdout.buffer.write({final!r}); sys.stdout.buffer.flush()\n"
                f"sys.exit({exit_code})\n"
            )
            lines = queue.Queue()
            errors = []
            report = {}
            with (output / 'old-hold.stderr').open('wb') as child_log:
                child = subprocess.Popen([sys.executable, '-u', '-c', program], stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=child_log)
                reader = threading.Thread(target=preserve_stdout,
                                          args=(child.stdout, output / 'old-hold.stdout', lines, errors),
                                          daemon=True)
                reader.start()
                try:
                    self.assertEqual(lines.get(timeout=5), b'ready\n')
                    self.assertIsNone(child.poll(), 'child must actually wait for explicit release')
                    child.stdin.write(b'release\n')
                    child.stdin.flush()
                    yield child, reader, lines, errors, output, report
                finally:
                    if child.poll() is None:
                        child.kill()
                    child.wait(timeout=5)
                    reader.join(timeout=5)
                    child.stdin.close()
                    child.stdout.close()
            self.assertFalse(reader.is_alive())
            self.assertEqual(errors, [])
            self.assertEqual((output / 'old-hold.stdout').read_bytes(), b'ready\n' + final)
            self.assertEqual((output / 'old-hold.stderr').read_bytes(), stderr)

    def test_closed_receipt_follows_explicit_release_and_zero_exit(self):
        receipt = b'{"action":"released","closed":true}\n'
        with self.child(receipt) as arguments:
            await_closed_receipt(*arguments)
            report = arguments[-1]
            self.assertEqual(report['owned_child_exit'], 0)
            self.assertEqual(report['old_release']['status'], 'closed')
            self.assertEqual((arguments[-2] / 'old-release.stdout').read_bytes(), receipt)

    def test_nonzero_exit_is_reported_before_missing_receipt_is_consumed(self):
        with self.child(b'partial stdout without newline', exit_code=1) as arguments:
            with patch.object(arguments[2], 'get_nowait', side_effect=AssertionError('must check exit first')):
                with self.assertRaisesRegex(RuntimeError, 'exited 1 before closed receipt'):
                    await_closed_receipt(*arguments)
            self.assertEqual(arguments[-1]['owned_child_exit'], 1)
            self.assertEqual(arguments[-1]['old_release'],
                             {'exit': 1, 'status': 'failed', 'reason': 'nonzero-child-exit'})
            self.assertFalse((arguments[-2] / 'old-release.stdout').exists())

    def test_nonzero_exit_cannot_be_overridden_by_a_closed_receipt(self):
        with self.child(b'{"action":"released","closed":true}\n', exit_code=1) as arguments:
            with self.assertRaisesRegex(RuntimeError, 'exited 1 before closed receipt'):
                await_closed_receipt(*arguments)
            self.assertEqual(arguments[2].qsize(), 1, 'failed child receipt must not be consumed')

    def test_zero_exit_without_receipt_is_an_explicit_protocol_failure(self):
        with self.child() as arguments:
            with self.assertRaisesRegex(RuntimeError, 'successfully without a closed receipt'):
                await_closed_receipt(*arguments)
            self.assertEqual(arguments[-1]['owned_child_exit'], 0)
            self.assertEqual(arguments[-1]['old_release']['reason'], 'missing-closed-receipt')

    def test_malformed_or_unclosed_receipts_cannot_prove_closure(self):
        for receipt in [b'{bad json}\n', b'{"action":"released","closed":false}\n',
                        b'{"action":"still-held","closed":true}\n', b'{"action":"released","closed":"yes"}\n']:
            with self.subTest(receipt=receipt), self.child(receipt) as arguments:
                with self.assertRaises(RuntimeError):
                    await_closed_receipt(*arguments)
                self.assertEqual(arguments[-1]['old_release']['reason'], 'invalid-closed-receipt')
                self.assertEqual((arguments[-2] / 'old-release.stdout').read_bytes(), receipt)


if __name__ == '__main__':
    unittest.main()
