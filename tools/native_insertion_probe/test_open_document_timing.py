"""Pure timing cases only: no SSH, input, service, or native execution."""
import unittest
from open_document_timing import OpenDocumentTiming


class TimingTests(unittest.TestCase):
    def test_observed_setup_fits_without_extending_original_deadline(self):
        t = OpenDocumentTiming(100, 460, 127)  # 026 setup delta: 27 seconds
        self.assertTrue(t.admit("ready", 140))  # ~13-second observed readiness
        self.assertTrue(t.admit("open", 160))
        self.assertTrue(t.admit("trigger", 166))
        self.assertTrue(t.admit("result", 181))
        self.assertTrue(t.admit("stop", 186))
        self.assertEqual(t.observation_end, 197)
        self.assertEqual(t.deadline, 460)

    def test_latest_candidate_preserves_entire_recovery_reserve(self):
        t = OpenDocumentTiming(100, 460, 205)
        self.assertEqual(t.observation_end, 275)
        self.assertEqual(t.deadline - t.observation_end, 185)
        with self.assertRaises(ValueError):
            OpenDocumentTiming(100, 460, 206)

    def test_expired_stages_cannot_be_renewed_by_repeated_admission(self):
        t = OpenDocumentTiming(100, 460, 127)
        for stage in ("ready", "open", "trigger", "result", "stop"):
            end = t.stage_end(stage)
            self.assertTrue(t.admit(stage, end - 1))
            self.assertFalse(t.admit(stage, end))
            self.assertFalse(t.admit(stage, end + 1))
            self.assertEqual(t.stage_end(stage), end)

    def test_host_loss_cannot_change_fixed_observation_cutoff(self):
        t = OpenDocumentTiming(100, 460, 205)
        self.assertFalse(t.admit("trigger", 275))
        self.assertFalse(t.admit("stop", 459))
        self.assertEqual(t.observation_end, 275)

    def test_invalid_clock_or_changed_absolute_budget_refuses(self):
        for args in ((100, 461, 127), (100, 460, 99),
                     (True, 361, 27), (100, 460, 127.0), (-1, 359, 27)):
            with self.subTest(args=args), self.assertRaises(ValueError):
                OpenDocumentTiming(*args)
        t = OpenDocumentTiming(100, 460, 127)
        self.assertFalse(t.admit("open", 126))
        self.assertFalse(t.admit("trigger", True))

    def test_unknown_or_earlier_sdk_expiry_refuses_creation(self):
        t = OpenDocumentTiming(100, 460, 127)
        self.assertFalse(t.admit_creation(160, None))
        self.assertFalse(t.admit_creation(160, 160))
        self.assertFalse(t.admit_creation(160, True))
        self.assertTrue(t.admit_creation(159, 160))
        self.assertFalse(t.admit_creation(167, 200))  # host stage expired


if __name__ == "__main__":
    unittest.main()
