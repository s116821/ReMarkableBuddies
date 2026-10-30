# Saved storage mode maintenance

These commands inspect or record a local mode choice. They do not start sync,
verify a provider binding, join/bootstrap a remote group, restore data, stop a
service, or contact a tablet. A chosen group/root ID is metadata, not proof of
cloud readiness or native authority.

Set `REMARKABLE_BUDDIES_CONFIG` to the supported absolute configuration file.
Use valid absolute Buddy-owned data/cache/credential roots. Initialization and
choice require legacy sync disabled. An active Store owner returns Busy; the
command never stops it or bypasses its lock.

```text
reader-buddy --sync-mode-operation initialize
reader-buddy --sync-mode-operation inspect
reader-buddy --sync-mode-operation choose --sync-mode-input ABSOLUTE_JSON_FILE
reader-buddy --sync-mode-operation choose --sync-mode-input -
```

Initialize explicitly permits normal owned Store creation/recovery, including
owned structural markers/staging cleanup. It does not choose a mode and is not
filesystem-immutable inspection. Inspect/choose require an existing valid Store;
they do not initialize/repair its structure or clean staging. Selected format2
is supported by a dedicated maintenance path; unrelated legacy maintenance keeps
its prior selected-format refusal.

Only choose accepts input. It is one strict UTF8 JSON object with exactly
`operation`, `expected` and `choice`. Supply a new nonnil UUID for a new operation,
and copy the original `data.current_snapshot.token` object unchanged from inspect
or initialize into `expected` (its fields are `actor` and `sha256`). The choices are:

```json
{"kind":"local-only"}
```

```text
{"kind":"isolated-backup","binding_sha256":"64 LOWERCASE HEX DIGEST"}
{"kind":"shared-group","group":"NONNIL GROUP UUID","binding_sha256":"64 LOWERCASE HEX DIGEST","root_id":"EXPLICIT ROOT ID"}
```

The latter examples show field shapes, not literal requests. Choice IDs and
digests must satisfy the existing Store contract. Unknown fields (including extra
fields on local-only), nil IDs, malformed JSON and invalid choices refuse.
Do not send credentials. The entire file/stdin input is at most16384 bytes,
including whitespace. Stdin needs EOF; the caller owns its deadline/cancellation.
Relative paths, symlinks and nonregular file inputs refuse.

Retain an uncertain operation's original UUID, expected token and choice. A
deliberate exact retry returns the original receipt without a new commit. Do not
replace the expected token automatically or reuse an operation UUID for changed
input. For a genuinely new choice after a conflict, inspect/review current state
and submit a new operation. An I/O failure can follow commit, so absence of a
successful response does not prove nothing changed.

## Report-v1 contract

One bounded JSON line (at most16384 bytes) has exactly:

- `report_version`:1; `operation`:initialize, inspect or choose.
- `status`:ok, invalid-input, unsupported, unavailable, busy, conflict or failed.
- `incomplete`:boolean; `activation_performed`:false.
- `data`:null, or an object with `current_snapshot`, `receipt`, `receipt_status`.

`current_snapshot` contains `device`, `epoch`, `mode` and `token`. Device/epoch/mode
are null when no explicit mode is chosen. That is unconfigured legacy policy,
not an explicit LocalOnly choice or proof a worker is inactive. A chosen mode has
the existing typed `kind` and its identity/binding fields. Token has nonnil actor
UUID and a64lowercase-hex conditional digest; it is not a live selection/native
capability. No credentials or runtime configuration values are returned.

Inspect/initialize have null receipt/status. Choose may return the exact original
Store receipt and `receipt_status:current` only if its epoch AND mode match the
actual snapshot. An old exact retry after a later choice returns
`receipt_status:historical`, preserving the old receipt and the newer actual
snapshot. It never claims the old mode is active. Errors can have null data;
consumers must preserve the top-level status instead of substituting LocalOnly,
empty state or success. Inspect consumers should accept data only from an `ok`
report and reject unsupported versions, malformed/oversized or partial output.

The command holds the Store/process and current-mode leases through a single
consuming write. A prefix-write failure never appends another report or retries
the mutation. The normal ordinary Reader separately holds ordinary-mode and
legacy-data guards through its run loop. It refuses shared/isolated mode or
selected data before config snapshots and model/device/domain construction;
LocalOnly with a legacy publisher enabled also refuses. Choosing LocalOnly does
not convert selected history into ordinary history or erase it.

This delivery does not activate the separate selected Reader integration or waive
live-model/native requirements. Older binaries may reject these flags; that is an
unsupported capability, not an unconfigured mode or permission to inspect private
mode files through a fallback protocol. App versions do not imply wire compatibility.
