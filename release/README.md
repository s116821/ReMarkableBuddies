# Tag-based releases

> Unmerged REM-46 implementation: actual upstream-distribution fixtures pass
> direct recovery of an old draft with its existing ID, partial upload retry and
> published-release skips. Hosted CI, final independent review and coordinated
> delivery remain required; the central change is not archived or complete.

Git tags are the only application version authority. GitVersion **6.8.2** computes
versions from the exact squash commit's tagged ancestry and conventional history.
The fixed package-manifest version is not maintained as an application version.
Upstream Actions own filtering, calculation, immutable tag creation and publication;
`build.py` only builds, packages and verifies app artifacts. There is no custom
release coordinator, path classifier, bump algorithm or GitHub publication client.

## Workflow

`release.yml` admits only merged main PRs, using the trusted base workflow and the
actual squash SHA, including contributions from forks. It never executes an
unmerged PR head. `paths-filter` excludes the explicit documentation paths in
`.github/application-paths.yml`; `GitVersion.yml` has equivalent history exclusions.
Unknown paths, executable fixtures, renames and mixed changes stay relevant.
PR file counts must be present and between 1 and 2,999; the API cannot reliably
classify larger PRs, so split them. Required checks finish on docs-only changes.

Scoped feat increments minor; fix/perf/refactor/build/ci/chore/test/revert increments
patch; breaking syntax calculates major even at 0.x. An application PR cannot use
a docs title. The separate REM-35 gate blocks major publication until 1.0 is ready.
Existing baseline tags are preserved; there are no generated version-bump commits.

`publish.yml` keeps the tag/build/upload sequence in one native publication queue
(`queue: max`, at most 100 pending runs). Ordering is not assumed: every run retains
its exact source, and GitVersion handles earlier commits after a newer descendant
has already been tagged. Cancelled/overflowed runs remain visibly unfinished.
The tag Action never force-updates a tag. A fetched remote tag must match the exact
admitted SHA before compilation. Ordinary main CI never compiles the application;
PR CI builds development artifacts. No tag-triggered second workflow or PAT is needed.

Official packages contain the tag version, source SHA and checksum provenance.
Application code is checked out at the tag while build tooling comes from the
reviewed workflow revision, allowing recovery after tooling changes. Assets upload
to a draft; a separate final Action publishes only after all uploads succeed.
Published releases are skipped without rebuilding or replacing their assets.

Exact release discovery uses the official GraphQL Action's fixed tag query;
an old draft is found directly without listing pages. Missing/error/malformed
observations fail before building. Published releases skip building and writes.
Draft IDs are preserved through fixed official create/publish requests and the
upstream uploader. The uploader reads at most 30 existing assets, so a supported
draft must contain at most 26 assets before upload, reserving the entire fixed
package inventory (three Rust or four Manager files). Larger or malformed counts
refuse safely; they are never silently treated as empty. Downloads must match the
fixed inventory and checksums before upload; only successful uploads allow final
publication. Native concurrency serializes workflow retries, without claiming
protection from an external actor changing a release during a run.

## Recovery

Rerun the original failed Release workflow. If a tag already exists, use **Run
workflow** on main with that exact tag, or:

```sh
gh workflow run release.yml --ref main -f tag=v0.1.17
```

Substitute the unfinished tag from this repository. Manual recovery never invents
a version or tags current main. If failure occurred before a tag existed, rerun
the original merge run. A docs merge remains build-free and does not replay earlier
failures. Direct main commits outside the PR/squash workflow do not automatically
receive releases; retain the PR workflow instead of manually stamping versions.

## Target runtime evidence

`Cross.release.toml` pins the build/emulation images. `build.py` records the
toolchain/image identity and forces eager dynamic binding during emulated version
verification. `abi.py` verifies ELF architecture, interpreter, ARM hard-float ABI,
NEEDED providers and all loader symbol versions against the selected RM2 vendor
sysroot inventory, including version requirements arising from weak symbols.
The baseline records October 1 independently matched libc/libm/libgcc/loader
hashes for development RM2 firmware 3.28.0.172. It contains interface metadata only,
not provider binaries. A changed dependency requires reviewed baseline evidence;
an unsupported provider/symbol/version fails the build.

This remains a legacy cross backend. It does not claim ReMarkableOpenSDK
consumption or native app/API qualification. AArch64 receives ELF metadata and
selected-image emulation evidence; Paper Pro hardware qualification remains
REM-29. One Buddy tag/source covers both target packages; Manager versions remain
independent and no separate SDK runtime installation is introduced.

## Public fixtures

Install Git, Node 24, Python 3.12 and the official GitVersion 6.8.2 tool. Its
`dotnet-gitversion` executable must be on PATH, or set `GITVERSION` to the standalone
executable. CI installs it with GitTools/actions. No private board, API key, account
connection or tablet is needed.

```sh
python -m unittest discover -s release -v
npm ci --prefix release/fixtures --ignore-scripts
```

The Actions fixture additionally executes pinned upstream distributions. Clone
these repositories into `.upstream/paths`, `.upstream/graphql`, `.upstream/request`,
`.upstream/upload` and `.upstream/tag`, then checkout the exact commits below.
Set `PATHS_ACTION`, `GRAPHQL_ACTION`, `REQUEST_ACTION`, `UPLOAD_ACTION` and `TAG_ACTION` to their absolute directories and run `npm test --prefix release/fixtures`.
These same steps appear in CI. Git Bash is used on Windows (override `BASH` if needed).

| Fixture distribution | Exact commit |
| --- | --- |
| dorny/paths-filter v4.0.3 | ceb8a2b8f2d89434be7ff52d3de7ec3738c5cc9d |
| octokit/graphql-action v3.0.2 | ddde8ebb2493e79f390e6449c725c21663a67505 |
| octokit/request-action v3.0.0 | b91aabaa861c777dcdb14e2387e30eddf04619ae |
| AButler/upload-release-assets v4.0.0 | 34491005a5d7ec239a784e460807ce844fde7962 |
| rickstaa/action-create-tag v1.7.2 | a1c7777fcb2fee4f19b0f283ba888afa11678b72 |

Fixtures use real local Git history/tags, actual Action bundles with a localhost
API, and GitHub's expression evaluator for the actual workflow admission guards.
They cover pagination/renames, queued merges, tag conflicts, draft failures/retries,
published skips and precise runtime versions. They never create production tags.
Hosted workflow execution remains separately verified after coordinated delivery.

GitHub documents `queue: max`; actionlint 1.7.12 predates that key. Validate all other
syntax normally and narrowly ignore only its `unexpected key "queue" for
"concurrency" section` warning until the linter supports the native feature.

Requirements live only in the central Docs `release-versioning` capability and
`action-driven-releases` change. Full source and actual API behavior outrank old
coordinator documentation. Upstream licenses are retained by their repositories;
fixture dependencies are development-only.
