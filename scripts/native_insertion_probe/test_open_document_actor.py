"""Exact-source shell windows, synthetic monotonic time; no device or systemd."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import coordinator
from test_actor import fixture


class ObservationWindow(unittest.TestCase):
    def test_diagnostic_window_keeps_original_70_seconds_and_no_input_recovery(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d)
            fixture.shell(root, 'armed_at=100; deadline=460; experiment=open-document-readiness-observation-v1; observation_window; test "$observe_end" = 275; recover_owned_input',
                          'now() { printf "205\\n"; }')
            self.assertEqual((root/'candidate.window').read_text(),'1'*32+' 100 460 205 275\n')

    def test_latest_start_preserves_185_seconds_and_never_renews(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fixture.shell(root, 'armed_at=100; deadline=460; experiment=open-document-insertion-v1; observation_window; test "$observe_end" = 275',
                          'now() { printf "205\\n"; }')
            self.assertEqual((root / "candidate.window").read_text(), "1" * 32 + " 100 460 205 275\n")
            # A second stage invocation cannot overwrite the bound window.
            fixture.shell(root, 'armed_at=100; deadline=460; experiment=open-document-insertion-v1; observation_window',
                          'now() { printf "205\\n"; }', expected=2)

    def test_late_candidate_refuses_before_window_publication(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fixture.shell(root, 'armed_at=100; deadline=460; experiment=open-document-insertion-v1; observation_window',
                          'now() { printf "206\\n"; }', expected=1)
            self.assertFalse((root / "candidate.window").exists())

    def test_legacy_window_retains_30_seconds_and_no_new_marker(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fixture.shell(root, 'armed_at=100; deadline=460; experiment=caller-selected-insertion-v1; observation_window; test "$observe_end" = 157',
                          'now() { printf "127\\n"; }')
            self.assertFalse((root / "candidate.window").exists())

    def test_new_mode_execution_refuses_before_transport_or_host_pin(self):
        argv = ["coordinator", "--packet", "unused", "--receipt-sha256", "a" * 64,
                "--known-hosts", "unused", "--known-hosts-sha256", "b" * 64,
                "--identity-file", "unused", "--output", "unused", "--execute-main"]
        with patch("sys.argv", argv), patch.object(coordinator, "load_packet", return_value={"experiment_kind": "open-document-insertion-v1"}), patch.object(coordinator, "transport") as transport, patch.object(coordinator, "pin_hosts") as pin:
            with self.assertRaisesRegex(ValueError, "execution refused"):
                coordinator.main()
            transport.assert_not_called()
            pin.assert_not_called()


if __name__ == "__main__":
    unittest.main()
