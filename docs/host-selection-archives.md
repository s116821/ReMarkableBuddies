# Selected archive commands

These explicit offline commands preserve or inspect historical archive custody.
They do not restore data, activate membership, contact a tablet/provider, stop a
service, or start Reader/Writer/sync workers.

```text
reader-buddy --archive-operation export-selected --archive-input ABSOLUTE_JSON_FILE
reader-buddy --archive-operation inspect --archive-input ABSOLUTE_JSON_FILE
reader-buddy --archive-operation inspect --archive-input -
```

Both operations accept an absolute regular input file or exact dash stdin. Input
is one strict UTF8 JSON object, at most16384 bytes including whitespace:

```json
{"path":"ABSOLUTE_ARCHIVE_DIRECTORY"}
```

The example specifies a shape; replace the path with a real absolute path. Unknown
keys, relative paths, symlinks/reparse points, unsafe roots, nonregular input files
and oversized input refuse. Stdin requires EOF; callers own their deadline and
cancellation. Runtime/model/logging/simulation and other one-shot options cannot
be combined. The route runs before dotenv and runtime initialization.

For export-selected, set REMARKABLE_BUDDIES_CONFIG to a valid supported config
with absolute existing owned roots and legacy sync disabled. The path is a **new**
destination outside all data/cache/credential roots. The command never initializes
or repairs source storage, bypasses its OS lock, overwrites a destination or falls
back to ordinary export. A running Store owner returns Busy. Source selected
winner/history/retained evidence/media and unrelated committed scopes are copied
using the existing bounded format2 archive policy; configuration, credentials,
cache and runtime trees are excluded. Current mode is held during export and
output; any absent/local/shared/isolated choice permits historical custody only.
A chosen mode does not verify a remote binding or grant restore authority.

For inspect, path is an existing archive. No config or live Store is needed or
opened. Existing format1 and selected format2 archives are validated in full,
including exact referenced object/history bytes. Unknown, corrupt or incomplete
archives refuse. Valid format1 configuration presence is reported as a boolean;
the command does not expose its values or endorse that configuration as portable.
Inspection describes validated historical custody, not continuing file freshness.

## Single report

Commands return one JSON line, at most16384 bytes, with:

- report_version:1; operation:inspect or export-selected.
- status:ok, invalid-input, unavailable, unsupported, busy or failed.
- incomplete:boolean; activation_performed:false.
- data:null, or {inspection:{format,manifests,objects,selected_scopes,
  selection_receipts,complete_media,configuration_included}}.

Inspection fields have the existing BackupInspection types: format is an integer,
counts are nonnegative integers, and the last two fields are booleans. No record
contents, media bytes, paths, raw errors, credentials, live tokens or current
native/group authority are returned. An intentionally omitted medium gives
status:ok, complete_media:false and incomplete:true. Missing promised bytes refuse
the archive rather than converting corruption to a partial successful report.
Unavailable/failed also set incomplete:true; other refusals return null data and
incomplete:false. Accept data only for status:ok and supported report versions;
partial/malformed/oversized output is not empty success.

## Interruptions and uncertainty

backup.json is published only after existing archive closure verification. A
failure can occur after that marker is committed or after only a report prefix is
written. A failed/missing response therefore does **not** prove no archive exists.
The command never automatically retries export, removes partial output or appends
a replacement report. Deliberately inspect the **original destination** to learn
whether it is a complete verified archive. Partial directories remain for manual
reconciliation; do not silently reuse them. A genuinely new attempt requires an
explicit new destination. Existing selected restore remains unsupported.

Export retains source Store/process and current-mode lifetimes through the one
consuming report write or failure cleanup. Archive inspection acquires no Store
authority. These host results do not qualify actual-device transfer, recovery,
native pages, remote synchronization or live model integration.
