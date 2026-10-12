"""Closed source5 phase policy; no native calls, retries or independent clocks."""
import hashlib
import json
import multiprocessing
import time
import tempfile
import pickle
from pathlib import Path

MAX_FRAMES = 10
MAX_BYTES = 192 * 1024 * 1024
# Conservative debit includes native frame, both PNGs/duplicate overview, maps,
# capped stderr/sidecars, base64 response, transport and host records.
FRAME_BYTES = 25 * 1024 * 1024
CONTROL_BYTES = 17 * 1024 * 1024
FUTURE_CONTROL_BYTES = 1024 * 1024
POST_STAGES = frozenset({"result", "normalization-source", "inserted-destination"})

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),ensure_ascii=False).encode()

def page_map_sha256(content):
    if type(content.get("pageCount")) is not int or content["pageCount"]!=9:
        raise ValueError("Exact nine-page map required")
    pages=[]
    for p in content["cPages"]["pages"]:
        pages.append({"id":p.get("id"),"idx":p.get("idx"),"deleted":p.get("deleted"),
            "redir_present":"redir" in p,"redir":p.get("redir")})
    pages.sort(key=lambda p:json.dumps(p["id"],ensure_ascii=False))
    return hashlib.sha256(canonical({"pageCount":9,"pages":pages})).hexdigest()

class ControlLedger:
    """One shared serialized-writer quota; failed/used claims are never refunded."""
    def __init__(self):
        self.used=0
        self.terminal_reserve=65536
        self.overflow=False
        self._record_claims={}
        self.remote_reserved=0
    def reserve_remote(self,size):
        # Same total quota: remote outputs are not a second outside budget.
        self.claim(size)
        self.remote_reserved+=size
    def reserve_record(self,size,terminal=False):
        self.claim(size,terminal=terminal)
        ticket=object()
        self._record_claims[ticket]=size
        return ticket
    def reserve_records(self,sizes):
        if type(sizes) not in {list,tuple} or any(type(n) is not int or n<0 for n in sizes):
            raise ValueError("Typed closed record reservation required")
        self.claim(sum(sizes))
        tickets=[]
        for size in sizes:
            ticket=object()
            self._record_claims[ticket]=size
            tickets.append(ticket)
        return tickets
    def consume_record(self,ticket):
        try: return self._record_claims.pop(ticket)
        except (KeyError,TypeError) as error: raise ValueError("Unknown/reused control reservation") from error
    @property
    def remaining(self):
        return FUTURE_CONTROL_BYTES-self.used
    def claim(self,size,terminal=False):
        if type(size) is not int or size<0:
            raise ValueError("Typed control size required")
        limit=FUTURE_CONTROL_BYTES if terminal else FUTURE_CONTROL_BYTES-self.terminal_reserve
        if self.used+size>limit:
            self.overflow=True
            raise ValueError("Control evidence budget exhausted; no qualification")
        self.used+=size

class FrameLedger:
    def __init__(self):
        self.frames = 0
        self.bytes = CONTROL_BYTES
        self.reserved = 0
        self.arm_reserved = False

    def claim(self, reserved=False):
        if reserved:
            if self.reserved < 1:
                raise ValueError("No reserved postcreation capture")
            self.reserved -= 1
        elif self.arm_reserved:
            raise ValueError("Prearm capture after reservation")
        protected=self.reserved if self.arm_reserved else 3
        if self.frames + protected + 1 > MAX_FRAMES or self.bytes + (protected + 1) * FRAME_BYTES > MAX_BYTES:
            raise ValueError("Shared frame/byte budget exhausted")
        self.frames += 1
        self.bytes += FRAME_BYTES  # Failed and used claims never refunded.

    def reserve_creation(self):
        if self.arm_reserved or self.frames > 7 or self.frames + 3 > MAX_FRAMES or self.bytes + 3 * FRAME_BYTES > MAX_BYTES:
            raise ValueError("Three result/navigation captures unavailable before publication")
        self.reserved = 3
        self.arm_reserved = True

    def forecast_source(self, count):
        # Read-only minimum admission; claims remain exclusive and sticky.
        if type(count) is not int or self.arm_reserved or count not in {2, 3} or self.frames + count + 3 > MAX_FRAMES or self.bytes + (count + 3) * FRAME_BYTES > MAX_BYTES:
            raise ValueError("Required source and postcreation captures unavailable before input")

    def terminal(self):
        self.reserved = 0  # Only genuinely unused reserved slots.

class PhaseBudget:
    def __init__(self, result_deadline, both=False):
        now = time.monotonic()
        if result_deadline - now < (41 if both else 21):
            raise ValueError("Original RESULT allowance cannot fit selected branch")
        self.result_deadline = result_deadline
        self.end = min(now + 20, result_deadline - 1)
        self.staging_end = min(now + 3, self.end)
        self.capture_end = None
        self.processing_end = None

    def check(self, deadline):
        if time.monotonic() >= min(deadline, self.end, self.result_deadline):
            raise ValueError("Fixed source5 component exhausted; no renewed clock")

    def capture(self):
        if self.capture_end is not None:
            raise ValueError("Capture clock already sealed")
        self.check(self.end)
        self.capture_end = min(time.monotonic() + 10, self.end, self.result_deadline)
        return self.capture_end

    def arrived(self):
        if self.capture_end is None or self.processing_end is not None:
            raise ValueError("Processing clock requires one sealed arrival")
        self.check(self.capture_end)
        self.processing_end = min(time.monotonic() + 2, self.end, self.result_deadline)
        return self.processing_end

MAX_LOCAL_RESULT_BYTES = 2 * 1024 * 1024

def _work_file(path, fn, args, result_limit):
    try:
        result=(True,fn(*args))
    except BaseException as e:
        result=(False,repr(e)[:256])
    data=pickle.dumps(result,protocol=4)
    if len(data)>result_limit:
        raise SystemExit(2) # No result publication or unbounded traceback.
    with open(path,"xb") as stream:
        stream.write(data)
        stream.flush()
    # Process exit is the completion signal; no pipe/header/partial-message wait.

def bounded_local(fn, args, deadline, result_limit=MAX_LOCAL_RESULT_BYTES):
    """Tracked child with exclusive bounded result file and death-before-read."""
    if type(result_limit) is not int or not 1<=result_limit<=MAX_LOCAL_RESULT_BYTES:
        raise ValueError("Closed local result byte bound required")
    if time.monotonic() >= deadline:
        raise ValueError("No local processing allowance")
    ctx = multiprocessing.get_context("spawn")
    with tempfile.TemporaryDirectory(prefix="rmb-source5-") as directory:
        allocated=Path(directory).resolve()
        if allocated.parent!=Path(tempfile.gettempdir()).resolve() or not allocated.name.startswith("rmb-source5-"):
            raise ValueError("Task temporary path escaped its owned parent")
        path=str(allocated/"result")
        process=ctx.Process(target=_work_file,args=(path,fn,args,result_limit),daemon=True)
        process.start()
        try:
            while process.is_alive():
                remaining=deadline-time.monotonic()
                if remaining<=0:
                    raise ValueError("Local work exhausted fixed processing deadline")
                time.sleep(min(0.005,remaining))
            process.join(timeout=0)
            if time.monotonic()>=deadline or process.exitcode!=0:
                raise ValueError("Local worker failed or completed late")
            result_path=Path(path)
            size=result_path.stat().st_size
            if size>result_limit:
                raise ValueError("Local result exceeds fixed size")
            with result_path.open("rb") as stream:
                data=stream.read(result_limit+1)
            if len(data)!=size or time.monotonic()>=deadline:
                raise ValueError("Bounded result read changed or arrived late")
            try: ok,result=pickle.loads(data)
            except (pickle.UnpicklingError,EOFError,ValueError) as error: raise ValueError("Incomplete local output") from error
            if not ok: raise ValueError("Local work failed: "+str(result)[:256])
            if time.monotonic()>=deadline: raise ValueError("Local result decoding arrived late")
            return result
        finally:
            if process.is_alive(): process.kill()
            process.join(timeout=0.5)
            if process.is_alive():
                raise ValueError("Local worker death unconfirmed")

def native_positive(native, diagnostics):
    flags = {"enabled":True, "development_explicit_fixture":True,
        "mutation_attempted":True, "returned":True, "returned_bool_known":True,
        "returned_bool":True, "exception":False, "duplicate_callback":False,
        "durable_success":False, "effect_requires_reconciliation":True}
    if any(type(native.get(k)) is not bool or native[k] is not v for k,v in flags.items()):
        raise ValueError("Unknown or negative native creation outcome")
    if type(native.get("callback_count")) is not int or native["callback_count"] != 1 or native.get("phase") != "call-observed" or native.get("guard_stage") != "accepted":
        raise ValueError("Exact single callback required")
    if diagnostics.get("creation_after_open_gate") is not True or diagnostics.get("creation_setup_cutoff_ms") != 95000 or diagnostics.get("access_anchor") != "creation-arm-observation" or diagnostics.get("access_budget_ms") != 5000:
        raise ValueError("Original native setup/access contract missing")
    arm = diagnostics.get("creation_arm_accepted_at_ms")
    stamps = [native.get(k) for k in ("attempted_at_ms","returned_at_ms","first_callback_at_ms","cleanup_return_ms")]
    if type(arm) is not int or any(type(t) is not int or t < arm or t >= arm+5000 for t in stamps):
        raise ValueError("Probe-relative native access stamps unqualified")
    return hashlib.sha256(canonical(native)).hexdigest()

def blank_region(png):
    from open_document_host import pixels, BODY
    return pixels(png, BODY) == bytes([255]) * ((BODY[2]-BODY[0])*(BODY[3]-BODY[1])*3)

def pixel_classification(png, source, footer):
    from open_document_host import same_pixels, BODY, FOOTER
    return {"blank":blank_region(png), "source":same_pixels(png,source,BODY),
            "footer":same_pixels(png,footer,FOOTER)}
