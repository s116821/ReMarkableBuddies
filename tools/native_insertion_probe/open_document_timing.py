"""Source-only proposed timing contract; NOT wired into an actor or device entry.

All times use the same integer monotonic clock as the existing actor. Stage
limits are fixed from candidate start, never refreshed by signals or tokens.
The owning proposal is SDK 2cbe204; hardware feasibility remains unqualified.
"""
from dataclasses import dataclass

ABSOLUTE_BUDGET = 360
RECOVERY_RESERVE = 185
OBSERVATION_BUDGET = 70
STAGE_OFFSETS = {"ready": 20, "open": 35, "trigger": 40,
                 "result": 55, "stop": 60}


@dataclass(frozen=True)
class OpenDocumentTiming:
    armed_at: int
    deadline: int
    candidate_at: int

    def __post_init__(self):
        values = (self.armed_at, self.deadline, self.candidate_at)
        if any(type(v) is not int or v < 0 for v in values):
            raise ValueError("Canonical nonnegative monotonic seconds required")
        if self.deadline - self.armed_at != ABSOLUTE_BUDGET:
            raise ValueError("Original absolute 360-second deadline required")
        if self.candidate_at < self.armed_at:
            raise ValueError("Candidate cannot precede independent actor arming")
        if self.deadline - self.candidate_at < OBSERVATION_BUDGET + RECOVERY_RESERVE:
            raise ValueError("Insufficient candidate observation and recovery reserve")

    @property
    def observation_end(self):
        return min(self.candidate_at + OBSERVATION_BUDGET,
                   self.deadline - RECOVERY_RESERVE)

    def stage_end(self, stage):
        return min(self.candidate_at + STAGE_OFFSETS[stage], self.observation_end)

    def admit(self, stage, now):
        """Timing prerequisite only; token/context/input checks remain mandatory."""
        if type(now) is not int or now < self.candidate_at:
            return False
        if now >= self.stage_end(stage):
            return False
        if stage == "trigger" and self.deadline - now < RECOVERY_RESERVE + 30:
            return False
        return True

    def admit_creation(self, now, sdk_gate_end):
        """Intersect host timing with a verified SDK monotonic gate expiry.

        Unknown expiry refuses. The future host must read this value from a
        candidate-bound SDK receipt; waiting-file mtime is not that authority.
        This check does not substitute for identity/context/token validation.
        """
        return (type(sdk_gate_end) is int and sdk_gate_end > self.candidate_at
                and type(now) is int and now < sdk_gate_end
                and self.admit("trigger", now))
