# Saved basic settings

This bounded maintenance interface edits only saved trigger_corner, log_level and
debug_dump declarations. Previously shipped v0.19.0 supports general inspection
but does not implement these new selectors. Unknown command support refuses;
never substitute a file editor, service API or automatic retry.

Select an explicit absolute file through `REMARKABLE_BUDDIES_CONFIG` in the child
environment. There is no `--config` option. Inspect does not consume stdin:

```text
reader-buddy --basic-settings-operation inspect
reader-buddy --basic-settings-operation prepare --basic-settings-input -
reader-buddy --basic-settings-operation apply --basic-settings-input -
reader-buddy --basic-settings-operation status --basic-settings-input -
```

Prepare takes the original editor observation's path/base hashes, a non-nil
operation UUID and a dirty changes object. Each selected field is either
`{"action":"set","value":SUPPORTED_VALUE}` or `{"action":"clear"}`. Clear removes
that declaration; absent changes leave it alone. Supported corners are UR/UL/LR/LL,
levels off/error/warn/info/debug/trace and the debug flag is a boolean. Null is not
a set value. Only a logical change produces a private conditional intent; no-op
preserves the original bytes without touching storage.

A real edit preserves unrelated decoded settings and their omitted/null origins,
including exact multiline personalization text. JSON whitespace/order/escaping may
normalize. Apply consumes the exact original intent once, compares the whole-file
revision and target under the existing guarded atomic writer, and verifies exact
readback. Valid existing ordinary/selected storage and current mode custody are
required. A current owner is Busy. No Store is initialized/repaired, no data or
mode changes, and no provider/credential resolution or service restart occurs.
Successful readback means saved on disk with restart required; it does not prove
effective environment settings or running activation.

If apply might have run but its complete response is missing, preserve the original
intent and treat it as unknown. Explicit status reads only the original file:
desired-file-observed means its complete bytes currently equal the prepared target.
It does not prove which process published them, acknowledge the historical apply,
or prove rollback when they differ. Status never writes or refreshes conditions.
Do not automatically reapply; a new deliberate edit needs a distinct fresh context.
Private hashes/intents stay outside the Manager renderer and routine logs.

Config reads cap at one MiB; basic stdin/report each cap at16,384 bytes, including
EOF-delimited input and the report's final LF. Other runtime/maintenance selectors,
non-dash input files, malformed/duplicate/unknown fields and invalid conditions
refuse. The general inspection wire retains its separate131,072-byte report cap.
The shared maintenance parser requires object-shaped config records and refuses
positional struct arrays while retaining valid list fields. Legacy runtime
Config::load is outside that narrow correction.

The complete [versioned contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/82717f481005bfd403ec35c149dfbbf28805d156/openspec/changes/edit-saved-basic-settings/contract.md)
defines shapes, hashes, phases and shared-host lifetime. Consumer implementation,
release publication and real-device qualification are separate evidence gates.
