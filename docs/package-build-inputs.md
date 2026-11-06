# Strict package build inputs

The explicit `package-build-inputs.yml` workflow is a build-input qualification
path, separate from normal release tarballs and APK generation. It has only a
manual trigger and a default-false `execute_build` gate. Ordinary pushes, PRs and
release publication do not dispatch it. Source implementation and PR checks do
not select execution.

For a separately approved operation, select the exact reviewed producer workflow
commit, published stable Buddy tag and full release commit. The workflow uses
normal full standalone Linux checkouts for producer, release source and the
independent SDK v0.2.0 proof. Checkout does not create safe-directory exceptions
or persist credentials. It reuses the maintained pinned Cross, stable toolchain
and immutable images. Strict host/container ownership, normal Git configuration,
raw source inventory, SDK/Cargo, compiler, executed-binary and pre/post checks
remain in the existing `apk_source_build.build` implementation.

The build-only entry is `cicd/release/apk_build_inputs.py`. It requires explicit
`--source`, `--source-sha`, `--tag`, `--sdk-source`, `--producer-sha`,
`--target-dir` and `--output` arguments. Build/output roots are fresh, disjoint
and external to all three checkouts. It builds ARMv7 resident then ordinary
AArch64, using the existing producer rather than a new compiler wrapper.

Each target emits `reader-buddy`, canonical `build.json` and retained
`private-evidence.json`. `build-input-receipt.json` is exclusively published only
after both targets and their exact hashes/source/SDK/profile correspondence
validate. `verify(output, tag, commit, producer_commit)` rechecks the exact
inventory and bytes; a missing marker, extra file, changed payload or contradictory
metadata refuses. Partial outputs are not complete and must not be reused.
These checks establish retained byte correspondence, not authenticity of JSON
supplied outside the reviewed workflow. A hash manifest is not production trust.

Output remains `strict-build-inputs-only` / `not-qualified`. There is no APK,
private key, signature, signer/trust setup, installation or tablet operation.
The current weaker release tarballs cannot be relabeled as these strict inputs.
APK consumption uses the separate explicit entry below. Production trust remains
unqualified.

The workflow has a55-minute job limit; existing build900-second and bounded
helper/runtime phases remain. Compiler stdout/stderr retain the existing16KiB
combined cap. Optional source-probe receipts are at most8KiB per observation.
Private evidence is at most32MiB per target. Successful exact input artifacts and
bounded failure diagnostics are retained14days. Failure upload excludes source
trees, Cargo caches, compiler intermediates and incomplete binary artifacts.
Heavy caches stay on the ephemeral GitHub runner and are retired with that job;
no self-hosted cleanup or held input deletion is selected.

The Linux Cross mapping accepts the retained0.2.5/49cd054 and maintained
0.2.5/8c1a8aa revisions. The latter's reviewed
[local mounts](https://github.com/cross-rs/cross/blob/8c1a8aa4b661711f4b7b6ac07c2e8929ce2f7d27/src/docker/local.rs),
[workdir/user namespace/user](https://github.com/cross-rs/cross/blob/8c1a8aa4b661711f4b7b6ac07c2e8929ce2f7d27/src/docker/shared.rs)
and [host ID defaults](https://github.com/cross-rs/cross/blob/8c1a8aa4b661711f4b7b6ac07c2e8929ce2f7d27/src/id.rs)
retain the normally owned Linux path/UID/GID mapping. Existing environment override,
non-rootless endpoint and ordinary ownership checks still apply. Windows mapping
is unchanged; this workflow does not replay its spent ownership probe.

## Explicit packaging of accepted inputs

`cicd/release/apk_package_inputs.py` consumes completed strict inputs without a
compiler or target-runtime call. Select its execution separately from source
delivery. Supply these arguments using independently reviewed values:

```text
python -B cicd/release/apk_package_inputs.py --build-inputs INPUTS --expected-receipt-sha256 RECEIPT_SHA256 --build-producer-source ORIGINAL_PRODUCER --build-producer-sha ORIGINAL_PRODUCER_COMMIT --executor-sha THIS_EXECUTOR_COMMIT --source RELEASE_SOURCE --source-sha RELEASE_COMMIT --tag RELEASE_TAG --sdk-source SDK_RELEASE_SOURCE --pkgrel 0 --apk-tool PINNED_TOOL_DIRECTORY/apk --rm2-native-bundle PINNED_BUNDLE --work-dir NEW_WORK --output NEW_OUTPUT
```

The external receipt digest binds accepted provenance; adjacent files cannot
establish it. Both original producer and packaging executor must be clean full
canonical checkouts. They remain distinct and the original inputs are read-only.
Actual source/tag/raw-file/license identity must still match accepted evidence.
The consumer compares every field and type in the complete historical and newly
observed independent SDK proofs. Equal proofs are recorded as
`historical-proof-equal`. Exactly a Git-directory ctime-only difference may be
recorded as `historical-git-ctime-only-revalidated`; root identity/ctime, Git
device/inode/mode, raw bytes, tag/remote/manifest/conversion, field/type/structure
changes and any second difference still refuse. This is fresh content
correspondence, with no claim of continuous historical custody. No allow-drift
flag or replacement proof is accepted. Original binary/canonical build metadata
bytes remain unchanged, including versions and profiles.

The unchanged SDK observer enforces full before/after stability within each
observation. Every later packaging recheck must match the newly admitted full
proof exactly, including ctime. A later ctime-only change refuses; it is never
reclassified. Private work evidence `sdk-proof-correspondence.json` retains both
complete proofs, classification, explicit old/current differences and
`continuous_custody_claimed=false`. Completion provenance binds that record's
name, byte count and SHA256 plus classification/differences. Preserve this private
record with operation evidence; it is not a public APK asset or production trust.

The ARMv7 resident profile requires its exact pinned native bundle; it cannot be
downgraded to ordinary packaging to bypass a missing bundle. The existing pinned
APK tool/image and generator/observer isolation remain unchanged. The generation
container's private fixture key stays in temporary storage and is disposed before
the separate public-key-only observer. Only its public key and control-channel
SPKI pin pass forward. This operation remains test-key-only and not-qualified.

Correct tool bytes also need executable permissions. Shared admission checks both
entry points and existing rechecks: on POSIX the task owner needs effective read/
execute access and the pinned image's cap-dropped UID/GID0 needs read/execute in
the applicable file permission class. Special mode bits refuse; equivalent
ordinary modes are accepted without an exact0755 whitelist. A non-executable0664
or owner-only0744 tool refuses before building or image/signing execution. The
operator may prepare an owned tool's ordinary mode separately (for example0755),
recording unchanged content/owner/inode. Production admission never chmods it.

Fresh private `generation-diagnostics` and `observation-diagnostics` work folders
retain bounded outer phase stdout/stderr on success, failure or deadline. The
existing writer additionally accepts only generation's existing64KiB-stdout/
None-combined profile; stderr remains16KiB and all command/cleanup/time limits
remain unchanged. No private key, environment dump or public artifact is added.
Outer Python/container tracebacks do not recover stderr already discarded by a
nested failing command. Preserve the first failure; do not replay to obtain logs.

Work/final directories must be fresh disjoint external roots, separate from all
source and input checkouts. Failed or changed inputs never publish the final
`test-key-build-receipt.json`. Preserve useful diagnostics and inspect partial
work before a new explicit attempt; there is no automatic retry. A successful
receipt additionally records the accepted input-receipt digest, original producer
commit and packaging executor commit. This does not publish a release or enable
installation, production signing trust or device compatibility.

Both isolated phases bind the exact admitted tool file read-only at `/tool/apk`;
the original host tool directory may remain private. Fresh packaging work is
owned/private0700; only its newly created artifact child is initialized0777 so
the unchanged cap-dropped UID/GID0 container can export root-owned files. The
private work parent prevents unrelated host traversal. Frozen input/tool/source
permissions are never normalized.

Before generation, between phases, before/after public-only observation and at
final-copy/completion boundaries, shared admission checks the private ordinary
work root and applicable UID0/GID0 access to fixed mounted directories/files.
Unknown/symlink/nonordinary or inaccessible staging refuses; admission never
chmods. Restrictive umask staging may therefore refuse before container launch.
Existing source/input/SDK proof, signing disposal, SPKI channel, root/root payload
inventory, read-only observer mounts and cleanup/resource bounds remain required.
