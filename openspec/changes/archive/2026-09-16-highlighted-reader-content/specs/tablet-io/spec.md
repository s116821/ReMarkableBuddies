## MODIFIED Requirements

### Requirement: Screenshot layout and ambiguity handling
RM2 SHALL select four-byte BGRA for IMG_VERSION major/minor at least 3.24 and the legacy two-byte layout otherwise. Missing or malformed RM2 version fields SHALL error. Capture SHALL read xochitl process memory and reject missing or ambiguous modern RM2 allocations. Source: src/device/screenshot.rs rm2_uses_bgra/find_rm2_bgra_allocation/take_screenshot.

#### Scenario: Modern RM2 allocation
- **WHEN** modern RM2 capture searches anonymous writable mappings
- **THEN** it searches page-aligned addresses where the complete allocation fits, requires exactly one matching 32-bit mmap allocation header for 1404 by 1872 by four bytes, and reads pixels after the eight-byte header.

#### Scenario: Merged memory mappings after restart
- **WHEN** a valid framebuffer allocation is inside a merged anonymous mapping rather than at its start
- **THEN** discovery finds its validated header without a fixed address or historical-offset fallback.
- **AND** missing, malformed, truncated and multiple matching allocations are not accepted as a unique framebuffer.

#### Scenario: Native detail and overview
- **WHEN** modern RM2 pixels are encoded
- **THEN** neutral-preserving luminance from the BGR channels supplies full-range grayscale in portrait 1404 by 1872 without legacy rotation, retaining text contrast in colored highlights.
- **AND** the API provides a 768 by 1024 overview plus three overlapping full-width native-detail strips.

#### Scenario: Legacy and Paper Pro branches
- **WHEN** another implemented capture branch is selected
- **THEN** legacy RM2 uses its existing conversion/rotation/flip and Paper Pro uses its existing four-byte 1632 by 2154 path.
- **AND** presence of these paths does not establish hardware compatibility for every firmware.


#### Scenario: Colored highlighter pixels
- **WHEN** modern RM2 native pixels contain a yellow highlight over printed text
- **THEN** grayscale conversion uses (77R+150G+29B+128)>>8, preserving neutral values exactly and keeping yellow background lighter than black text.
