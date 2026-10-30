# Inspect saved configuration

Set `REMARKABLE_BUDDIES_CONFIG` in the child process environment to an explicit
absolute configuration-file path, then run:

```text
reader-buddy --configuration-operation inspect
```

The command runs once without reading stdin, creating a Store, resolving provider
credentials or starting the Buddy runtime. Input is limited to one MiB; output is
one report-v1 of at most128 KiB including its newline. It refuses mixed runtime or
other maintenance flags. There is no `--config` option or missing-file fallback.

The snapshot contains25 flat literal field names with declared, declared-null or
default origins. Endpoints, model names, credential references, paths and private
identities are presence-only. Personalization includes only enabled, presence and
UTF8 byte count; use the existing deliberate personalization editor for its text.
Unsupported settings remain explicit. The raw file digest identifies observed
bytes and stays private to the host when Manager reduces the report.

This is saved-file inspection with typed value validation. Declared storage roots
are not probed or validated. A snapshot does not establish effective environment
overrides, provider availability, active storage mode, runtime activation, reload
success, continuing file freshness or permission for a later write. An explicit
refresh takes a new observation; cancellation and identity changes discard old
results. Unavailable, invalid-input and unsupported reports have no snapshot.

The full proposed producer/consumer contract and current review state live in
the central [saved-configuration change](https://github.com/s116821/ReMarkableBuddiesDocs/blob/2a320181739e0d598ed830d31329214e036df646/openspec/changes/inspect-saved-configuration/contract.md).
The shared Manager consumer is a separately reviewed part of this delivery;
this guide alone does not claim it is implemented or hardware-qualified.
