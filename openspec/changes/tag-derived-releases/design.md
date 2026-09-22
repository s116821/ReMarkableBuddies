## Context

The old v0.1.3 action checks `feature/*` branch names rather than `feat` messages. On main it falls through to a source-change patch bump. It writes Cargo version commits and builds a checkout not bound to its tag. CI also compiles every main push. Existing history through v0.1.12 must remain intact.

REM-30's description, timestamped comment list (empty at planning), and the user's manifest clarification define acceptance. The concurrent roadmap audit also requires explicit comment chronology review in AGENTS.md; REM-11's omitted footer is handled by that audit, not silently folded into this release change.

## Goals / Non-Goals

**Goals:** reliable semantic versions, immutable source identity, docs-only merge build suppression, safe queue/retry behavior, and reproducible isolated evidence.

**Non-Goals:** registry publishing, tablet deployment, changing Reader behavior, rewriting existing tags or repairing historical release assets. Simulator behavior is unaffected; release repositories and runtime metadata fixtures provide the relevant regression model.

## Decisions

### Maintained semantic computation

Pin git-cliff 2.14.2 (verified published release; the documentation banner still advertises 2.14.0) for conventional semantic calculation and use maintained release-it to create each annotated tag from that calculated version. Disable release-it's version commit, package publishing and implicit branch push. The runner operates on the exact selected squash SHA; the wrapper verifies and pushes only its tag. `feat` always bumps minor, including 0.x; `fix` and other accepted application maintenance types bump patch. `!` and `BREAKING CHANGE:` always bump major, including 0.x to 1.0. No custom version arithmetic or custom tag creation remains.

semantic-release was considered but its branch-head validation complicates releasing the last application commit when newer docs-only commits exist. Cocogitto's default bump flow creates an unwanted version commit. git-cliff exposes a version-only computation at an explicitly selected revision without that commit. Official references: https://git-cliff.org/docs/configuration/bump/ and https://git-cliff.org/docs/configuration/git/.

### Conservative shared path and semantic policy

Use a short explicit list of documentation paths (README/AGENTS/changelog/license, docs, OpenSpec and agent skill documentation). Everything else is relevant by default, including Rust, dependencies, tests, deployment, build scripts and workflows. Share the exclusion configuration with git-cliff and the workflow classifier. Renames/deletions and multi-commit pushes inspect all touched paths, not only the final net diff. Mixed commits remain relevant.

Application commits must have a scoped conventional title using feat, fix, perf, refactor, build, ci, chore, test or revert. Application changes labeled docs or with unknown/nonconventional messages fail visibly before tagging. PR CI checks the prospective squash title and path set, and refreshed title events revalidate it. Pure documentation commits are excluded regardless of their type and never cause a tag. Required PR job names remain present; their application steps are conditional, so documentation PRs conclude successfully rather than waiting forever.

### Serialized publication after a cheap gate

Every push runs a lightweight full-history path classifier, avoiding GitHub's truncated path-filter behavior. Only application pushes or an explicit retry dispatch enter a single job-level release concurrency group with cancel-in-progress false. Documentation pushes do not enter the release queue or compile. Ordinary CI application steps run only for PRs; all main application compilation belongs to the tag-first release job.

Inside the serialized release job, fetch current main and tags again. Select the oldest relevant first-parent main commit after the latest reachable semantic tag. Evaluate the range through that application SHA, excluding docs-only commits. release-it creates its annotated tag without a version commit; push without force and confirm remote identity before any application compilation. Build and publish that merge, then repeat for the next untagged application commit. A replaced pending job does not lose an earlier merge: every relevant squash receives its own semantic tag. A newer documentation HEAD is never substituted as the target.

Recover all incomplete managed tags before creating/publishing another release. Managed annotation identifies this workflow's tags without pretending older custom releases meet the new runtime contract. Tags are the durable pending-work record. A serialized job rebuilds incomplete releases from their exact tags, stages both architecture packages in a draft and publishes only after downloaded-byte verification. Publication includes a completion record with tag/SHA and asset hashes. Later retries compare GitHub's server-side asset digests to that record without downloading every historical binary. Published complete releases are immutable and skipped. API/push/build errors stop visibly; no force updates or version regeneration for the same tagged change. A pending GitHub job can be replaced, but its successor re-reads all unreleased main changes and incomplete tags. A manual dispatch retries failure when no new application merge arrives.

Both targets build sequentially within this serialized job. This trades some latency for a single clear lock spanning tag creation through final publication, eliminating inter-workflow races. GITHUB_TOKEN with contents:write suffices; explicit ordered steps do not depend on tag-triggered runs or PAT-created version commits.

### Tag-derived Rust metadata

Use vergen-gitcl in build.rs for Git describe/SHA metadata and wire the CLI to it. Cargo's required package version becomes a documented fixed placeholder with publish=false; it never drives app version. Clean exact official builds report the release tag version. Other source states report an explicit development identifier; unavailable metadata cannot masquerade as a release.

Official builds require a full, clean Git checkout, exactly matching semantic tag and expected SHA, and no metadata override. Reject shallow, missing, dirty, wrong-tag or wrong-SHA source. Keep tag/ref/source changes in Cargo rerun dependencies so cached development metadata cannot leak into release binaries. Cross receives the official expectations explicitly; verify both foreign binaries' --version under target emulation before packaging, and record tag, SHA, version and checksums in release provenance. Native Windows worktree paths may be unreadable inside Docker; local cross builds can report explicit unknown development metadata, but official builds cannot use that fallback. Reference: https://docs.rs/vergen-gitcl/latest/vergen_gitcl/.

## Risks / Trade-offs

- GitHub queue replacement or out-of-order starts -> each admitted job refreshes main and recovers all tagged pending work; fixture tests simulate stale requests and advanced main.
- Failure after tag push -> retain immutable tag and retry the same source, including on a newer application event.
- Documentation events after failed application release -> remain build-free; manual dispatch or next application event performs recovery.
- Misclassified or unknown application commit -> fail closed with the offending SHA/type rather than silently omit it.
- Tool/path semantics drift -> pin tool and test real git-cliff against isolated repositories, including mixed files, deletion, revert and breaking footers.
- Cross metadata/emulation differences -> exact-tag builds and runtime checks on both architecture targets are acceptance gates, not inferred from host output.
- Release migration first run -> base from existing reachable v0.1.12; no historical tag edits or experimental production tags.

## Migration Plan

Merge only after isolated acceptance tests, strict lint/build checks and independent review. The first real post-merge release tags the merged implementation SHA using the new policy. Observe its ordered tag/build/publication and version provenance. Roll back workflow source by normal reviewed change if necessary; preserve any already created tags and repair publication from their exact SHAs.

## Open Questions

No user decision is pending. Exact git-cliff context flags, vergen builder API and cross/emulator setup will be validated in isolated fixtures; any design adjustment must preserve the contracts above and be recorded before closure.
