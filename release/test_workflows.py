"""Guard the actual required-job/build-gate wiring without external test packages."""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WorkflowTests(unittest.TestCase):
    def test_required_ci_jobs_finish_and_gate_every_application_step(self):
        source = (ROOT / ".github/workflows/ci.yml").read_text()
        for job, name in [("check", "Check Build & Lint"), ("test", "Test")]:
            block = re.search(rf"^  {job}:\n(.*?)(?=^  \w+:|\Z)", source, re.M | re.S).group(1)
            self.assertIn(f"name: {name}", block)
            self.assertIn("if: always()", block)
            self.assertIn('test "$POLICY_RESULT" = success', block)
            steps = block.split("\n      - ")[1:]
            for step in steps[1:]:
                self.assertIn("if: github.event_name == 'pull_request' && needs.policy.outputs.application == 'true'", step)

    def test_docs_never_enter_release_lock_and_tag_events_are_not_required(self):
        source = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertNotIn("\nconcurrency:", source)
        publish = source.split("  publish:\n", 1)[1]
        self.assertIn("if: needs.policy.outputs.application == 'true'", publish)
        self.assertIn("    concurrency:\n", publish)
        self.assertIn("cancel-in-progress: false", publish)
        self.assertIn("python release/coordinator.py publish", publish)
        self.assertNotIn("RELEASE_TOKEN", source)
        self.assertNotIn("MagDrago", source)
        self.assertNotIn("tags:", source)


if __name__ == "__main__":
    unittest.main()
