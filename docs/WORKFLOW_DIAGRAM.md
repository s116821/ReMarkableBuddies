# CI/CD workflow

The scoped PR title becomes the squash commit message. See
[release policy](../release/cliff.toml) and [public release tests](../release/README.md).

## Pull requests and main CI

```mermaid
flowchart TD
  Event[Pull request] --> Policy[Inspect complete changed-path history]
  Policy --> Docs{Only documented docs paths?}
  Docs -->|Yes| Success[Required checks finish without application compilation]
  Docs -->|No| Title[Validate scoped application semantic type on PR]
  Title --> Checks[Format, lint, release fixtures, Rust tests and both ARM builds]
```

Unknown paths are relevant by default. A mixed code/docs change remains relevant;
application edits followed by reverts in the same push are not hidden by a net diff.
PR title edits rerun classification. A failed policy check fails the named required
checks; a documentation-only change finishes those checks successfully.

## Release ordering and retries

```mermaid
flowchart TD
  Push[Application main push or explicit retry] --> Lock[Enter serialized release job]
  Lock --> Fetch[Refresh main and all tags]
  Fetch --> Recover[Recover incomplete managed tags from their exact SHAs]
  Recover --> Analyze[git-cliff analyzes all relevant unreleased commits]
  Analyze --> Tag[release-it tags next application squash SHA]
  Tag --> Confirm[Push and verify immutable remote tag]
  Confirm --> Clone[Clean full clone at exact tag]
  Clone --> RM2[Build and run ARMv7 version under emulation]
  RM2 --> Pro[Build and run aarch64 version under emulation]
  Pro --> Draft[Upload both packages and provenance to draft]
  Draft --> Verify[Verify uploaded checksums and source identity]
  Verify --> Publish[Publish release and exit lock]
```

Documentation-only pushes never enter the release lock or compile the application.
A later documentation HEAD is not used as the release SHA. Queued application events
are drained in commit order, with one semantic tag and release per application merge.
Ordinary main CI only checks policy; all main compilation occurs after the tag.
No Cargo version commit is created. The tag precedes both release builds.

A failure retains the immutable tag and any draft for retry. Use the Release
workflow's Run workflow button on main or `gh workflow run release.yml --ref main`.
The next application push also recovers pending releases. A docs push remains
build-free even after a failed release. Published complete releases are verified
and skipped; their assets are not overwritten.

## Semantic mapping

| Relevant squash message | Version change |
| --- | --- |
| `feat(scope): ...` | Minor, including 0.x |
| `fix(scope): ...` | Patch |
| `perf`, `refactor`, `build`, `ci`, `chore`, `test`, `revert` with a scope | Patch |
| Scoped `!` or `BREAKING CHANGE:` footer | Major, including 0.x to 1.0 |
| Only documentation paths changed | None, regardless of type |
| Application paths with `docs` or unsupported/unscoped message | Visible validation failure |

A feature followed by a fix yields a minor release then a patch release, each on its own squash SHA. Pure docs
commits in that range are excluded. Both packaged binaries report the semantic
version from the same tag; provenance records its full source SHA and checksums.
Cargo's fixed package placeholder is never a release version source.

## User Experience Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        END USER WORKFLOW                                 │
└─────────────────────────────────────────────────────────────────────────┘

User Wants Reader Buddy
     │
     v
Navigate to GitHub Releases
     │
     v
Download Latest Release
     │
     ├─> reader-buddy-armv7-*.tar.gz (for RM2)
     └─> reader-buddy-aarch64-*.tar.gz (for RMPP)
     │
     v
Extract Binary
     │
     v
Copy to reMarkable
     │
     v
Set OPENAI_API_KEY
     │
     v
Run ./reader-buddy
     │
     v
✅ Using Reader Buddy!
```

