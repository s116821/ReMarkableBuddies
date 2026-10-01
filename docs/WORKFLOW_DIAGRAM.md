# CI/CD workflow

The scoped PR title becomes the squash commit message. See
[release policy](../.github/application-paths.yml) and [public release tests](../release/README.md).

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
  Merge[Merged application PR] --> Queue[Publication queue retains exact squash SHA]
  Queue --> Version[GitVersion analyzes that SHA's tagged ancestry]
  Version --> Tag[Upstream Action creates immutable tag]
  Tag --> Verify[Verify fetched tag target]
  Verify --> Draft[Create or recover draft; skip published release]
  Draft --> Build[Build exact tag and verify both emulated binary versions]
  Build --> Upload[Upload packages and provenance while draft]
  Upload --> Publish[Publish only after every upload succeeds]
```

Docs-only merges never enter publication or compile the app. Native queue ordering
is not assumed; later main changes do not alter a run's source identity. Main CI
performs non-compiling checks. There are no Cargo version commits.

Rerun an unfinished merge run or manually dispatch Release on main with its existing
tag: `gh workflow run release.yml --ref main -f tag=v0.1.17`. A failure before tagging
requires the original run. Published assets remain untouched. See the public
[release guide](../release/README.md) for limits and isolated tests.

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

