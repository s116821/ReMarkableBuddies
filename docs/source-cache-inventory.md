# Source-cache metadata inventory

On Linux, select the existing nonsecret Config using REMARKABLE_BUDDIES_CONFIG
and run `reader-buddy --inspect-source-cache`. The command takes no stdin, path,
UUID, filter or pagination argument. Other platforms return unsupported before
Config/filesystem admission. Future browser and Electron host adapters may both
invoke the Linux producer; this slice does not implement their UI.

A version1 JSON report lists at most16 sorted canonical document IDs and apparent
file lengths from the existing owned normalized-sources-v1 namespace. Empty is
`ok` with [] and0; unavailable/unsupported/busy/conflict have null entries/totals.
Unknown/staging/reparse paths, invalid ownership/current format, oversized metadata
or inconsistent observations refuse without repair or a partial success list.
Reports are bounded to8192 UTF8 bytes including newline and retain both existing
leases through output. Exit0 means one complete report, including refusal; exit1
means output/transport failure. There is no initialization or Store recovery.

The observer never opens entry contents or durable history, hashes source text,
looks up original documents or contacts a provider/runtime. Empty/corrupt contents
can appear only as unvalidated metadata. Lengths are apparent bytes, not allocated
or reclaimable space; source freshness and runtime remain unverified. Inventory
grants no cleanup permission. A later [one-slot cleanup](source-cache-cleanup.md)
requires a fresh selected-slot preview/prepare and explicit original confirmation.

Ordinary CLI parse errors exit2 before startup and can repeat unexpected arguments
in stderr. That text is outside the fixed report; no private argv input is
supported. Recognized --api-key/--model/--simulate conflicts are tested to omit
supplied values. Keep the Config selector in the existing environment variable.

The central contract and paired delivery are tracked in
[Docs85](https://github.com/s116821/ReMarkableBuddiesDocs/pull/85), change
inspect-source-cache-metadata. Synthetic filesystem/CLI qualification does not
establish tablet/provider/native or installed runtime behavior.
