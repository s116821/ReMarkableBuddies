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
