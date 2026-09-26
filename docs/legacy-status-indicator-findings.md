# Retired source-page status experiments

The September 26, 2026 roadmap retires source-page triangles, circles, spokes and
failure X marks. Normal Reader requests no longer acquire a status-tool lease,
draw or erase feedback, or run status restoration. Future feedback belongs on
the Buddy conversation page (REM-40). Existing unresolved journals still cause
read-only refusal; they are not silently deleted or replayed at startup.

The prior experiment is preserved at Rust commit
`da8838db9863d504b12b5e8b44a3015af9ded0cd`. Diagnostic-only geometry, ownership and
tool-lease code remains available to explicit developer probes. Its passing
synthetic tests do not establish an active product feature or native safety.

## What the measurements established

- An earlier capture-optimized candidate reduced the median of three matched
  ordinary-tool samples from 5.902 to 4.415 seconds with a single status lease.
  This did not establish availability for nondefault tools.
- One later primary-Medium native sample added 15 identified Lines to a baseline
  of 125, then returned to 125 after clear, save and reopen. Original recognized
  blocks and 66 opaque blocks were preserved. Its deletion record also referenced
  four unknown post-baseline IDs; this was not universal integrity proof.
- Acquisition, cleanup and final observation took 5.675 seconds in that sample.
  The separate host acknowledgement took 13.641 seconds and must not be presented
  as the same latency measurement.
- A manual sentinel sometimes reappeared after apparent erasure. Its cause was
  not established and must not be attributed to production history operations.

The obsolete pen-width and marker-restoration matrix is intentionally unfinished.
It is superseded, not retrospectively passed. Historical evidence and preserved
journals remain useful for diagnosis but do not authorize further marker input.

## Findings retained by the active work

A single qualified outside-panel tap on candidate `dbf5609` was positively
observed as six touch events in two frames. Its following screenshot failed with
the typed vanished-discovery-candidate error. A later read-only image showed the
panel closed, but that separate observation did not make the failed workflow
successful. Recovery must refresh capture at most once, retain the original
owner/native-content/input guards and deadlines, and never repeat the tap.

Native answer persistence remains a separate concern. One 146-character append
took 9.805 seconds to observe as persisted; a PageInfo field changed from 4 to 5
with unknown semantics. Earlier extra-newline behavior remains unexplained.
REM-38, REM-43 and the REM-35 integrated gate must retain opaque-content and
ownership checks rather than normalizing unknown fields or inferring completion
from elapsed time. No new native verification is claimed by this document.
