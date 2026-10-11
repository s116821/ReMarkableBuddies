"""Source-only proposed timing contract; NOT wired into an actor or device entry.

All times use the same integer monotonic clock as the existing actor. Stage
limits are fixed from candidate start, never refreshed by signals or tokens.
The owning proposal is SDK 2cbe204; hardware feasibility remains unqualified.
"""
from dataclasses import dataclass
import re

ABSOLUTE_BUDGET = 360
RECOVERY_RESERVE = 185
OBSERVATION_BUDGET = 70
STAGE_OFFSETS = {"ready": 20, "open": 35, "trigger": 40,
                 "result": 55, "stop": 60}


def diagnostic_cutoff(timing, waiting_expiry_ms):
    if type(waiting_expiry_ms) is not int or waiting_expiry_ms <= timing.candidate_at * 1000:
        raise ValueError('Original SDK waiting expiry required')
    return min((timing.candidate_at + 40) * 1000, waiting_expiry_ms,
               timing.observation_end * 1000, (timing.deadline - RECOVERY_RESERVE) * 1000) // 1000


def upper_uptime_ms(text):
    """Conservative BOOTTIME observation from bounded /proc/uptime bytes.

    Linux exposes truncated decimal uptime. Add one displayed unit so the
    returned integer never understates the observed time. No floating point.
    This is an observation timestamp; transport delay must be checked afresh
    remotely before publication, not assumed to be covered by rounding.
    """
    if not isinstance(text, str) or len(text) > 64:
        raise ValueError("Bounded uptime record required")
    m = re.fullmatch(r"([0-9]{1,12})\.([0-9]{1,3}) [0-9]{1,12}\.[0-9]{1,3}\n", text)
    if not m:
        raise ValueError("Canonical decimal uptime record required")
    scale = 10 ** (3 - len(m[2]))
    return int(m[1]) * 1000 + int(m[2]) * scale + scale


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

    def admit_creation_ms(self, upper_now_ms, sdk_gate_end_ms):
        """Fail closed across milliseconds and the actor's integer seconds."""
        if (type(upper_now_ms) is not int or type(sdk_gate_end_ms) is not int
                or upper_now_ms < self.candidate_at * 1000
                or sdk_gate_end_ms <= self.candidate_at * 1000):
            return False
        # Round current time upwards, never the SDK deadline upwards. The
        # independent actor's seconds cutoffs may refuse up to one second early.
        return (upper_now_ms < sdk_gate_end_ms
                and self.admit("trigger", (upper_now_ms + 999) // 1000))
