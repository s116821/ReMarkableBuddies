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
PRODUCTION_V2 = 'open-document-insertion-v2'
V2_STAGES = {"ready": 45, "open": 75, "trigger": 90, "result": 115, "stop": 125}
V2_CONTRACT = {"purpose": PRODUCTION_V2, "stages": V2_STAGES,
               "candidate_seconds": 130, "start_reserve_seconds": 315,
               "absolute_seconds": 360, "recovery_seconds": 185,
               "sdk_setup_seconds": 95, "accepted_seconds": 5,
               "publication_transport_seconds": 12,
               "library_exclusion_half_open": [735, 30, 737, 36]}
V2_SDK_REVISION = '39e386dd2927f771ccf5849b01edc60cffada49b'
V2_CONFIG_LINE = 'config.setupVariant = qml_access::CreationConfig::SetupVariant::AfterOpen95s;'
V2_LIBRARY_SHA256 = '9d4af329c95069fbf5845164b2b58c504a67c3aa30a9b85a91c226a6d23409aa'
V2_PUBLISHER_SHA256 = '9f2379c2312bafa5a8fd2a914d7df98d02319f12fff97fbd48929f03dbdbaae8'
V2_SDK_SOURCES = {
    'tools/qt_qml_access_probe.cpp': '69e2e0402c676f6f338eee5248de923520bb31cb561926976d37e8451c54fdaf',
    'tools/qt_qml_access_probe_core.h': '5b732ff7535a87a22880eefea867e4d4bc3671c029c910d18d1f6968be846460',
    'tools/qt_qml_creation_bridge.h': '7165b284430b13feba6b7b21d6303d77eb3288629e4b14d647034c1c35c6a88b',
    'tools/qt_qml_creation_bridge.cpp': '38a437c7865db590c215b2b632011cccfe7ab90cc5c5d1174afb1f88ad9ac81f',
    'tools/qt_page_open.h': '5058c60a39dd75db8cbde2cd2ca02113ecce2ce2860cf00572b40020b956eaf7',
    'tools/qt_page_open.cpp': '213ce54cc6985a25029a5cbc3342c58ce0d74dec9d363c2cba2cd540b70afc39',
    'tools/qt_page_owner.h': '992a9c8c5d627030c2286d6b168758c725957e150067db8160c5e560231f2399',
    'tools/qt_page_open_arm.h': '800daaad233995dc5f5c70db86d7192491aadd385b67e33479b05b8bf1535bbb',
    'tools/qt_creation_deadline.h': 'c3d8e7124eff6de5868c74f321189ce1f55f8b4111e8983ecdc7eb7c86b3a109'}

PRESERVED_EIGHT = 'preserved-eight-after-9005'
# The eight-page Library reference is pinned only after Main's fresh visual review.
EIGHT_LIBRARY_SHA256 = None
EIGHT_SDK_REVISION = '7e3cca6fa15491a32a01b71f7f209a1d0d5dfec9'
EIGHT_SDK_SOURCES = V2_SDK_SOURCES | {
    'tools/qt_qml_access_probe_core.h': '1971b00dd34242cbe03a7f49a45cfc9d075f4099e49ec18f0f2f8df99b0e6463'}
EIGHT_CONFIG_LINE = 'config.inventoryVariant = qml_access::CreationConfig::InventoryVariant::PreservedEightAfter9005;'
FIXTURE_DOCUMENT = 'e7f661f1-db6f-4dfc-854a-b38aff7f75de'
ORIGINAL_PAGES = (
    'a7181850-3f03-4bb0-8b9e-01acfc20032e', '5aef884d-d3fc-4635-895a-28237568823d',
    'd1261cb9-a8c0-4e15-ab24-7a9172b2027b', 'f39ae285-3e0c-43dd-b27c-866dff7a24cd',
    '028b118f-474a-4886-a469-7273b290320c', 'fd5afbe2-a79c-461b-8b15-96907d181ab7',
    '1692961d-ec5c-4f0f-9635-8e6081111dab')
PRESERVED_PAGES = ORIGINAL_PAGES[:1] + ('4c8a847e-0eb8-4cf3-a747-77952d56445a',) + ORIGINAL_PAGES[1:]


def fixture_inventory_selection(profile, page_field='page_ids'):
    variant = profile.get('fixture_inventory_variant', 'legacy')
    if type(variant) is not str or variant not in {'legacy', PRESERVED_EIGHT}:
        raise ValueError('Exact fixture inventory selector required')
    if variant == PRESERVED_EIGHT:
        if (profile.get('experiment_kind') != PRODUCTION_V2 or
            profile.get('document_id') != FIXTURE_DOCUMENT or
            profile.get(page_field) != list(PRESERVED_PAGES)):
            raise ValueError('Exact preserved eight-page fixture required')
        return variant, 8
    if profile.get('experiment_kind') == PRODUCTION_V2 and (
        profile.get('document_id') != FIXTURE_DOCUMENT or
        profile.get(page_field) != list(ORIGINAL_PAGES)
    ):
        raise ValueError('Exact original seven-page production fixture required')
    return variant, 7


def validate_v2_binding(profile):
    variant, _ = fixture_inventory_selection(profile, 'before_page_ids' if 'before_page_ids' in profile else 'page_ids')
    revision, sources = (EIGHT_SDK_REVISION, EIGHT_SDK_SOURCES) if variant == PRESERVED_EIGHT else (V2_SDK_REVISION, V2_SDK_SOURCES)
    if (profile.get('production_contract') != V2_CONTRACT or
        profile.get('sdk_revision') != revision or
        profile.get('sdk_source_hashes') != sources or
        profile.get('sdk_setup_variant') != 'AfterOpen95s' or
        not re.fullmatch('[0-9a-f]{64}', profile.get('sdk_configuration_sha256', ''))):
        raise ValueError('Exact reviewed production v2 SDK/configuration binding required')


def validate_v2_config(text, inventory_variant='legacy'):
    """Lexically exclude comments/literals in the separately reviewed config."""
    if not isinstance(text, str) or len(text.encode('utf-8')) > 65536:
        raise ValueError('Bounded reviewed creation configuration required')
    tokens = r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|\x27(?:\\.|[^\x27\\])*\x27'
    active = re.sub(tokens, lambda m: ''.join('\n' if c == '\n' else ' ' for c in m[0]), text)
    if (len(re.findall(r'\bconfig\s*\.\s*setupVariant\s*=', active)) != 1 or
        not re.search(r'^\s*' + re.escape(V2_CONFIG_LINE) + r'\s*$', active, re.MULTILINE)):
        raise ValueError('One unambiguous active reviewed SDK95 selector required')
    assignments = len(re.findall(r'\bconfig\s*\.\s*inventoryVariant\s*=', active))
    if inventory_variant == PRESERVED_EIGHT:
        if assignments != 1 or not re.search(r'^\s*' + re.escape(EIGHT_CONFIG_LINE) + r'\s*$', active, re.MULTILINE):
            raise ValueError('One active preserved-eight SDK inventory selector required')
    elif inventory_variant != 'legacy' or (assignments and (
            assignments != 1 or not re.search(r'^\s*config\.inventoryVariant = qml_access::CreationConfig::InventoryVariant::Legacy;\s*$', active, re.MULTILINE))):
        raise ValueError('Legacy SDK configuration must not select another inventory')


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
    purpose: str = 'open-document-insertion-v1'

    def __post_init__(self):
        values = (self.armed_at, self.deadline, self.candidate_at)
        if any(type(v) is not int or v < 0 for v in values):
            raise ValueError("Canonical nonnegative monotonic seconds required")
        if self.deadline - self.armed_at != ABSOLUTE_BUDGET:
            raise ValueError("Original absolute 360-second deadline required")
        if self.candidate_at < self.armed_at:
            raise ValueError("Candidate cannot precede independent actor arming")
        if self.purpose not in {'open-document-insertion-v1', 'open-document-readiness-observation-v1', PRODUCTION_V2}:
            raise ValueError('Unknown timing purpose')
        if self.deadline - self.candidate_at < self.candidate_seconds + RECOVERY_RESERVE:
            raise ValueError("Insufficient candidate observation and recovery reserve")

    @property
    def candidate_seconds(self):
        return 130 if self.purpose == PRODUCTION_V2 else OBSERVATION_BUDGET

    @property
    def observation_end(self):
        return min(self.candidate_at + self.candidate_seconds,
                   self.deadline - RECOVERY_RESERVE)

    def stage_end(self, stage):
        offsets = V2_STAGES if self.purpose == PRODUCTION_V2 else STAGE_OFFSETS
        return min(self.candidate_at + offsets[stage], self.observation_end)

    def admit(self, stage, now):
        """Timing prerequisite only; token/context/input checks remain mandatory."""
        if type(now) is not int or now < self.candidate_at:
            return False
        if now >= self.stage_end(stage):
            return False
        if self.purpose == PRODUCTION_V2:
            remaining = self.observation_end - now
            if self.deadline - now < remaining + RECOVERY_RESERVE:
                return False
            if stage == 'trigger' and (remaining < 40 or self.deadline - now < 225):
                return False
        elif stage == "trigger" and self.deadline - now < RECOVERY_RESERVE + 30:
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
