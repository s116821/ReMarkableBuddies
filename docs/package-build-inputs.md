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
Future APK consumption and production trust require their own reviewed admission.

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
