# Development pre-Enter consumer

This closed diagnostic returns historical raw UTF-16 units. It establishes no
semantic Submit, current draft, focus/edit/clear lease or durable save. There is
no new product CLI, automatic input, retry or reconnect. Local socket fixtures
do not model native handles or prove a physical Enter occurred.

The central change is `consume-development-pre-enter` in
[Docs PR 152](https://github.com/s116821/ReMarkableBuddiesDocs/pull/152).
SDK framing, validation, native variant and arming marker remain SDK-owned.

## Isolated source and dependency recipe

Use a dedicated Buddy worktree at the independently reviewed consumer revision,
an originally clean source lock, and a separate output/target directory. Do not
change normal/development dependency declarations, home/repository Cargo config,
ordinary CI or release/APK proof. A path-patched diagnostic deliberately fails
the ordinary package source admission; it is not a distributable resident APK.

Set absolute paths for `SDK` and `EVIDENCE`, then run from that Buddy worktree:

```sh
set -eu
SDK=/ABS/REVIEWED-SDK
EVIDENCE=/ABS/OWNED-OUTPUT
SDK=$(realpath "$SDK")
test "$(git -C "$SDK" rev-parse HEAD)" = 146d78c9eded3dcfbfe84d55ca2de353e1996dff
test -z "$(git -C "$SDK" status --porcelain --untracked-files=normal)"
test "$(git -C "$SDK" rev-parse 'HEAD^{tree}')" = 59d26a10c627765dd75926c733062f2e7c762dc8
sha256sum "$SDK/Cargo.toml"
# Expected manifest SHA256:
# f571158c91cfe61051898e8e987676b99481b1289e3fef66231b1f1ec85dd0a0
mkdir -p "$EVIDENCE"
cp Cargo.lock "$EVIDENCE/Cargo.ordinary.lock"

# These settings belong only to this shell/owned checkout. Start without other
# RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS or target-specific rustflags/config overrides.
export RUSTFLAGS='--cfg buddy_development_native_pre_enter'
export CARGO_TARGET_DIR="$EVIDENCE/target-developer"
PATCH="patch.\"https://github.com/s116821/ReMarkableOpenSDK\".remarkable-open-sdk.path=\"$SDK\""
FEATURES='development-resident-bootstrap,remarkable-open-sdk/development-native-pre-enter'

# Resolve exactly once, preserving the separate ordinary lock above.
cargo metadata --format-version 1 --config "$PATCH" --features "$FEATURES" > "$EVIDENCE/metadata.json"
cp Cargo.lock "$EVIDENCE/Cargo.developer.lock"

# Verify the full graph with Cargo metadata and jq, not a second resolver.
jq -e --arg manifest "$SDK/Cargo.toml" '
  . as $m |
  [.packages[] | select(.name == "remarkable-open-sdk")] as $sdk |
  ($sdk | length) == 1 and
  $sdk[0].source == null and $sdk[0].manifest_path == $manifest and
  ([.resolve.nodes[] | select(.id == $sdk[0].id) | .features | sort] ==
   [["development-capture", "development-native-pre-enter", "mock", "native-adapter"]]) and
  ([.resolve.nodes[] | select(.id == $m.resolve.root) | .deps[] |
    select(.name == "remarkable_open_sdk") |
    {pkg, kinds: ([.dep_kinds[].kind] | sort)}] ==
   [{pkg: $sdk[0].id, kinds: [null, "dev"]}])
' "$EVIDENCE/metadata.json" > /dev/null

cargo metadata --locked --format-version 1 --config "$PATCH" --features "$FEATURES" > "$EVIDENCE/metadata-locked.json"
cmp "$EVIDENCE/metadata.json" "$EVIDENCE/metadata-locked.json"
cmp Cargo.lock "$EVIDENCE/Cargo.developer.lock"
test -z "$(git -C "$SDK" status --porcelain --untracked-files=normal)"
cargo test --locked --lib pre_enter --config "$PATCH" --features "$FEATURES"
cargo check --locked --lib --config "$PATCH" --features "$FEATURES"
cmp Cargo.lock "$EVIDENCE/Cargo.developer.lock"
```

Stop on any failed command or identity comparison. Record the actual toolchain,
Buddy revision and source delta, original/developer lock hashes, SDK identity,
metadata graph, compiler flags/config/argv, and hashes of generated artifacts.
The reviewed Linux test recipe uses Rust/Cargo 1.98.1, native
`x86_64-unknown-linux-gnu`; it does not build a native shim or tablet executable.
Removing the patch does not restore the ordinary lock: use the independently
preserved original lock for ordinary `--all-features` checks in an isolated target.
Never commit the developer lock delta as an ordinary dependency update.

## Build admission

The build-only cfg is absent in ordinary builds, including `--all-features`.
`RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, and Cargo target rustflags propagate the
effective cfg through Cargo's `CARGO_CFG_*` build-script input. Boolean empty
presence selects the gate. Do not set that input manually or supply the internal
`buddy_development_native_pre_enter_admitted` approval cfg yourself.

With selection enabled, any presence of the four release/candidate identity
variables refuses, including empty/non-UTF-8/malformed/unpaired values. An exact
official semantic tag on actual HEAD also refuses, even with a dirty developer
lock and no identity environment. Required Git identity must be available. No
environment values are printed. Ordinary Git-derived build metadata is unchanged.
No version is maintained in source.

Final-target-only `cargo rustc -- --cfg buddy_development_native_pre_enter`
cannot establish build-script approval. Both library and product binary compile
guards refuse that uncoupled selection, including if official metadata was
generated by a build script that did not see the final-target cfg. Use the
documented Cargo-wide developer recipe instead.

Run the build-script refusal regressions against the exact Cargo-built script:

```sh
python3 tests/development_pre_enter_build.py /ABS/target/debug/build/remarkable-reader-buddy-HASH/build-script-build
```

These use disposable Git fixtures, never project tags. The selected tests use the
real Unix client and read-only wrapper for raw NUL/separator/surrogate units,
empty/refused/Unsupported/Capacity, invalid ranges, response correlation, original
deadline, pre/post admission, retention and no automatic replay/reconnect. They
do not fabricate a Native arming marker. A future device packet separately needs
the real marker after handle admission and verified owner/health/recovery checks;
a designated Enter cannot bypass those checks or rely on an unrelated Reader
currentness lease. Physical folio input remains human-dependent unless a real
alternative mechanism is separately verified and authorized.
