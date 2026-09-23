## ADDED Requirements
### Requirement: Equivalent direct capture pixels
Screenshot normalization SHALL preserve exact decoded pixels, dimensions, orientation, luminance and nearest-neighbor overview sampling for every implemented format while avoiding unnecessary PNG encode/decode work. Status observation MAY consume a fresh image directly without serializing native/overviewPNG. Discovery ambiguity, invalid-length and owner/session checks SHALL remain enforced, and frames SHALL NOT be reused across mutation boundaries. Source: REM9; src/device/screenshot.rs and backend.rs.

#### Scenario: Direct status image
- **WHEN** fresh status pixels replace the prior serialization path
- **THEN** exact normalized pixels match the prior codec pipeline and identity is checked around that observation before any dependent input.

#### Scenario: Format equivalence
- **WHEN** modern RM2BGRA, legacy RM2 or PaperProRGBA is processed
- **THEN** exact legacy conversion/rotation/flip, colored luminance and alpha/nearest sampling are preserved; software equivalence does not claim native PaperPro timing validation.

### Requirement: Preserve the selected drawing tool
Automatic drawing SHALL preserve the user's selected pen, slot, color and width.
It SHALL NOT simulate menu presses to select or inspect drawing tools. A future
selection mechanism requires evidence of a robust supported direct interface.
Optional status and failure marks SHALL be suppressed before input when fresh
non-mutating observations cannot establish a visible, bounded, safely erasable
current tool. Saved preferences alone SHALL NOT establish actual tool state.
Normal Q&A SHALL remain available when optional feedback is suppressed.

#### Scenario: Supported current pen
- **WHEN** the current pen is positively recognized and its full supported width range fits the verified blank footprint and cleanup envelope
- **THEN** marks use that pen and slot with zero tool-selection presses; ownership, journal, neighbor-ink and cleanup checks remain enforced.

#### Scenario: Unknown or unsuitable current tool
- **WHEN** the toolbar is unknown, the tool is destructive, or visibility/maximum footprint/cleanup coverage is unproven
- **THEN** optional ink is suppressed without opening menus or changing settings; Q&A continues and the missing visual feedback is reported honestly.

#### Scenario: Unsafe cleanup or external change
- **WHEN** owner, content, input activity, controls or cleanup postconditions change after owned ink begins
- **THEN** further mutation fails closed and recovery evidence is retained; the implementation never broadens erasure to hide the failure.

#### Scenario: Uncalibrated notes toolbar
- **WHEN** a notes/blank page toolbar or other layout differs from the calibrated PDF annotation toolbar
- **THEN** optional ink is suppressed before journal creation or drawing, without changing tools or blocking core Q&A; the reduced feedback coverage is explicit.

#### Scenario: Trigger release does not navigate menus
- **WHEN** a Reader trigger qualifies
- **THEN** the workflow must not navigate menus; the proposed removal of the automatic bottom-center tap requires native trigger/overlay verification. A necessary verified non-menu tap remains allowed, and subsequent operations retain their ownership checks.

#### Scenario: Known trigger overlay needs an outside dismissal
- **WHEN** a released Reader trigger leaves the positively qualified overflow panel visible
- **THEN** one guarded tap outside every menu item may dismiss it, followed by fresh unchanged-owner/tool/native-content and panel-absence verification; no menu item is selected.

#### Scenario: Missing or unknown trigger overlay
- **WHEN** the trigger overlay is positively absent or its layout cannot be qualified
- **THEN** absence emits no dismissal input, while unknown layout fails closed; no guessed or repeated tap occurs.

#### Scenario: Discovery candidate vanishes during trigger observation
- **WHEN** a trigger observation fails with the typed verified-unmapped discovery-header EIO
- **THEN** at most one complete fresh read-only retry is allowed within500ms and the overall5s dismissal deadline, with the same retained input observer and unchanged pinned owner/session/native bytes; all other errors or guard changes refuse, and no tap is repeated.
