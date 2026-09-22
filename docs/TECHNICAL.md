# Technical Documentation

## Architecture Overview

ReMarkable Reader Buddy is a Rust application that provides AI-powered reading assistance on reMarkable tablets. It detects outlined content and handwritten questions, then displays answers on new pages.

### Project Structure

```
src/
├── main.rs              # Application entry point with CLI
├── lib.rs               # Library exports
├── device/              # Hardware interaction (from ghostwriter)
│   ├── mod.rs           # Device detection (RM2 vs Paper Pro)
│   ├── screenshot.rs    # Framebuffer capture
│   ├── pen.rs           # Drawing via evdev
│   ├── keyboard.rs      # Virtual keyboard input
│   └── touch.rs         # Touch event handling
├── llm/                 # LLM integration
│   ├── mod.rs           # LLM trait
│   └── openai.rs        # ChatGPT API client
├── analysis/            # Image analysis
│   ├── mod.rs           # Types (QuestionContext, BoundingBox)
│   ├── circle_detector.rs     # Outline detection (placeholder)
│   └── question_extractor.rs  # Question extraction (placeholder)
└── workflow/            # Orchestration
    ├── mod.rs           # Main workflow coordinator
    ├── orchestrator.rs  # High-level control flow
    ├── page_manager.rs  # Page navigation (TODO)
    └── renderer.rs      # Content rendering (placeholder)
```

## Current Workflow (v0.1)

1. User outlines content (any closed shape)
2. User writes question nearby
3. User touches lower-right corner
4. App captures screenshot (768x1024)
5. **Single LLM Call**: Detects outline + extracts question + generates answer + provides bounding boxes
6. **Erase question text** (preserves outline) if bounding box available
7. **Draw reference symbol** (①②③④⑤...) on original page where question was
8. **Create new blank page** after current via swipe gesture
9. **Render Q&A** on new page with matching symbol
10. **Navigate back** to original page to preserve reading context

### Why Single LLM Call?

**Design Decision**: One vision API call handles everything (detection + OCR + answering) because:
- **Simpler**: Less code, fewer edge cases
- **Faster**: Saves one API round-trip (5-10 seconds)
- **Cheaper**: One API call instead of two
- **Context**: LLM sees full context while answering
- **Effective**: GPT-4o vision handles all tasks well

**Note**: reMarkable has native handwriting conversion (MyScript engine), but it's not easily accessible from third-party apps. The LLM vision approach is more practical and works well.

## Key Technical Details

### Device Detection

Reads `/etc/hwrevision` to determine:
- reMarkable 2: `armv7`, 1872x1404, 16-bit grayscale
- Paper Pro: `aarch64`, 1632x2154, 32-bit RGBA

### Screenshot Capture

- Finds `xochitl` process PID
- Reads framebuffer from `/proc/{pid}/mem`
- Applies color correction and orientation
- Normalizes to 768x1024 virtual resolution

### Pen Drawing

- Uses `/dev/input/event1` (RM2) or `/dev/input/event2` (RMPP)
- Simulates pen events via evdev
- Supports lines, rectangles, and bitmap rendering

### Touch Detection

- Monitors `/dev/input/event2` (RM2) or `/dev/input/event3` (RMPP)
- 68x68 pixel trigger zones in corners
- Default: Lower-right (LR)

### Virtual Keyboard

- Creates uinput virtual device
- Maps characters to key events
- Supports Ctrl commands for text formatting

## Implemented Features (v0.1)

### ✅ Page Management System

**Implementation**: Uses touch gesture simulation
- Swipes left to navigate to next page
- If at last page, xochitl auto-creates new blank page
- Swipe right to navigate back
- Methods: `create_page_right()`, `next_page()`, `previous_page()`
- **File**: `src/workflow/page_manager.rs`

### ✅ Outline Preservation

**Implementation**: LLM returns separate bounding boxes
- `QUESTION_BOX`: Location of question text (erased)
- `SELECTION_CENTER`: Center of circled or highlighted content in the 768 x 1024 overview
- Only the question region is erased, outline remains visible
- **File**: `src/workflow/orchestrator.rs` (parse_bounding_box)

### ✅ Symbol Pool Implementation

**Implementation**: Persistent symbol cycling
- Pool of 10 symbols: ①②③④⑤⑥⑦⑧⑨⑩
- Cycles through pool across triggers
- State persists in `/home/root/.reader-buddy-symbol-state`
- Automatically loads on startup
- **File**: `src/workflow/symbol_pool.rs`

### ✅ Question Erasure

**Implementation**: Uses erase_region() with bounding box
- Erases only the question text region
- Outline shape is not affected (different bounding box)
- Falls back gracefully if no bounding box provided
- **File**: `src/workflow/orchestrator.rs` (render_answer)

### ✅ Dual Symbol Placement

**Implementation**: Symbol appears on both pages
- Drawn on original page at question location
- Rendered at start of Q&A on answer page
- Same symbol used for linking
- **File**: `src/workflow/orchestrator.rs` (render_answer)

## Remaining TODOs

### 🔧 Symbol Rendering Enhancement

**Current**: Simple geometric circle
**Goal**: Render actual ①②③④⑤ glyphs

**Options to explore**:
- Font-based rendering (check if system fonts have circled numbers)
- Pre-rendered bitmap glyphs
- SVG paths for each symbol

**File**: `src/workflow/symbol_pool.rs` (symbol_to_bitmap method)

### 🔧 Improved Bounding Box Accuracy

**Current**: LLM provides approximate boxes
**Goal**: More precise regions for better erasure

**Approaches**:
- Fine-tune LLM prompt
- Add local CV validation
- Use multiple iterations if needed

### 🔧 White Erasure Testing

**Current**: Uses pen drawing for erasure
**Goal**: Ensure it actually erases on device

**Testing Needed**:
- Verify erasure works with different pen colors
- Check if white fill is effective
- May need alternative approach (background color matching)

## Dependencies

Core crates:
- `evdev` - Input device simulation
- `image` + `imageproc` - Image processing
- `ureq` - HTTP client for OpenAI API
- `clap` - CLI parsing
- `serde_json` - JSON handling
- `base64` - Image encoding
- `anyhow` - Error handling
- `log` + `env_logger` - Logging

## Building

### Cross-Compilation

```bash
# Requires Docker and cross tool
./build.sh rm2    # armv7 for reMarkable 2
./build.sh rmpp   # aarch64 for Paper Pro
```

### Local Testing

Default execution requires reMarkable hardware. The maintained simulator runs the
same Reader orchestrator through `DeviceBackend` and `LLMEngine` interfaces:
`cargo run -- --simulate docs/simulator/scenarios/blank-answer.json`.
See [scenario format and fidelity](simulator.md). Desktop behavioral and simulator
tests run with `cargo test --all-targets`. For bounded device checks, use:
- `--screenshot-only FILE` to capture without credentials or input initialization
- `--once --no-trigger` to run one real-device question without a held gesture
- `READER_BUDDY_DEBUG_DUMP=true` to retain optional local capture diagnostics

The former `--input-png`, `--no-draw` and `--save-screenshot` switches are removed;
they did not provide a working offline simulator.

## CI/CD Integration

Uses pinned **git-cliff** for semantic versions and **vergen-gitcl** for runtime metadata:

**Version Bump Rules**:
1. **Major**: Scoped `!` syntax or `BREAKING CHANGE:` footer, including 0.x to 1.0.
2. **Minor**: Scoped `feat` squash commit, including 0.x.
3. **Patch**: Scoped fix/perf/refactor/build/ci/chore/test/revert application changes.
4. **None**: Only explicit documentation paths changed; no main application compilation.

**Path policy**: `release/cliff.toml` excludes explicit documentation paths; other
paths are relevant, including build/dependency changes. Misclassified application
messages fail visibly. Mixed and multi-commit history is evaluated in full.

**Workflows**:
- `.github/workflows/ci.yml` - Required checks with docs-only application build gates.
- `.github/workflows/release.yml` - Serialized immutable tag, exact-source builds,
  runtime/checksum verification and recoverable draft publication. No version commit.
- `build.rs` - Tag-derived CLI version; strict official source checks, explicit dev fallback.
- [Release tests and recovery](../release/README.md) - Public, isolated fixtures and retry commands.

## Testing Strategy

### Current Testing
- Behavioral tests cover configuration, hold timing, page decisions, Q&A composition
  and response parsing without a tablet.
- Real-device smoke remains required for input, model, navigation and rendering changes.
- Inspect service logs with `journalctl -u reader-buddy.service`; use `RUST_LOG=debug`
  in the protected service environment file for more verbose diagnostics.

### Simulator extensions
- REM-23 must add production Writer and combined-mode simulator regressions.
- REM-25 must extend the backend/scenarios for native insertion and preservation.
- REM-17 must cover final gesture arbitration and combined workflows before 1.0.

## Performance Considerations

- LLM vision calls: 5-10 seconds latency
- Screenshot capture: ~100ms
- Pen drawing: Depends on complexity
- Future: Local CV could reduce LLM calls

## Known Limitations (v0.1)

1. Single outline-question pair per trigger
2. No page creation (renders to current page)
3. Simple geometric symbols (not ①②③ yet)
4. No symbol cycling/tracking
5. No context retention between triggers
6. LLM-based detection (expensive, slow)
7. Requires internet connection

## Extension Points

### Adding LLM Providers

Implement `LLMEngine` trait in `src/llm/`:
```rust
pub trait LLMEngine {
    fn add_text_content(&mut self, text: &str);
    fn add_image_content(&mut self, base64_image: &str);
    fn clear_content(&mut self);
    fn execute(&mut self) -> Result<String>;
}
```

### Adding Local CV Detection

Implement in `src/analysis/circle_detector.rs`:
- Hough Circle Transform
- Contour detection
- Shape analysis
- Fallback to LLM if fails

## Troubleshooting

### Build Issues
- **Windows**: Cannot build natively (ARM targets only)
- **Solution**: Use `cross` with Docker

### Runtime Issues
- **No xochitl process**: Tablet in sleep mode or no document open
- **Touch not working**: Check trigger corner setting, use hand not pen
- **API errors**: Verify OPENAI_API_KEY is set

## References

- **ghostwriter**: Core device interaction code source
- **git-cliff / vergen-gitcl**: Semantic releases and Git-derived application versions
- **reMarkable Community**: Device documentation
- **OpenAI**: Vision API capabilities

---

**Version**: 0.1.0  
**Last Updated**: 2025-11-01

## Highlighted selections

Reader accepts a deliberate highlighted passage as well as a closed hand-drawn
outline. A highlight needs no enclosing circle. The handwritten question must
still be readable and clearly associated with the selected topic, and an independent
transcription must agree before output. Printed gray figures, shading or an X do
not constitute a user selection. Missing or ambiguous question/selection cases
request NONE instead of a general passage summary.

The required SELECTION_CENTER response field denotes the center of either the
outlined or highlighted content in the full 768 x 1024 overview, independently
of the question box or detail crop. Exactly one finite, in-bounds pair is required;
missing, ambiguous or invalid coordinates stop processing before verification or
navigation. Rust normalizes the pair to [0, 1], rounded to two decimal places with
trailing zeros omitted, and writes matching `<Start of Q-A block for Q @ (x, y)>`
and `<End of Q-A block for Q @ (x, y)>` delimiter lines around ordinary `Q:` and
`A:` content. Both delimiters stay with the complete block through undo/redo.
Earlier plain-Q or coordinate-Q answers remain unchanged; there is no migration.
Future follow-up extraction and scrolled-page recognition are separate work and
must handle legacy text explicitly. Recognition
and approximate center placement remain model-based, not precise region detection.

Modern RM2 framebuffer capture uses neutral-preserving luminance from BGRA pixels.
Native highlighter colors exist in the framebuffer even on the monochrome tablet;
using only blue darkened yellow highlights. The conversion preserves all neutral
gray values and improves printed-text contrast under yellow marks. Legacy RM2,
Paper Pro and framebuffer allocation discovery are unchanged.

## REM-8 progress ownership

`workflow::indicator` owns geometry/clearance eligibility; `Workflow` tracks eligibility and owned temporary native marks. Capture, navigation and keyboard wrappers clear owned marks first; typing/navigation invalidate stale eligibility. A cleanup error poisons the orchestrator, which cannot resume on an unknown later page. Status drawing failure attempts immediate cleanup before the HTTP wait finishes.

`LLMEngine::execute_with_progress` has a deterministic default; OpenAI runs one bounded HTTP call in a scoped worker and waits up to 750 ms for its result between callbacks on the caller thread (drawing time adds to the visible refresh interval). The worker never owns device input. Callback errors stop ticks, the worker is joined, and its answer is discarded. RealDevice retraces the owned circle with the rubber tool and adds 100 ms settling after native erasure; native visual checks remain necessary because event injection does not itself prove xochitl has removed the strokes. Simulator models a separate temporary circle and records lifecycle operations/faults.

The 50x50 box is (698,934)..(747,983), with 12 pixels of blank clearance. The circle centerline has radius 14 around (722,958); X centerline endpoints are inset 10 pixels. Native highlighter strokes extend beyond these paths, which is why endpoint coordinates alone are not a bounds check. Final RM2 highlighter smoke measured active bounds 700,936..743,979 and zero ROI differences after erasing; neighboring Fineliner ink survived. Tick drawing took 476-491ms and clearing 576ms, so the 750 ms wait is added to drawing time rather than defining the whole refresh cadence.

The independent transcription pass still treats provider errors as conservative declines, but propagates device progress/cleanup errors. Answer-page classification uses the shared clean-capture path, refreshing status eligibility from the settled classification frame rather than the earlier navigation screenshot.
