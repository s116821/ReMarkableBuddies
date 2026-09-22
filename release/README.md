# Release tooling and verification

Requirements are public in `openspec/specs/release-versioning` (after canonical sync)
and the corresponding change proposal. The implementation uses git-cliff **2.14.2**
for semantic version computation and vergen-gitcl **10.0.3** for Rust metadata.
No private board, account connector, API key or development tablet is needed.

Install Git, Python 3.11+, Rust 1.96+ and the pinned git-cliff binary from its public
GitHub release (or `cargo install git-cliff --version 2.14.2 --locked`). Then run:

```sh
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
includes both architecture builds and final publication. Pending GitHub jobs may
coalesce; each admitted job refreshes main, recovers managed tags and analyzes the
entire relevant unreleased range. A failed run is visible and retryable from Actions
or `gh workflow run release.yml --ref main`. A documentation push remains build-free
even when a previous application release failed.

Cross builds pass expected tag/SHA through Cross.toml. The official build script
checks those expectations against real Git metadata; they are not a version override.
The coordinator runs each target's binary with `cross run ... -- --version` before
packaging. This is emulator verification, not proof of tablet hardware behavior.
Native feature verification follows the separate authorized-tablet checklist.

Cargo's version is a fixed unpublished package placeholder. Do not bump it for a
release. Future registry packaging would need its own tag-derived manifest step;
it is not a second application version source and is outside this binary workflow.
