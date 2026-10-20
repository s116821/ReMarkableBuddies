# Saved memory declarations

This maintenance protocol edits only `memory.enabled` and `memory.profile`. It does not create or inspect profiles, change `memory.selection`, call a provider, reload a service or contact a tablet. Select an existing absolute configuration file through `REMARKABLE_BUDDIES_CONFIG`.

Use `--memory-settings-operation inspect` without input, or prepare/apply/status with exact `--memory-settings-input -`. Strict EOF-delimited request and LF-terminated report limits are 16384 bytes; the complete configuration limit is 1048576 bytes. Duplicate/unknown keys, positional records and unrelated runtime or maintenance options refuse.

Prepare binds the original file/path hashes, version1, a fresh operation UUID and one or two set/clear changes. Enabled accepts a boolean. Profile accepts the existing decoder's nonnil UUID strings and preserves their exact decoded spelling. Clear removes only the selected leaf. Clearing an absent parent does not create it; clearing the final editable child of an existing parent retains `{}`. Original invalid files refuse even if the proposed patch would repair them. Unrelated values, including selection, survive; semantic no-ops retain exact file bytes.

Reports disclose enabled's origin/value and profile's origin/presence, never profile text. Changed prepare returns a digest-only intent. Retain the original patch privately for apply's `{intent,changes}`. Apply verifies original file/path/patch/target and uses the same guarded conditional configuration writer as the existing basic/model editors. Status accepts only `{intent}` and reads the original file without Store, root or profile probes.

Every report, including verified apply, keeps runtime activation `unverified`. Saving false does not disable existing explicit maintenance commands; saving true/profile does not activate automatic Reader memory or prove profile membership. Desired-file-observed proves current bytes, not historical publication or runtime state. Partial/missing output and possible publication failures remain unknown; do not retry automatically or infer rollback.

The exact coordinated contract is the central `edit-saved-memory-settings` change. Host UI and automatic Reader memory remain separate work.
