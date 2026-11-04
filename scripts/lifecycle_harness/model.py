"""E0 policy model. No device adapters, service commands or production activation."""
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Scope:
    boot: str
    generation: int
    nonce: str
    process: str


@dataclass
class Lifecycle:
    boot: str = "boot-1"
    state: str = "Stock"
    now: float = 0
    attempts: int = 0
    supervisor: bool = True
    guard: bool = False
    injected: bool = False
    config: bytes = b"stock\n"
    baseline: bytes = b"stock\n"
    unrelated: bytes = b"user-config\n"
    scope: Scope | None = None
    deadline: float = 0
    last_heartbeat: float = 0
    rollback_available: bool = True
    owned_files: set[str] = field(default_factory=set)
    events: list[str] = field(default_factory=list)

    def transition(self, state):
        self.state = state
        self.events.append(state)

    def request(self, compatible=True):
        if self.state == "Ready":
            return self.scope
        if not self.supervisor or self.state != "Stock" or self.attempts:
            return None
        if not compatible:
            self.transition("DisabledForSession")
            return None
        self.transition("Preflight")
        return True

    def arm(self, available=True):
        if self.state != "Preflight" or not self.supervisor:
            return False
        if not available:
            self.transition("DisabledForSession")
            return False
        self.guard = True
        self.transition("RecoveryArmed")
        return True

    def activate(self, nonce="challenge-1", process="owned-start-1"):
        if self.state != "RecoveryArmed" or not self.supervisor or not self.guard:
            return False
        self.attempts += 1
        self.scope = Scope(self.boot, self.attempts, nonce, process)
        self.owned_files.update({"session", "session.partial"})
        self.config = b"injected\n"
        self.injected = True
        self.deadline = self.now + 5
        self.transition("Activating")
        return True

    def ready(self, scope):
        if (self.state != "Activating" or scope != self.scope or not self.guard
                or not self.supervisor or self.now >= self.deadline):
            return False
        self.last_heartbeat = self.now
        self.transition("Ready")
        return True

    def heartbeat(self, scope):
        if self.state != "Ready" or scope != self.scope or not self.guard:
            return False
        self.last_heartbeat = self.now
        return True

    def fail(self):
        if self.injected or self.config != self.baseline or self.owned_files:
            if not self.guard and not self.supervisor:
                self.transition("FailedUnprotected")
                return
            self.deadline = self.now + 5
            self.transition("RestoringStock")
        else:
            self.guard = False
            self.transition("DisabledForSession")

    def finish_restore(self):
        if self.state != "RestoringStock" or not self.rollback_available:
            return False
        if not self.guard and not self.supervisor:
            self.transition("FailedUnprotected")
            return False
        self.config = self.baseline
        self.injected = False
        self.owned_files.clear()
        self.guard = False
        self.transition("DisabledForSession")
        return True

    def death(self, who):
        if who == "supervisor":
            self.supervisor = False
        elif who == "guard":
            self.guard = False
        elif who == "both":
            self.supervisor = False
            self.guard = False
        else:
            raise ValueError(who)
        self.fail()

    def advance(self, seconds):
        if seconds < 0:
            raise ValueError("monotonic clock cannot go backward")
        self.now += seconds
        if self.state == "Activating" and self.now >= self.deadline:
            self.fail()
        elif self.state == "Ready" and self.now - self.last_heartbeat >= 3:
            self.fail()
        elif self.state == "RestoringStock" and self.now >= self.deadline:
            self.transition("RecoveryFailed")

    def cold_boot(self):
        # Production mechanism is not implemented. Model runtime-only config reset.
        return Lifecycle(boot=self.boot + "-next", baseline=self.baseline,
                         config=self.baseline, unrelated=self.unrelated)
