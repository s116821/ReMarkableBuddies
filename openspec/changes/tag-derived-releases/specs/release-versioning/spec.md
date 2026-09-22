## ADDED Requirements

### Requirement: Semantic application releases
The release system SHALL use maintained git-cliff to calculate versions from all relevant unreleased conventional squash commits. Scoped feat SHALL bump minor, fix and recognized application maintenance types SHALL bump patch, and ! or BREAKING CHANGE footers SHALL bump major even at 0.x. Existing tags SHALL remain unchanged. Source: REM-30; release configuration and coordinator.

#### Scenario: Feature followed by a fix
- **WHEN** unreleased application history contains feat(REM-9) and fix(REM-9) commits
- **THEN** the next version advances minor, retaining both changes in the release range.

#### Scenario: Breaking change
- **WHEN** a relevant commit has conventional breaking syntax or footer
- **THEN** git-cliff advances major, including from 0.x to 1.0.0.

#### Scenario: Misclassified application change
- **WHEN** an application path changes under docs or an unsupported/nonconventional commit type
- **THEN** validation fails visibly before tag creation rather than silently skipping a release.

### Requirement: Documentation merge exclusion
The workflows SHALL share conservative path classification with semantic release analysis. Pure documentation pushes SHALL create no tag and execute no application compilation in either CI or release workflows. Unknown non-documentation paths, build/dependency changes and mixed changes SHALL be application-relevant. Required PR checks SHALL finish even for documentation-only changes. Source: REM-30; CI/release workflow gates and path policy.

#### Scenario: Documentation-only merge
- **WHEN** a docs commit modifies only README, OpenSpec or documentation paths
- **THEN** the push performs only non-compiling checks and never enters the release publication queue.

#### Scenario: Mixed and reverted changes
- **WHEN** a push includes application and documentation changes, including application changes reverted by a later commit in the same push
- **THEN** classification considers the complete touched-path history and does not discard the application commits because of the net diff.

### Requirement: Immutable tag before exact-source build
Release tags SHALL identify actual merged main application commits without generated version commits. Successful remote tag creation/identity verification SHALL precede every official application build. Both distributed targets SHALL build that exact tagged SHA, report its version and include matching source/checksum provenance. Source: REM-30; release coordinator and build metadata.

#### Scenario: Main advances to documentation
- **WHEN** newer docs commits exist after the selected application merge
- **THEN** the release tag and both artifacts still identify the selected application SHA.

#### Scenario: Tag push fails or conflicts
- **WHEN** the immutable tag cannot be pushed or has a different remote target
- **THEN** publication fails and no application release compilation begins.

### Requirement: Recoverable serialized publication
Application release jobs SHALL serialize tag creation through publication, refresh main state after queueing, and recover incomplete managed tags without duplicate versions or wrong-source assets. Published complete releases SHALL be skipped on retry. A manual dispatch SHALL recover a failed tagged release without requiring another application merge. Source: REM-30; release coordinator and workflow concurrency.

#### Scenario: Queued jobs coalesce
- **WHEN** newer application events replace a pending job or an older event starts after main advances
- **THEN** the admitted job evaluates all relevant unreleased changes rather than only its triggering commit.

#### Scenario: Partial release retry
- **WHEN** a previous run failed after tagging or while building/uploading
- **THEN** a retry uses the existing immutable tag/SHA and publishes only after both target packages and provenance verify.

#### Scenario: Documentation after failure
- **WHEN** only a documentation push follows a failed application release
- **THEN** that push remains build-free and recovery is available by explicit dispatch or the next application event.
