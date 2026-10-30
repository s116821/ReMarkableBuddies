# Generation-inclusive owned-data inspection

`reader-buddy --inspect-owned-data-v3` is an exclusive offline metadata observer.
It reads the configured existing Buddy data/cache roots without Store startup,
recovery, repair, service control, provider traffic or tablet interaction. It
accepts no input. The existing `--inspect-owned-data` report1/2 route is unchanged.

The supported filesystem custody is Linux: existing descriptor-relative,
no-follow/nonblocking paths and a nonblocking existing Store lock retained
through one consuming output. Other platforms return `unsupported-platform`;
their refusal must not be presented as healthy empty storage. A busy owner is
never stopped. Out-of-band changes withhold complete totals or fail the command;
this is not an atomic filesystem snapshot.

Report3 is one compact UTF8 JSON object plus LF, at most32768 bytes. All26 keys
are present. Counts are canonical unsigned decimal strings through u64::MAX;
byte aggregates are strings or null when the exact sum exceeds u64. Null is
unknown, not zero. Consumers must validate raw framing, closed fields, numeric
spellings/ranges and relational status/totals rules, not merely JSON parsing.

Current and retained supported formats1/2/3/4 are validated independently.
Notebook archive/plan/stage metadata is a nonadditive subset of Source and
Conversation metadata. Reference totals are also nonadditive. Physical totals
count apparent file length once per owned path/generation, including hard links;
they do not describe allocated or reclaimable space. Complete requires no issues,
known totals, all categories actually inspected and zero missing/omitted/unknown
references. Partial retains observed facts; absent categories are uninspected.

Work is bounded to4096 entries/references,1MiB individual records and8MiB total
metadata reads, including repeated validation reads. Metadata can contain
protected inline text; no text is emitted. Archived source/media payloads are
only sized, not read, hashed or decoded. Credentials, original/native documents
and external diagnostics remain excluded. No logical liveness, media-content
integrity, sync, restore, feature activation or native authority is implied.

Use the full central `inspect-generation4-owned-data` report contract for the
exact schema, issue codes and consumer invariants. Both Manager hosts require
their separately coordinated report3 consumer; no automatic fallback or change
to report1/2 semantics is authorized by the new selector.
