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
- `OUTLINE_BOX`: Location of outline shape (preserved)
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

The normal workflow requires reMarkable hardware. Desktop behavioral tests run with
`cargo test --all-targets`. For bounded device checks, use:
- `--screenshot-only FILE` to capture without credentials or input initialization
- `--once --no-trigger` to run one real-device question without a held gesture
- `READER_BUDDY_DEBUG_DUMP=true` to retain optional local capture diagnostics

The former `--input-png`, `--no-draw` and `--save-screenshot` switches are removed;
they did not provide a working offline simulator.

## CI/CD Integration

Uses **MagDrago Rust Semver Action** for automated versioning:

**Version Bump Rules**:
1. **Major** (X.0.0): Commit with `!` before `:`
2. **Minor** (0.X.0): Merge from `feature/` branch
3. **Patch** (0.0.X): Any source file change
4. **None**: Docs-only changes

**Source Files**: Defined in `.versioning/source_globs.txt`

**Workflows**:
- `.github/workflows/ci.yml` - Format, lint, check on PRs
- `.github/workflows/release.yml` - Version, build, release on main

## Testing Strategy

### Current Testing
- Behavioral tests cover configuration, hold timing, page decisions, Q&A composition
  and response parsing without a tablet.
- Real-device smoke remains required for input, model, navigation and rendering changes.
- Inspect service logs with `journalctl -u reader-buddy.service`; use `RUST_LOG=debug`
  in the protected service environment file for more verbose diagnostics.

### Future Testing
- Mock device interfaces for desktop testing
- Full simulator scenarios with screenshot fixtures and model-response replay

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
- **MagDrago Rust Semver Action**: Automated versioning
- **reMarkable Community**: Device documentation
- **OpenAI**: Vision API capabilities

---

**Version**: 0.1.0  
**Last Updated**: 2025-11-01

