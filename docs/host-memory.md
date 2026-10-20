# Offline memory maintenance

These explicit commands operate on Buddy-owned host data without starting Reader,
a model client, a device adapter or a sync worker. Set `REMARKABLE_BUDDIES_CONFIG`
to an absolute regular configuration file. Configure absolute `paths.data`,
`paths.cache` and `paths.credentials`, an explicit nonnil `memory.profile` UUID,
and disable legacy sync. Keep personal documents outside these owned paths.

```text
reader-buddy --memory-operation put --memory-input ABSOLUTE_INPUT_JSON
reader-buddy --memory-operation inspect --memory-input ABSOLUTE_KEY_JSON
reader-buddy --memory-operation query --memory-input -
reader-buddy --memory-operation forget --memory-input ABSOLUTE_FORGET_JSON
```

`put` accepts `{operation,expected,fact}`. `operation` is a nonnil UUID for that
exact mutation; `expected` is the exact set of current revision UUIDs (empty for
a new fact). `fact` contains `key:{profile,scope:{kind:"global"},fact}`, `claim`,
`value`, `subject`, `tags`, `provenance`, `confidence_basis_points`, `observed_ms`
and optional `expires_ms`. A document scope is
`{kind:"document",document:UUID}`. Provenance is an assertion, not independently
verified evidence; it contains `basis` (`user-confirmed`, `source-assertion` or
`inferred`), `description` and nullable `conversation`, `turn`, `source` UUIDs.
Keep identity and claim unchanged when updating. Unknown or invalid fields refuse.

`inspect` accepts the fact key and returns revision envelopes. Preserve all current
revision IDs when preparing an update or `forget:{operation,expected,key}`.
Forgetting records a tombstone and does not erase original documents. Exact retries
retain their original receipt; do not reuse an operation UUID for changed input.
Conflicts and uncertain output require inspection, not automatic mutation replay.

Selected maintenance additionally requires explicit `memory.selection` identity
(`group`, `profile`, `binding_sha256`) matching an already selected SharedGroup
storage mode. The CLI does not choose that mode, create a cloud binding or start
sync. It refuses foreign/isolated modes and does not fall back to local memory.

`initialize-aggregate` accepts `{operation,accepted_base_sha256,fact}` and returns
an explicit registration. `initialize-selection` accepts
`{operation,accepted_base_sha256,registrations}`. `register-selection` accepts
`{operation,registration}`. These are explicit domain setup operations: a saved
descriptor alone does not grant current membership or native authority. An orphan
aggregate remains absent from queries until explicitly registered.

All operations return one bounded report-v1 JSON line with truthful status. Missing
stores are not created by queries, inspections or forgetting. Valid Store startup
may recover owned state. A Busy store is never stopped or unlocked by this CLI.
File inputs are bounded absolute regular files; only query supports stdin. See
[query input, EOF, output and cancellation](memory-query-stdin.md) for that contract.
Mode and selected-currentness guards last through actual output; partial I/O errors
never produce a second report. This offline interface does not activate optional
memory in the normal Reader loop or qualify live-model/native behavior.
