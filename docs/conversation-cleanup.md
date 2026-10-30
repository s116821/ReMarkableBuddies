# Ordinary historical conversation cleanup

The Linux-only `--conversation-operation inspect|prepare|apply|status` interface
logically deletes one explicitly selected ordinary schema-one conversation.
It preserves all source images, historical records, export associations, original
PDF/EPUB/notebook files and native pages. It does not reclaim disk space, delete
remote notes, synchronize deletion, or establish native/runtime qualification.

Use the existing explicit `REMARKABLE_BUDDIES_CONFIG` absolute configuration path
and `--conversation-input -`. Input is strict JSON on stdin plus EOF, at most16KiB;
the CLI never accepts an arbitrary input/deletion path. Reports are closed JSON,
at most32KiB. Ordinary parser diagnostics may echo unexpected command-line arguments;
never put private values into unsupported arguments. Recognized conflicting modes
refuse before normal application startup.

Inspect body: `{"request_version":1,"conversation":"CANONICAL-NONNIL-UUID"}`.
Prepare adds `"operation":"FRESH-CANONICAL-NONNIL-UUID"`. Save its complete original
private intent. Apply/status body is `{"intent": ORIGINAL_INTENT}`. The UUIDs in
these shapes are placeholders, not runnable example identities or discovery rules.
A preview grants no deletion authority. Never substitute a newly prepared intent
for an uncertain original operation.

Admission requires the fixed `reader-buddy.service` to be observed loaded, inactive,
dead and MainPID0 using bounded read-only OS properties, and an existing stable
Store lease. The producer never changes service state. Missing/active/unknown
service or a competing writer refuses. Non-Linux refuses before stdin/storage/OS
access; a Windows host must not interpret that response as local cleanup support.
Existing ordinary absent/LocalOnly mode is required; sync/selected history refuses.

The target must have complete valid ordinary history and settled exchanges.
An interpreted user request remains unchanged and needs one later terminal assistant
in its exact exchange, with the required completion/outcome evidence. Pending,
ambiguous, unsupported, selected/schema-two or uncertain history refuses. The
existing-only recovery validates owned committed history/media within conservative
inventory and cumulative read/work limits. Large stores may refuse even for a
small target. Staging is retained; no repair or cleanup is performed to fit limits.

Prepare/first apply reserve room for final and interrupted publication plus later
reconciliation. Publication atomically writes root/optional historical-binding
tombstones and an exact operation receipt. An exact historical receipt can remain
acknowledged while current state is `changed`; that never authorizes resurrection.
After a possible publication or output failure, use explicit status with the SAME
original intent. Partial original objects without a complete receipt remain
unresolved and cannot trigger another apply. No automatic retry or rollback occurs.

Source/spec implementation and synthetic fixtures are separate from real Linux
unit, tablet, provider and integrated qualification. This guide does not claim any
of those runtime gates have been executed. Complete normative wire/budget rules
live in the paired central Docs change `prepare-historical-conversation-deletion`.
