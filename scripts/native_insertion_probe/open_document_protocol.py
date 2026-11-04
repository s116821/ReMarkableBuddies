"""Pure creation permission framing. Does not publish tokens or operate a device."""
from dataclasses import dataclass
import re


@dataclass(frozen=True)
class Waiting:
    nonce: str
    pid: int
    start_ticks: int
    expiry_ms: int

    def arm_bytes(self):
        return f"{self.nonce} {self.pid} {self.start_ticks} create {self.expiry_ms}\n".encode("ascii")


def parse_waiting(data, nonce, pid, start_ticks):
    """Exact wire/identity check; filesystem ownership and freshness are separate."""
    if (not isinstance(nonce, str) or not re.fullmatch(r"[0-9a-f]{32}", nonce)
            or nonce == "0" * 32 or type(pid) is not int or pid <= 1
            or type(start_ticks) is not int or start_ticks <= 0):
        raise ValueError("Fresh selected nonce and canonical candidate identity required")
    if type(data) is not bytes or len(data) > 128:
        raise ValueError("Bounded literal permission bytes required")
    m = re.fullmatch(rb"([0-9a-f]{32}) ([1-9][0-9]{0,9}) ([1-9][0-9]{0,19}) waiting ([1-9][0-9]{0,15})\n", data)
    if not m:
        raise ValueError("Exact waiting protocol required")
    parsed = Waiting(m[1].decode("ascii"), int(m[2]), int(m[3]), int(m[4]))
    if (parsed.nonce, parsed.pid, parsed.start_ticks) != (nonce, pid, start_ticks):
        raise ValueError("Candidate permission identity mismatch")
    return parsed
