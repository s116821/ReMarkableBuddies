# Selected finite development facts timing

Source recipe only; no native binary, literal packet or device action selected here.
Main/parent explicitly selected this opt-in profile for unattended Main-controlled
manual fixture opening and capture. Physical user participation is not required.
`development-budget.ps1` is the fixed data-only profile for a later reviewed operator.
SDK config, waiting marker, request and private proof must all name
`main-dev-facts-120s` and setup=120000 with development opt-in true.

The entry setup clock starts at startup and refuses request admission at 120000 ms.
The actual facts read ends before acceptedRequestAt+5000 ms, including queued delay.
The live collector uses the existing host observation clock's origin before arming
(original operator line 106), with admission strictly before 150000 ms. Historical
final collection cannot change a live proof flag. The independently prearmed
singleton timer begins rollback at 180 s from timer arming before activation;
restoration TimeoutStartSec=240 s remains a distinct bound, not proof of restoration.

A later literal packet keeps the original one-shot launcher/stock-name policy and
singleton mandatory restore. Main verifies fresh waiting/current generation, opens
the one expected fixture through a deliberate existing development tool action,
captures/interprets the fresh page view, then invokes the fixed publisher once.
Any active page in the exact document/order is eligible. No effect automation,
page round trip, hot attach, OPEN guard, new supervisor, sleep or retry is provided.
The known installed helper hashes are not a source-qualified helper claim.

Reason/provenance: Main reported about 50.4 s from local readiness to source setup,
plus startup about 4 s and positive checks/capture/tool interpretation/publication
and uncertainty. The earlier 20/35/45 values were inherited development defaults,
not firmware or product requirements. This selected finite allowance is not a
new timing measurement or a hard syscall/failure guarantee. Ordinary product and
Reader/navigation timeouts remain unchanged. Preserve private fixture/baseline,
live process/empty-job evidence, callback limits, exact cleanup and stock receipt.
Source/artifact/packet review still precedes a separately selected device action.
