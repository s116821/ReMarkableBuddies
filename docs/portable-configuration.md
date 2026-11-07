# Portable preference artifacts

Export a private, read-only snapshot from an explicitly selected saved configuration:

```text
reader-buddy --export-portable-configuration /absolute/path/config.json
```

The command emits one JSON report. Check `status == "ok"`, then save only its
`artifact` object to a private file using your own JSON-aware tooling. Export
includes the selected personalization text, including disabled text; choose
private output/storage rather than routine logs or public reports. No file is
written by Buddy. These commands read explicit absolute regular files; they do
not use stdin, the default configuration, environment configuration/credentials,
configured data roots, a running Buddy or a tablet connection.

Validate the saved artifact independently:

```text
reader-buddy --inspect-portable-configuration /absolute/path/preferences.json
```

Inspection emits a report with a digest, declared-setting count and whether the
personalization member is present. It never echoes the artifact or its text.
`artifact_sha256` hashes compact JSON serialization of the complete portable
artifact; equivalent whitespace/key-order changes share the digest, while text
and declaration changes remain significant. It establishes content correspondence,
not authenticity, ownership or stable live-configuration identity.

## Version1 content

The artifact has exactly `portable_configuration_version:1` and `settings`.
Only these existing saved declarations are allowed inside settings:

- `product_settings`: Reader/Writer enable flags and export policy/schema.
- `personalization`: existing version/enabled/text preferences.
- `research_policy`: existing required/disabled/null preference.

Omitted parents/children, sparse objects and supported explicit nulls stay as
saved. Disabled Unicode personalization text and line breaks remain exact. Defaults
are not inserted as explicit declarations. Product/research parents can be null;
personalization must be an object when present. The full source configuration must
validate before extraction, including excluded settings. Invalid, repeated,
unknown or incompatible data refuses as a whole.

Paths, endpoints, model/credential references, device/group/selection identities,
sync, Reader/memory bindings and logging/debug/gesture settings are excluded.
Version1 does not promise complete model/memory preference portability. The
artifact is not a full backup or a reset/import instruction.

## Outcomes and limits

Both commands emit report_version1 with `runtime_activation:"unverified"`.
Semantic refusal has a non-ok `status` and null artifact/summary. A zero process
exit means a report was written; read its status before using it. CLI conflicts
retain ordinary argument errors. Input must be an explicit absolute regular file;
relative paths, dash/stdin, symlinks/reparse points and special files refuse.
Saved config input is at most1MiB; artifact input and newline-inclusive output are
at most128KiB. These are inclusive byte bounds.

A prefix-write/flush error yields `output-failure` without a replacement frame or
retry. Caller-managed output is not a verified persisted artifact. The reader
inherits existing trusted/quiescent local-path assumptions and validates the
retrieved bytes; it does not claim hostile-filesystem race protection or locking
of a live writer. No private preference is included in routine diagnostic errors.

Neither command imports or activates settings, writes destination configuration,
opens a Store, resolves credentials, contacts a provider/device, or grants export
permissions. Existing local notebook configuration snapshots and archive formats
are unchanged. Selected archives still exclude configuration until a separately
implemented integration exists. Shared sync, cross-actor restoration and runtime
reload remain separate work.

## Conditional saved-preference import

The separate private `--portable-configuration-operation inspect|prepare|apply|status`
family imports version1 artifacts into an existing saved Config. Set
`REMARKABLE_BUDDIES_CONFIG` to its explicit absolute UTF8 regular-file path. It
does not fall back to dotenv/default configuration or resolve runtime credentials.
Inspect reads no stdin; every other operation requires
`--portable-configuration-input -` and one strict JSON object followed by EOF.
Do not put these requests/reports in routine logs: even digests can expose
low-entropy preference text through offline comparison.

Inspect returns the original `base_sha256`, exact supplied-path
`config_path_sha256` and declaration origins for the three allowed parent names.
Prepare takes exactly:

```json
{
  "portable_import_version": 1,
  "operation": "a caller-generated non-nil UUID",
  "base_sha256": "the original Inspect digest",
  "config_path_sha256": "the original Inspect path digest",
  "artifact": {"portable_configuration_version": 1, "settings": {}}
}
```

Replace the explanatory UUID/digest strings with actual values. Present allowed
parents/children overlay destination declarations; omitted members preserve them.
An explicit empty object establishes object presence without clearing existing
children. Supported null remains a saved null declaration. Exact imported text
replaces that declaration, including disabled text; omitted text stays unchanged.
The complete original, artifact and resulting raw Config must validate. Excluded
destination values remain intact and defaults are never materialized.

A semantic no-op returns `no-change`, preserves original bytes and creates no
intent or Store custody. Changed Prepare returns a digest-only `intent` and only
fixed changed parent names/declaration origins. Keep that original intent and
artifact. Apply takes exactly `{"intent": INTENT, "artifact": ARTIFACT}`; it
requires the original path/base/artifact and recomputed exact target digest.
Changed inputs refuse rather than rebasing. Hashes establish correspondence,
not authentication. The path hash is SHA256 of the UTF8 domain
`remarkable-buddies-portable-import-path-v1` plus a NUL byte followed by the exact
absolute supplied UTF8 path string; it does not rewrite paths before hashing.

Only changed Apply opens an existing owned Store: legacy format1 or a complete
ordinary format4 notebook, with absent/LocalOnly mode. Selected/feedback legacy
formats2/3, selected notebook history, incomplete/corrupt readiness and nonordinary
modes refuse without repair. Canonical Config/data/cache/credentials paths must
be disjoint, including Windows case aliases. Existing Store/mode ownership and
conditional publication remain held through the single consuming report write.
This mutating protocol does not flush, retry or emit a replacement frame.

Status takes exactly `{"intent": INTENT}` without an artifact. It needs no Store
or roots: matching exact desired full bytes returns `desired-file-observed`,
otherwise `unresolved`. It cannot identify the publisher, settle historical
publication uncertainty or invoke Apply. Apply reports verified only after the
existing publisher's matching disk readback; possible-publication uncertainty
stays `unknown` without rollback. Prefix output loss is caller uncertainty.

The request cap is256KiB including trailing whitespace; the compact complete
embedded artifact cap is128KiB, original/target Config caps are1MiB, and the
newline-inclusive report cap is16KiB. Bounds are inclusive and independent.
Unknown/duplicate/malformed members and unsupported versions refuse as a whole.
Output errors return fixed `output-failure`. Check report `status` and `outcome`:
a zero exit only means the one semantic report was written.

All import reports keep `runtime_activation:"unverified"`. Saving declarations
does not reload the running application, authenticate credentials, change Store
records/media or grant cloud/native/device authority. The filesystem model is
trusted local paths and cooperating Store writers, not hostile pathname-race
protection. The exact closed request/report schema is in the central
[portable import contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/main/openspec/changes/archive/2026-11-06-import-portable-configuration/contract.md).
