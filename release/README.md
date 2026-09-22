# Release tooling and verification

Requirements are public in `openspec/specs/release-versioning` (after canonical sync)
and the corresponding change proposal. The implementation uses git-cliff **2.14.2**
for semantic version computation, release-it **19.0.6** for actual tag creation
and vergen-gitcl **10.0.3** for Rust metadata.
No private board, account connector, API key or development tablet is needed.

Install Git, Node.js 22+, Python 3.11+, Rust 1.96+ and the pinned git-cliff binary from its public
GitHub release (or `cargo install git-cliff --version 2.14.2 --locked`). Then run:

```sh
npm ci --prefix release --ignore-scripts
cargo fetch --locked
python -m unittest discover -s release -v
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
```

Set `GIT_CLIFF` to an absolute binary path when it is not on PATH. The regression
repositories are temporary local repositories/remotes; all fixture tags stay there.
Tests execute the real semantic tool and actual build.rs, including cached source
transitions, dirty/shallow/missing Git rejection, mixed/reverted application changes,
queued requests and retry identity. Workflow tests inspect the actual docs gates.

`python release/coordinator.py plan` is read-only with respect to tags/releases;
it temporarily checks out the selected source in a disposable Git worktree. It
requires a complete `origin/main` history and an existing semantic baseline tag.
`publish` is reserved for the serialized protected-main workflow. It creates an
annotated immutable tag, builds a clean clone at that tag and publishes both archives
only after runtime and checksum verification. Do not run it as an experimental test
against the production remote.

The production workflow uses GITHUB_TOKEN with contents:write and explicit ordered
steps. No PAT or tag-triggered second workflow is required. Its job-level lock
includes both architecture builds and final publication. Pending GitHub jobs may replace one another; each admitted job refreshes main,
recovers managed tags and processes every relevant unreleased squash commit in
order, creating one semantic tag per application merge. Main CI never compiles;
all main application builds occur after their tag is pushed. A failed run is visible and retryable from Actions
or `gh workflow run release.yml --ref main`. A documentation push remains build-free
even when a previous application release failed.

Cross builds pass expected tag/SHA through Cross.toml. The official build script
checks those expectations against real Git metadata; they are not a version override.
The coordinator runs each target's binary with `cross run ... -- --version` before
packaging. This is emulator verification, not proof of tablet hardware behavior.
Native feature verification follows the separate authorized-tablet checklist.

To exercise the actual two-target builder before a release, commit the source and run:

```sh
python release/verify_tagged_build.py --output /tmp/reader-fixture-packages --target-dir target
```

This creates a temporary full clone, removes its remote, gives it a fixture tag,
then runs the production build/package checks for both targets. Tags in the source
repository remain untouched. The resulting packages are test artifacts, not releases.
Use a suitable output path on Windows. On the tested Windows Docker Desktop setup,
bind-mounted files appear owned by UID/GID 0; setting `CROSS_CONTAINER_UID=0` and
`CROSS_CONTAINER_GID=0` for this trusted fixture matches that ownership. Other
container setups should use their actual mount owner. Git's ownership checks remain
enabled; no global or wildcard safe-directory exception is added. Clean clones use
`core.autocrlf=false` so Linux sees the committed bytes. See cross's public
[environment settings](https://github.com/cross-rs/cross/blob/main/docs/environment_variables.md).

Cargo's version is a fixed unpublished package placeholder. Do not bump it for a
release. Future registry packaging would need its own tag-derived manifest step;
it is not a second application version source and is outside this binary workflow.
