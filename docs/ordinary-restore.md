# Conditional ordinary archive restore

This new maintenance interface is a bounded producer candidate. Previously
published v0.20.0 does not implement these selectors. It restores an ordinary
format1 archive additively into an existing ordinary owned Store in unconfigured
or local-only mode. Selected archives/stores, shared/isolated modes and clean or
replacement-device setup remain unsupported by this interface.

Set `REMARKABLE_BUDDIES_CONFIG` to the explicit absolute configuration file. There
is no `--config` flag, default selection or file-input fallback. Each operation
reads strict bounded JSON from stdin through EOF:

```text
reader-buddy --restore-operation prepare --restore-input -
reader-buddy --restore-operation apply --restore-input -
reader-buddy --restore-operation status --restore-input -
```

Prepare takes `{restore_version:1,operation:NONNIL_UUID,source:ABSOLUTE_ARCHIVE_PATH}`.
It writes nothing and returns an additive impact plus a private original intent.
Keep that exact intent and source spelling privately. Apply/status take exactly
`{source:ABSOLUTE_ARCHIVE_PATH,intent:ORIGINAL_INTENT}`. Paths, actor/generation IDs,
hashes and intent are host-private evidence, not renderer inputs or physical
tablet identity proof. A future companion needs its own reviewed host/UI path.

One deliberate apply revalidates the original configuration/path/mode, source
metadata/content and target generation/committed history under the same Store
mutex used by the existing restore writer. It reserves the prepared new generation
exclusively, preserves existing target records and stages/verifies the additive
union before atomic CURRENT activation. A running owner is Busy. No store is
initialized/repaired, no service is stopped/restarted and no provider is contacted.

Original generation/source and partial stages are retained. A complete
`not-dispatched` result never entered the restore writer. `not-activated` means
CURRENT publication was not attempted, but partial staged files can remain.
`unknown` means CURRENT publication was attempted and its verified result is
unavailable. A missing or truncated report is not rollback. Do not reuse an
existing target stage, automatically retry, clean up retained files or apply the
same intent again. A new deliberate restore uses new original conditions and a
new prepared generation.

Verified `restored` means exact desired on-disk generation/closure readback, with
runtime activation still unverified. Explicit status acquires consistent existing
Store/mode custody and checks the original target binding plus complete desired
state; it does not reread the source archive. `desired-generation-observed` proves
only the intended current state, not historical publisher identity. A matching
UUID alone is insufficient. Nonmatching state is `unresolved`; a later status
refusal cannot settle an earlier unknown apply.

The current actor, active configuration, credentials, caches and original native
documents remain separate. An optional legacy archive configuration is kept only
as inactive `restored-config.json` inside the new generation; it is never applied
or used to resolve credentials. Media completeness preserves intentional omission
versus missing promised content. No remote group join, native-page availability,
account synchronization or service readiness follows from restore.

Input/report caps are16KiB, including whitespace and the report LF. Existing
metadata/record/object bounds and host deadlines remain in force. The full
[versioned contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/5ed60540affe670c89dac2ff4db9c61f12a2a53a/openspec/changes/restore-ordinary-owned-archive/contract.md)
defines exact shapes, fingerprints, private conditions, statuses and limits.
