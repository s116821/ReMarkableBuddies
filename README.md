# ReMarkableBuddies

This repository currently contains Reader Buddy. The Rust package, `reader-buddy`
executable, service, and release archive names retain their existing names.

An AI-powered reading assistant for the reMarkable tablet that watches for outlined or highlighted content and handwritten questions, then provides answers directly on your device using ChatGPT.

## Features

- **Selected Content Detection**: Recognizes deliberate highlights or closed outlines (circles, rectangles, or other closed shapes)
- **Question Extraction**: Uses vision AI to read your handwritten question near the selected content
- **Intelligent Answers**: Queries ChatGPT with the selected content and your question
- **On-Device Rendering**: Displays question and answer directly on your reMarkable tablet
- **Answer Page Detection**: Recognizes blank pages or existing answer pages for seamless Q&A flow

## How It Works

1. **Prepare Answer Page**: Before triggering, create a blank page to the **right** of your question page
2. **Select Content**: Highlight the passage or draw a closed shape (circle, rectangle, etc.) around the concept you want to ask about
3. **Write Question**: Write your question near the selected content
4. **Trigger**: Touch and **hold for at least 2 seconds** in the **lower-left corner**, then release
5. **Capture**: The app takes a screenshot of your current page
6. **Read and Verify**: A vision request identifies the selected concept, reads the question, and proposes an answer. An independent transcription pass over the page, without seeing the proposed question or answer, must agree before anything is written. Unreadable or conflicting readings produce an X instead.
7. **Page Check**: App navigates right and checks for a valid answer page:
   - **Valid**: Blank page or existing Reader Buddy answer page → renders Q&A
   - **Invalid**: No page exists or page has other content → draws an **X** in the bottom-right corner of the original page
8. **Render**: Displays the question and answer on the answer page (with "=== Reader Buddy Answers ===" header on first use)

**Important**: You must manually create a blank page to the right of your question page before triggering. The app will NOT create pages automatically.

The circle selects the topic to explain. Answers may use surrounding page content
and general knowledge; paper-specific values must match the visible paper. The
default model is `gpt-5.6-terra`; use `--model` to override it. The extra question
check adds an API request for recognized questions. It reduces confident misreads
but can reject valid handwriting when the readings differ; agreement is not a
guarantee of correctness. See the [hardware evidence comments](https://github.com/s116821/ReMarkableBuddies/pull/10#issuecomment-5687876142)
for results and limitations, and the [build and cost details](https://github.com/s116821/ReMarkableBuddies/pull/10#issuecomment-5687876514).

## Undo and redo the latest answer

On a supported native editing contract, hold four stationary fingers for two
seconds and release all fingers to undo the last Q&A. Hold two fingers the same
way to redo it. The header, older answers and unrelated ink remain. Repeated
toggles make no model requests.

History becomes available only after the complete answer is saved by the native
app; observed RM2 saves take about 10–11 seconds after typing ends. Input during
that confirmation can cancel ownership. Leaving the page, scrolling, editing,
using other controls, starting another Reader iteration or restarting the process
forgets the record. Returning to the page does not revive it.

The initial contract is RM2 firmware 3.28.0.172, ASCII Q&As of at most 2000 characters
and 32 paragraphs. Other devices/firmware and larger answers keep ordinary Reader
rendering without undo ownership. Missing current-document identity, unexpected
text or a native operation failure also disables history; there is no automatic
repair or retry. A native restart can leave identity unavailable until the
document is reopened; only a new successful Reader iteration can create history.
Synthetic native tests do not establish physical-finger or Paper Pro acceptance.

## Installation

### Prerequisites

- reMarkable 2 or reMarkable Paper Pro in developer mode
- SSH access to your reMarkable
- OpenAI API key
- Rust 1.96 or newer and `cross` for cross-compilation

### SSH Host Configuration

Throughout this documentation, `RM2` refers to your reMarkable 2 and `RMPP` refers to your reMarkable Paper Pro. You should replace these with your device's actual connection info.

**Option 1: Use IP address directly**

Replace `RM2` or `RMPP` with `root@<IP_ADDRESS>` in all commands:
```bash
# Example: if your IP is 10.11.99.1
ssh root@10.11.99.1
scp file.txt root@10.11.99.1:
```

**Option 2: Configure SSH host alias (recommended)**

Add an entry to your SSH config file (`~/.ssh/config` on Linux/Mac, `%USERPROFILE%\.ssh\config` on Windows):

```
Host RM2
    HostName 10.11.99.1
    User root

Host RMPP
    HostName 10.11.99.1
    User root
```

Then you can simply use `ssh RM2` or `scp file.txt RM2:` in all commands.

**Finding your reMarkable's IP address:**

1. **USB connection**: Connect via USB cable → IP is typically `10.11.99.1`
2. **Wi-Fi connection**: On your reMarkable, go to **Settings > Help > Copyrights and licenses** → scroll to the bottom to find the IP (usually starts with `192.168.x.x`)
3. **reMarkable app**: If using the reMarkable desktop app, check the connection settings

**Troubleshooting connection issues:**
- Ensure your reMarkable is awake (not in sleep mode)
- Verify developer mode is enabled
- Check that SSH is enabled in device settings
- Try both USB and Wi-Fi IPs if one doesn't work

### Building

```bash
# Install cross-compilation tool
cargo install cross --git https://github.com/cross-rs/cross

# Add targets
rustup target add armv7-unknown-linux-gnueabihf aarch64-unknown-linux-gnu

# Build for reMarkable2
./build.sh rm2

# Or build for reMarkable Paper Pro
./build.sh rmpp
```

### Deploying

#### Option 1: Download Pre-built Binary (Recommended)

Download the latest release from the [Releases page](https://github.com/s116821/ReMarkableBuddies/releases):

```bash
# Extract the binary
tar xzf reader-buddy-armv7-unknown-linux-gnueabihf.tar.gz  # For reMarkable 2
# or
tar xzf reader-buddy-aarch64-unknown-linux-gnu.tar.gz      # For Paper Pro

# Copy to reMarkable
scp reader-buddy RM2:    # or RMPP: for Paper Pro

# SSH into reMarkable
ssh RM2    # or RMPP for Paper Pro

# Set environment variables
export OPENAI_API_KEY=your-key-here

# Run the application
./reader-buddy
```

#### Option 2: Build from Source

```bash
# Build using the script
./build.sh rm2    # or ./build.sh rmpp

# Copy to reMarkable
scp target/armv7-unknown-linux-gnueabihf/release/reader-buddy RM2:
# For Paper Pro use:
# scp target/aarch64-unknown-linux-gnu/release/reader-buddy RMPP:

# SSH into reMarkable
ssh RM2    # or RMPP for Paper Pro

# Set environment variables
export OPENAI_API_KEY=your-key-here

# Run the application
./reader-buddy
```

## Configuration

### Environment Variables

- `OPENAI_API_KEY`: Your OpenAI API key (required)
- `OPENAI_BASE_URL`: Custom API endpoint (optional)

### Command Line Options

```bash
reader-buddy [OPTIONS]

Options:
  --simulate <SCENARIO>     Run a bounded offline simulator scenario
  --screenshot-only <FILE>  Capture and exit without AI or input devices
  --model <MODEL>           Model to use [default: gpt-5.6-terra]
  --base-url <URL>          Custom OpenAI endpoint
  --no-trigger              Skip waiting for trigger
  --once                    Run once instead of looping
  --trigger-corner <CORNER> Trigger corner: UR, UL, LR, LL [default: LL]
  -h, --help                Print help
  -V, --version             Print version
```

## Usage Examples

### Basic Usage

```bash
# Run with default settings (requires OPENAI_API_KEY env var)
./reader-buddy

# Keep OPENAI_API_KEY in the protected service environment file or local .env
# rather than passing credentials in command arguments.

# Use different model
./reader-buddy --model gpt-4o-mini

# Change trigger corner to upper-right (default is lower-left)
./reader-buddy --trigger-corner UR
```

### Testing

```bash
# Run the production Reader workflow locally with scripted model replies
cargo run -- --simulate docs/simulator/scenarios/blank-answer.json

# Capture only (still requires tablet process-memory access)
./reader-buddy --screenshot-only /tmp/page.png

# Run one real-device iteration without waiting for the hold gesture
./reader-buddy --no-trigger --once
```

The old `--input-png` and `--save-screenshot` flags were unused and are removed.
`--no-draw` did not provide a working simulator and is also removed. Use
`--simulate` for the maintained [local simulator](docs/simulator.md), or bounded
diagnostic probes for offline device actions.
Use `OPENAI_API_KEY` instead of `--api-key`, `RUST_LOG` instead of `--log-level`,
and `READER_BUDDY_DEBUG_DUMP=true` instead of `--debug-dump`.

### Background Execution

```bash
# Run in background
nohup ./reader-buddy > reader-buddy.log 2>&1 &

# Check logs
tail -f reader-buddy.log

# Stop background process
pkill reader-buddy
```

## Run at Boot (systemd)

To have Reader Buddy start automatically when your reMarkable boots:

**Prerequisites:** Install the binary to `/opt/bin/` on your reMarkable (standard location for optional software):

```bash
# SSH into reMarkable and create the directory
ssh RM2 "mkdir -p /opt/bin"

# Copy the binary to the proper location
scp reader-buddy RM2:/opt/bin/reader-buddy

# Or if building from source:
scp target/armv7-unknown-linux-gnueabihf/release/reader-buddy RM2:/opt/bin/reader-buddy
# For Paper Pro use:
# scp target/aarch64-unknown-linux-gnu/release/reader-buddy RMPP:/opt/bin/reader-buddy

# Make sure it's executable
ssh RM2 "chmod +x /opt/bin/reader-buddy"
```

**1. Create the systemd service file:**

```bash
ssh RM2
cat > /etc/systemd/system/reader-buddy.service << 'EOF'
[Unit]
Description=ReMarkable Reader Buddy
After=home.mount xochitl.service
Wants=xochitl.service

[Service]
Type=simple
Environment="OPENAI_API_KEY=your-api-key-here"
ExecStart=/opt/bin/reader-buddy
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF
```

> **Important**: Replace `your-api-key-here` with your actual OpenAI API key (starts with `sk-`).

**2. Enable and start the service:**

```bash
# Reload systemd to recognize the new service
systemctl daemon-reload

# Enable the service to start at boot
systemctl enable reader-buddy.service

# Start the service now
systemctl start reader-buddy.service

# Check service status
systemctl status reader-buddy.service
```

**3. Managing the service:**

```bash
# Stop the service
systemctl stop reader-buddy.service

# Restart the service
systemctl restart reader-buddy.service

# Disable auto-start at boot
systemctl disable reader-buddy.service

# Remove the service completely
systemctl stop reader-buddy.service
systemctl disable reader-buddy.service
rm /etc/systemd/system/reader-buddy.service
systemctl daemon-reload
```

### Viewing Logs with journalctl

When running as a systemd service, logs are captured by the journal system:

```bash
# View all Reader Buddy logs
journalctl -u reader-buddy.service

# Follow logs in real-time (like tail -f)
journalctl -u reader-buddy.service -f

# View logs since last boot
journalctl -u reader-buddy.service -b

# View last 100 lines
journalctl -u reader-buddy.service -n 100

# View logs from the last hour
journalctl -u reader-buddy.service --since "1 hour ago"

# View logs with timestamps
journalctl -u reader-buddy.service -o short-precise

# Filter journal priority (may not match Rust application log levels)
journalctl -u reader-buddy.service -p err
```

**Common debugging scenarios:**

```bash
# Check why the service failed to start
journalctl -u reader-buddy.service -b --no-pager

# Watch logs while testing (in one SSH session)
journalctl -u reader-buddy.service -f

# Then trigger Reader Buddy from your tablet and watch the output
```

**Tip**: If logs aren't appearing, ensure `StandardOutput=journal` and `StandardError=journal` are set in the service file. For more verbose output, set this in the service's environment file and restart the service during an authorized maintenance window:

```bash
RUST_LOG=remarkable_reader_buddy=debug
```

## Development

### Development Setup

```bash
# Install Rust toolchain (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install formatting and linting tools (usually included with Rust)
rustup component add rustfmt clippy

# Install cross-compilation tool
cargo install cross --git https://github.com/cross-rs/cross

# Add target architectures for reMarkable devices
rustup target add armv7-unknown-linux-gnueabihf    # reMarkable 2
rustup target add aarch64-unknown-linux-gnu         # reMarkable Paper Pro
```

### Common Development Tasks

```bash
# Format code
cargo fmt

# Check formatting without making changes
cargo fmt -- --check

# Run linter
cargo clippy

# Run clippy with strict warnings
cargo clippy -- -D warnings

# Check that code compiles
cargo check --all-targets --all-features

# Build for reMarkable
./build.sh rm2    # or rmpp for Paper Pro
```

**Architecture**: Modular design with device, llm, analysis, and workflow layers. Device interaction code adapted from [awwaiid/ghostwriter](https://github.com/awwaiid/ghostwriter).

**Technical Details**: See [docs/TECHNICAL.md](docs/TECHNICAL.md) for complete architecture documentation and implementation notes.

## Troubleshooting

### "No xochitl process found"
Make sure your reMarkable is not in sleep mode and has a document open.

### "OPENAI_API_KEY not set"
Set the environment variable: `export OPENAI_API_KEY=your-key`

### "No outlined regions found"
- Make sure you've drawn a closed shape around content (circle, rectangle, or any outline)
- Write your question near the outlined area
- Try using darker/clearer pen strokes
- Ensure the outline is complete (no gaps)

### Touch trigger not working
- Verify the trigger corner setting (default is **lower-left**)
- Make sure you're using your hand/finger, not the pen
- **Hold for 3 full seconds** - a quick tap won't trigger
- The trigger zone is 68x68 pixels in the specified corner
- Keep your finger still in the zone for the full duration

### X appears on my question page instead of answer
If you see an X drawn in the bottom-right corner of your question page, it means the app could not find a valid answer page. This happens when:
- **No page to the right**: You need to manually create a blank page to the right of your question page before triggering
- **Page has existing content**: The page to the right has content that isn't a Reader Buddy answer page (e.g., your notes, a different document page)

**Solution**: Navigate to the page with your question, add a new blank page to the right using the reMarkable's page menu, then trigger Reader Buddy again.

If an occupied successor is rejected, Reader Buddy checks whether the source is
already visible before swiping back. It attempts at most one return swipe. A failed
return draws the X on the current page without repeated swipes; navigate manually
if the source was not restored. At the end of the document, an unchanged page gets
the X without a reverse swipe.

### Answer not appearing on new page
If the answer doesn't render and no X appears:
- Enable debug logging with `RUST_LOG=remarkable_reader_buddy=debug`.
- Check the logged answer-page classification (Blank, ExistingQA, or Invalid).

### Debug Mode
Enable debug dumps to troubleshoot rendering issues:
```bash
READER_BUDDY_DEBUG_DUMP=true RUST_LOG=remarkable_reader_buddy=debug ./reader-buddy
```

This will save to `/tmp/` on the reMarkable:
- `reader-buddy-screenshot-*.png` - Original screenshots captured
- `reader-buddy-erase-mask-*.png` - Only produced when a diagnostic caller explicitly uses the smart-erase helper; the normal Q&A workflow does not erase questions

Image dumps default off; accepted values are `true`, `false`, `1`, and `0`.
Keep dumps local and sanitize logs before sharing: normal logs include question/answer
text, and debug logs include parsed model responses. Request diagnostics do not dump
base64 page images.

**Copying debug files to your computer:**

On Windows (PowerShell):
```powershell
# Copy all debug images from reMarkable to current directory
scp RM2:/tmp/reader-buddy-*.png .

# Or copy to a specific folder
scp RM2:/tmp/reader-buddy-*.png C:\path\to\debug\folder\
```

On Linux/Mac:
```bash
# Copy all debug images from reMarkable to current directory
scp RM2:/tmp/reader-buddy-*.png .

# Or copy to a specific folder
scp RM2:/tmp/reader-buddy-*.png ~/debug/
```

### Downloading Temp and Cache Files for Debugging

For deeper debugging, you can copy all temp files and cache data to your local machine:

**Temp files** (`/tmp/`) contain:
- `reader-buddy-screenshot-*.png` - Screenshots captured during execution
- `reader-buddy-erase-mask-*.png` - Erase mask visualizations
- Other runtime debug files

**Cache files** (`/var/cache/reader-buddy/`) contain:
- Header pattern used for answer page detection
- Other cached recognition data

#### Windows (PowerShell)

```powershell
# Copy all temp files to Downloads folder
scp RM2:/tmp/reader-buddy-* $env:USERPROFILE\Downloads\

# Copy entire cache directory to Downloads folder
scp -r RM2:/var/cache/reader-buddy $env:USERPROFILE\Downloads\

# Copy both temp and cache in one session
scp RM2:/tmp/reader-buddy-* $env:USERPROFILE\Downloads\; scp -r RM2:/var/cache/reader-buddy $env:USERPROFILE\Downloads\
```

#### Linux/Mac

```bash
# Copy all temp files to Downloads folder
scp RM2:/tmp/reader-buddy-* ~/Downloads/

# Copy entire cache directory to Downloads folder
scp -r RM2:/var/cache/reader-buddy ~/Downloads/

# Copy both temp and cache in one session
scp RM2:/tmp/reader-buddy-* ~/Downloads/ && scp -r RM2:/var/cache/reader-buddy ~/Downloads/
```

**Tip**: Enable `READER_BUDDY_DEBUG_DUMP=true` with `RUST_LOG=debug` only when collecting local diagnostic files.

## Cleanup and Uninstall

### Removing Reader Buddy from your reMarkable

SSH into your reMarkable and remove the binary:
```bash
ssh RM2
rm -f /opt/bin/reader-buddy
```

### Cleaning up debug files

Remove debug images from the reMarkable:
```bash
ssh RM2
rm -f /tmp/reader-buddy-*.png
```

### Clearing the cache

Reader Buddy stores cached data in `/var/cache/reader-buddy/`, including the header pattern used to recognize existing answer pages across service restarts.

**When to clear the cache:**
- If answer page detection stops working correctly
- After changing the "=== Reader Buddy Answers ===" header format
- To force Reader Buddy to re-learn what an answer page looks like

```bash
ssh RM2
rm -rf /var/cache/reader-buddy/
```

After clearing the cache, the next blank page you use will become the new reference pattern.

### Complete cleanup (all at once)

```bash
ssh RM2 "rm -f /opt/bin/reader-buddy /tmp/reader-buddy-*.png && rm -rf /var/cache/reader-buddy/"
```

### Stopping a running instance

If Reader Buddy is running in the background:
```bash
ssh RM2
# Find the process
ps | grep reader-buddy

# Kill it (replace PID with actual process ID from the output)
kill <PID>

# Or kill using killall (kills all instances)
killall reader-buddy
```

## Known Limitations

- **Manual Page Creation Required**: You must create a blank page to the right of your question page before triggering - the app does not create pages automatically
- **Single Question Per Trigger**: Processes one outline-question pair per trigger
- **Outline Detection**: Currently LLM-based (future: add local CV algorithms as optimization)
- **Internet Required**: Requires connection for ChatGPT API
- **No Context Retention**: Each trigger is independent (no follow-up question support)

## Pull request titles

Use a Conventional Commit title with a scope: `type(scope): description`.
Include related Linear ticket IDs in the scope when applicable, for example
`fix(REM-17,REM-18): restore RM2 capture and verify handwritten questions`.
For work without a related ticket, use a descriptive scope such as `ci` or `docs`.
The title check runs when a pull request is opened, edited, updated, or reopened.
Link related tickets in the description and distinguish partial work from completed
acceptance criteria.

## Automated Releases

This project uses [git-cliff](https://git-cliff.org/) for semantic calculation,
[release-it](https://github.com/release-it/release-it) for tag creation, and
[vergen-gitcl](https://docs.rs/vergen-gitcl/) for Git-derived binary versions.
Git tags are the only application version authority; Cargo's fixed `0.0.0`
package version is non-authoritative and the package is not published to a registry.

**Version Bump Rules**:
- **Major**: scoped `!` syntax or a `BREAKING CHANGE:` footer, including 0.x to 1.0.0.
- **Minor**: `feat(scope): ...`, including 0.x.
- **Patch**: `fix`, `perf`, `refactor`, `build`, `ci`, `chore`, `test` or `revert` application changes, with a scope.
- **None**: changes confined to the documentation paths in [release/cliff.toml](release/cliff.toml).

The scoped PR title becomes the squash commit message. Application changes labeled
`docs` or with unsupported types fail validation. Mixed code/docs and dependency or
build changes are relevant. Documentation-only main pushes neither create tags nor
compile the application, and their required PR checks still finish.

The release job serializes publication, refreshes all unreleased main history and
tags each relevant merged application commit in order before building it. Each
application merge gets its own tag; a newer documentation commit does not change its source SHA.
Both tablet packages execute `--version` under target emulation before publication.
`provenance.json` records their tag, SHA, version and archive checksums. Existing
published tags remain intact; historical releases predate this verification contract.

For recovery, rerun the failed Release job or use its **Run workflow** button on
main (CLI: `gh workflow run release.yml --ref main`). It recovers incomplete managed
tags from their exact source, without new version commits or tag replacement.
Documentation pushes do not retry builds; the next application push also recovers
pending work. Complete published releases are verified and skipped.

Local/PR binaries report `dev.<git-description>` or `dev.unknown` if Git metadata is
unavailable. Official builds reject missing/shallow history, dirty source, conflicting
metadata overrides and tag/SHA mismatches. See [release testing](release/README.md)
for the public, isolated validation commands.

## Contributing

Contributions welcome! Areas for enhancement:
- Local CV outline detection (reduce LLM calls)
- Multi-question support per trigger
- Device testing and refinement
- Automatic page creation (currently requires manual page setup)

**Recent Improvements (v0.3)**:
- ✅ Simplified workflow - user creates answer page, app detects blank/QA pages
- ✅ Clear failure indication - X drawn in bottom-right when no valid answer page found
- ✅ Debug dump mode for troubleshooting
- ✅ Answer page reuse - multiple questions from same page share one answer page

**Version bumps** follow semantic squash messages as described above; branch names
do not determine the version. Public issue/PR discussions and repository specs carry
acceptance criteria. Private Linear access, Codex and a tablet are not prerequisites.

See [docs/TECHNICAL.md](docs/TECHNICAL.md) for implementation details and TODOs.

## License

See LICENSE file for details.

## Documentation

- **[Technical Documentation](docs/TECHNICAL.md)** - Architecture, implementation details, and TODOs
- **[Workflow Diagrams](docs/WORKFLOW_DIAGRAM.md)** - Visual CI/CD and app workflow diagrams

## Acknowledgments

- [awwaiid/ghostwriter](https://github.com/awwaiid/ghostwriter) - Core device interaction code
- [git-cliff](https://git-cliff.org/) and [vergen](https://github.com/rustyhorde/vergen) - Semantic releases and Git build metadata
- reMarkable community for documentation and tools
- OpenAI for GPT vision capabilities

For explicit provider-backed laptop runs, see [local development setup](docs/local-development.md). Normal simulator regression scenarios remain offline.

## Reader activity indicator

Reader builds nested triangles for preparation, answer-request and answer-response stages, drawing one edge every 333 ms. Auxiliary model calls use an inner circle. Activity and six distinct X-plus-segment failure codes share a 50 by 50 area near the bottom-right corner. Temporary paths are cleared before capture, page navigation, typing and completion; they are never included in model or classifier images. Drawing pauses during keyboard input so pen and keyboard operations cannot interfere.

The area and surrounding clearance must be blank before Reader uses native pen marks. Existing corner handwriting, a previous failure X, or unknown image state suppresses status drawing only; normal question processing continues. Reader does not erase preexisting ink to make room for an indicator. Partial typing invalidates the earlier blank-region check. A cleanup failure stops further navigation/output and ends that orchestrator instead of attempting to erase a mark after another page might have become active.

Production model requests have a 90-second global timeout. Only HTTP work runs on a worker thread; indicator updates and all tablet input stay on the workflow thread. A progress error stops further ticks and the bounded worker is joined before returning. Explicit simulator timeout settings still apply. Abrupt process termination or concurrent manual page edits can leave native marks; this is not transactional document undo.

On the verified native UI contract, Reader temporarily uses black medium Fineliner and restores the exact original tool, active slot and saved preferences. Unsupported controls suppress status; failed restoration stops input. A crash recovery record prevents silent reuse of interrupted settings. See [native restoration and its hardware limits](docs/TECHNICAL.md#native-status-style-restoration).
