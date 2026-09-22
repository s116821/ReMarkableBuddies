## ADDED Requirements
### Requirement: Equivalent direct capture pixels
Screenshot normalization SHALL preserve exact decoded pixels, dimensions, orientation, luminance and nearest-neighbor overview sampling for every implemented format while avoiding unnecessary PNG encode/decode work. Status observation MAY consume a fresh image directly without serializing native/overviewPNG. Discovery ambiguity, invalid-length and owner/session checks SHALL remain enforced, and frames SHALL NOT be reused across mutation boundaries. Source: REM9; src/device/screenshot.rs and backend.rs.

#### Scenario: Direct status image
- **WHEN** fresh status pixels replace the prior serialization path
- **THEN** exact normalized pixels match the prior codec pipeline and identity is checked around that observation before any dependent input.

#### Scenario: Format equivalence
- **WHEN** modern RM2BGRA, legacy RM2 or PaperProRGBA is processed
- **THEN** exact legacy conversion/rotation/flip, colored luminance and alpha/nearest sampling are preserved; software equivalence does not claim native PaperPro timing validation.
