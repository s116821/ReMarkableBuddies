# Explicit confirmed handwriting records

`reader-buddy --handwriting` is an offline command with one strict bounded JSON
request on stdin and one JSON report on stdout. Select an existing owned configuration
with `REMARKABLE_BUDDIES_CONFIG`. It does not start a model, device or sync worker.
The first delivery is local-only correction retention, not improved recognition,
automatic learning, training or Reader/Writer/native integration.

The coordinated [central change](https://github.com/s116821/ReMarkableBuddiesDocs/pull/106)
defines confirm/update/inspect/query/forget/status fields, limits and exact report
outcomes. Commands require explicit profile/key IDs; mutations require operation IDs
and exact expected heads. Confirmation includes an explicit user-confirmed assertion,
original/corrected text and a bounded inline PNG sample. Source references are
caller assertions, not verified tablet ownership. Reports contain requested text
and sample descriptors; routine logs do not contain samples or correction text.

Use status with the same profile/correction/operation after uncertain acknowledgment.
It stages/commits nothing, although ordinary Store opening can perform recovery writes.
Missing accepted history means unknown, never rollback or automatic retry permission.
A deliberately invoked mutation is a CAS submission; identical accepted replay
returns historical data without another commit. Forget tombstones active use and
retains historical samples for complete owned backup/restore.

The command holds ordinary local ownership through actual output and excludes
existing/future legacy publishers while its report is live. Selected/shared/isolated
sync modes are unsupported. Reads validate bounded retained causal closure, including
known-key tombstones, before calling metadata a usable correction. Exhausted or
incomplete closure refuses; omitted samples are not all decoded. No supported
sample display, model consumption, learning efficacy or cross-device activation
follows from an available stored record.
