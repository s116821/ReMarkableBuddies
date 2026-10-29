# Ordered application releases

Canonical requirements live in the central
[release specification](https://github.com/s116821/ReMarkableBuddiesDocs/blob/main/openspec/specs/release-versioning/spec.md).
REM-56 supersedes the old tag-before-build/backfill pipeline.

The application-only reusable pipeline runs lint/unit/tool checks, freezes a
read-only Python Semantic Release candidate, builds packages, uploads immutable
CI artifacts, and downloads/exercises those exact artifacts in integration.
Only one final release job can tag or publish. PR/fork runs cannot enter it.
Docs-only changes finish metadata checks without application compilation.

A canonical main push must contain exactly one first-parent main commit.
Squash and ordinary merge events satisfy this structural check, but the actual
admitted main commit must also have a supported scoped semantic subject. A
default nonconventional merge message refuses before build, even if older failed
commits would otherwise supply an upstream candidate bump. Unknown/zero initial history,
non-forward updates and ambiguous batches (including an edit/revert net-zero
batch) refuse visibly before build/tag rather than misclassifying history.
Final associated-PR proof binds the exact source to a merged main PR; a direct
push cannot publish. Repository protection remains owner-managed.

## Maintained tools

- dorny/paths-filter 4.0.3 and semantic-pull-request 6.1.1 handle conservative paths
  and scoped PR titles. Unknown paths and executable fixtures remain relevant.
- Python Semantic Release 10.7.0 uses the hash-locked Python closure and
  conventional-monorepo path exclusions. It computes candidates read-only;
  tag-only invocation happens after artifact integration, with no source
  version, changelog/version commit, or release-job rebuild. Breaking 0.x advances
  major; the separate REM-35 hold refuses premature 1.0 rather than downgrading it.
- Exact GraphQL release lookup and octokit request Actions handle draft
  creation/read/finalization by ID, avoiding draft-list pagination.
- Unmodified Svenstaro upload-release-action2.11.5 source29e53e917877a24fad85510ded594ab3c9ca12de
  is compiled from a pinned minimal build/runtime recipe. TypeScript/ncc and
  required supported dependencies are selected explicitly; the upstream empty
  Jest placeholder and its unused ts-jest/lint graph are not part of this recipe.
  The complete selected closure is audited; this does not turn the original
  upstream audit or the SDK's other failing candidates green or prove security.
  Source, original/selected lock, MIT license and emitted bytes are recorded.
  Actual emitted-Action controls use loopback fake credentials only.

The earlier GitVersion6.8.2 equal-date failure and the SDK source-built softprops
two-page draft-discovery limitation remain evidence, not accepted substitutions.
Historical code/failures remain on the preserved REM-46 branches.

The uploader runs directly from the pinned maintained
`svenstaro/upload-release-action@29e53e917877a24fad85510ded594ab3c9ca12de`
Action distribution. CI does not rebuild upstream JavaScript, maintain replacement
vendor dependency locks, or repeat upstream implementation tests. Project-specific
artifact identity, ARM ABI, same-release retry and remote byte verification remain
local because the generic uploader does not establish those guarantees.

## Build and retry identity

Controlled candidate metadata embeds the frozen future tag/version and exact
clean source before a remote tag exists. These are unpublished CI candidates,
not publication/installation trust. Manager retains its existing four-field
release-profile build-info schema; official:true identifies that controlled
recipe, and alone cannot prove a published release. Ordinary developer builds
remain explicitly unofficial. Final remote annotated-tag/source proof and the
complete verified release supply publication eligibility.

Build artifact IDs flow directly to integration and publication. Buddy integration
executes extracted binaries in pinned cross-image emulation without a Cargo
build/source mount; native tablet compatibility is separate. Manager integration
stages the downloaded browser/native ZIPs and uses existing host checks without
an Angular/Electron application rebuild. CI artifacts and production releases
remain different evidence classes.

Use Actions **Re-run failed jobs** for an interrupted final release: successful
build artifact IDs and tested bytes are retained for 14 days. The same tag/draft
is observed, retained bytes are compared with the independently expected local
packages, only missing names are uploaded, and every remote byte is checked
before finalization. Unexpected/conflicting assets refuse; published complete
bytes are verified and skipped. No failed predecessor is backfilled by a later
success. A full rerun/expired artifact may rebuild only through all gates and
must refuse conflicting retained bytes, never pretend they are the old build.

The uploader receives an exact ID, overwrite:false, check_duplicates:true,
draft:true, promote:false and only missing files. It refuses same-name duplicates
without DELETE. Its upstream ID404 behavior can create a replacement draft;
the independent same-ID proof refuses finalization. It may also restore draft
status after an external actor changes it. Native Actions queue:max serializes
this workflow's publishers, not outside writers; there is no atomic platform
immutability/external-writer guarantee or dispatch-chronology promise.

## Local focused controls

Use Python 3.12, supported Node 24.21 and Git. Install the maintained tools with
`python -m pip install --require-hashes -r cicd/release/python-tools.lock`.
Run `python -m unittest discover -s cicd/release -v`. Rust metadata controls also need
the declared Rust toolchain/dependency cache. Workflow controls use
`npm ci --prefix cicd/release/fixtures` followed by
`node --test cicd/release/fixtures/workflow.test.mjs` on a host with jq.
These controls validate project wiring and release guards, not upstream Action
internals or tablet compatibility.

No local fixture creates a production tag/release, changes repository settings,
or operates a tablet. REM-35/full ecosystem/SDK production selection and current
unmerged consumer parent reconciliation remain separate gates.
