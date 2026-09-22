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
tests run with `cargo test --all-targets`. For bounded device checks, explicitly build/copy the examples described in
[README testing](../README.md#testing) and use:
- the `screenshot FILE` example to capture without credentials or input initialization
- the `reader_once` example to run one real-device question without a held gesture
  (environment credentials/endpoint, default model and LL; stop/restore normal service)
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
- Inspect service logs with `journalctl -u reader-buddy.service`; Reader Buddy debug
  diagnostics are enabled by default. `--log-level` overrides `RUST_LOG`; absent
  either, dependency logging stays at info. Image dumps remain opt-in.

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

## Reader progress ownership

`workflow::indicator` owns geometry/clearance eligibility; `Workflow` tracks finite temporary paths and the status-style lease. Capture, navigation and keyboard wrappers erase owned paths and restore preferences first; typing/navigation invalidate stale eligibility. A cleanup or restoration failure stops future iterations. The HTTP worker never owns tablet input. Caller-thread callbacks schedule stroke starts every 333 ms, skipping missed deadlines without a burst. The native backend establishes rubber proximity before contact, erases each unique path and verifies a fresh clean-corner image. Accepted input alone is not successful erasure.

Earlier REM-8 circle tests proved cleanup only for that geometry. REM-32 found that open triangle edges needed the rubber-proximity correction and that broad Highlighter strokes obscured nested geometry. The failed captures remain evidence; earlier circle dimensions/timings do not describe current triangles.

### Reader status stages and failures

The reserved bottom-right 50x50 region must be blank with its 12-pixel clearance before status drawing. Preparing, answer-request and answer-response stages use successively smaller triangles. Each edge starts on a 333 ms schedule; pending activity retraces edges. Queued first traversals complete in order while HTTP dispatch proceeds, so very fast responses can precede their displayed transition. Independent transcription uses a tangent incircle inside the innermost triangle. A page-scoped temporary black medium Fineliner style keeps the geometry readable; exact original tool, active slot and saved preferences must return before continuation.

Temporary paths are recorded before input and erased before capture, navigation or typing. Cleanup is bounded by unique paths, not request duration. Failed cleanup stops further input. Error marks are persistent X-plus-segment codes documented in [the simulator guide](simulator.md); neither expected failures nor outer-loop model/network errors type diagnostics into the document. History gestures draw no activity, and native scene ownership checks remain conservative. REM-4 will add context-enhancement center spokes; they are not implemented by this change.

The hardware probe's `indicator-smoke` command captures each stage and the auxiliary circle under `/tmp/reader-buddy-status-*.png`, then cleans its owned paths. `failure-code <code>` draws one guarded persistent failure mark on the currently selected disposable page. These are explicit native mutation diagnostics: stop the normal service, verify the selected document/tool/corner and restore state afterwards. They make no model request.


### Native status style restoration

The initial style UI contract is limited to RM2 firmware 3.28.0.172 in the observed portrait toolbar layout. Unsupported/hidden controls, an already-open menu, active non-pen tools or ambiguous identity suppress status. Screenshots used to inspect controls have a separate buffer, preserving clean model detail images. Before dependent UI input the lease checks page/session identity, visible page content and supported controls. Repeated status ticks never toggle tools or write recovery checkpoints.

Current controls are authoritative: native document files can retain old tool preferences while the UI shows newly selected pens. The lease captures the actual active slot before opening menus, the primary grid before selecting Fineliner, and actual Fineliner color/width before changing either. Each potentially changed dimension is recorded before input. A failed inspection restores the original grid/slot without touching color/width that were never changed. Restoration verifies actual menu values and original slot/tool controls; equality of saved files is not proof. A verified rollback suppresses status; failed rollback or restoration stops further input, including outer-loop cleanup retries.

Before mutation, `/var/cache/reader-buddy/status-style-recovery.json` is exclusively created with owner-private permissions. Its version2 newline-delimited JSON snapshots record phase, captured UI values and monotonic may-have-mutated flags; advisory metadata is clearly separate. Initial file content and the Linux parent directory are flushed before input. Each later input has a flushed intent checkpoint. Limits reserve space for worst-case rollback. The original file identity is checked before append/removal; partial records, ownership/I/O failures or interrupted restoration retain recovery evidence and stop automatic input. Reader startup refuses an unresolved journal before trigger dismissal or other UI input. Successful verified UI restoration removes the owned valid journal.

For deliberate recovery, stop Reader and diagnostic writers, inspect the last complete validated checkpoint and confirm its document/current UI. Restore only captured values for dimensions the record says may have changed; an early probe may require only closing its menu and restoring the active slot. The primary tool index is row-major in the observed three-by-three grid; Fineliner is index1. Color indices are Black, Gray, White, Blue, Red, Green, Yellow, Cyan, Magenta; width indices0/1/2 are Thin/Medium/Thick. Never use advisory file preferences as current rollback targets or apply an old snapshot over later deliberate user choices. Verify the controls before removing the recovery journal and restarting. Native ink left by a crash is not automatically claimed or erased. This journal protects interrupted Reader input; it does not force xochitl to persist its own UI state or provide transactional document undo.
