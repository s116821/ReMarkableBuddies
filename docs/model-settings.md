# Saved model and endpoint declarations

This independent maintenance protocol edits only root `model` and `base_url`. It never resolves credentials, calls a provider, reloads the running service or contacts a tablet. Select an existing absolute configuration file with `REMARKABLE_BUDDIES_CONFIG`; there is no `--config` option or default-path fallback.

Use `--model-settings-operation inspect` without input, or prepare/apply/status with exact `--model-settings-input -`. One EOF-delimited strict JSON request and one LF-terminated JSON report each have a 16384-byte cap. Configuration files have a 1048576-byte cap. Unknown/duplicate keys, arrays instead of records and unrelated runtime/maintenance flags refuse.

Inspect returns original file/path hashes and origin/presence for the two declarations. Prepare supplies those original hashes, version1, a fresh operation UUID and one or two explicit set/clear changes. Set strings retain exact decoded contents under the existing nonblank/256-byte model and 2048-byte endpoint limits. No URL validation or normalization is added. Clear removes the declaration. Unrelated declarations survive; a logical no-op preserves original bytes.

Prepare returns a digest-only intent, with no original/new model or endpoint text. Keep the original patch privately; apply supplies `{intent,changes}` and verifies the original file/path/patch/target before using the same guarded conditional configuration writer. Status supplies only `{intent}` and reads the original file without Store/root/provider probes. Current desired bytes prove no historical publisher identity or active runtime state. Hashes and intent are private maintenance context, not renderer data or bearer authority.

A complete verified apply confirms saved disk bytes and reports restart-required. Possible publication/readback or missing output remains unknown. Do not retry/reapply automatically; explicit status can observe desired-file-observed or unresolved. It cannot settle an earlier unknown attempt merely because a later invocation refused. No runtime/effective-provider qualification follows: model CLI/file/default and endpoint CLI/environment/file precedence remain unchanged.

The exact protocol lives in the coordinated central `edit-saved-model-settings` change. Host API/UI and private patch-buffer lifetime require separate implementation and review. The existing three-field basic editor and general inspection contracts remain unchanged.
