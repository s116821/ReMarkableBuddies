import unittest
from open_document_protocol import parse_waiting
from open_document_timing import OpenDocumentTiming


class ProtocolTests(unittest.TestCase):
    nonce = "1" * 32

    def test_exact_candidate_binding_and_expiry_echo(self):
        w = parse_waiting((self.nonce + " 42 561 waiting 160000\n").encode(), self.nonce, 42, 561)
        self.assertEqual(w.arm_bytes(), (self.nonce + " 42 561 create 160000\n").encode())
        t = OpenDocumentTiming(100, 460, 127)
        self.assertTrue(t.admit_creation_ms(159999, w.expiry_ms))
        self.assertFalse(t.admit_creation_ms(160000, w.expiry_ms))

    def test_stale_process_start_nonce_and_malformed_tokens_refuse(self):
        good = (self.nonce + " 42 561 waiting 160000\n").encode()
        for data in (good.replace(b"42", b"43"), good.replace(b"561", b"562"),
                     good.replace(b"1" * 32, b"2" * 32), good[:-1],
                     good.replace(b"\n", b"\r\n"), good + b"\n",
                     good.replace(b"160000", b"0160000"),
                     good.replace(b"waiting", b"create"), good + b"x"):
            with self.subTest(data=data), self.assertRaises(ValueError):
                parse_waiting(data, self.nonce, 42, 561)

    def test_invalid_selected_identity_cannot_authorize_token(self):
        good = (self.nonce + " 42 561 waiting 160000\n").encode()
        for nonce, pid, start in (("0" * 32, 42, 561), (self.nonce, True, 561),
                                  (self.nonce, 1, 561), (self.nonce, 42, 0)):
            with self.subTest(identity=(nonce, pid, start)), self.assertRaises(ValueError):
                parse_waiting(good, nonce, pid, start)


if __name__ == "__main__":
    unittest.main()
