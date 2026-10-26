# Explicit confirmed-context transcription

`reader-buddy --handwriting-transcription` consumes one bounded strict stdin JSON request. It calls the configured provider once at most and may send the target plus up to three explicitly selected confirmed sample/text pairs. It never confirms, trains, saves a source or starts the tablet workflow. This operation incurs the configured provider's normal costs when invoked; input/context refusal makes no call.

Select configuration with `REMARKABLE_BUDDIES_CONFIG`. Use a protected saved credential reference or process environment, never a key in JSON or command arguments. All other CLI flags conflict with this exclusive command.

```json
{"transcription_version":1,"invocation":"00000000-0000-4000-8000-000000000001","document":"00000000-0000-4000-8000-000000000002","page":1,"target_png":"<standard padded base64 PNG>","context":null}
```

For selected context replace null with `{ "profile": "<canonical nonnil UUID>", "examples": [{ "correction": "<UUID>", "revision": "<exact current revision UUID>" }] }`. One to three distinct keys are permitted in caller order. Capture refuses stale/conflicted/deleted/unsupported/unavailable samples with no no-context fallback. Samples additionally require complete static PNG framing/CRC/IEND and no APNG/trailing bytes; retained record validity is unchanged by this consumer's stricter eligibility.

Target bound:8MiB,4096 per side,8M pixels. Each example:512KiB,2048 per side,4,194,304 pixels. stdin12MiB; aggregate9.5MiB compressed/20,582,912 pixels decoded sequentially. No image transformation occurs. Output is one at-most32KiB report; result is an unverified normalized historical page or explicit unreadability. confirmed is always false. No-context opens no Store. With selected context the Store/mode ownership spans actual output/error/drop, including context refusal after ownership acquisition.

The caller invocation UUID is correlation only. Repeating it explicitly makes another provider call; there is no durable replay receipt or automatic retry. provider_attempt=called means the engine method was entered, not proof of HTTP acceptance. Failure/cancellation may leave remote outcome unknown. Routine logs/errors contain fixed codes, not request/sample text, images or private paths. The report itself may contain unconfirmed text and should be handled as content.

Arbitrary library engines are cooperative. The configured OpenAI-compatible transport has its existing progress-aware owned cancellation/join path; the CLI uses the existing Writer-style joined one-shot signal pattern. No claim of forced interruption of arbitrary engines follows. An unrecoverable worker failure emits a sanitized process error without inventing a dispatch receipt.

The versioned central [proposed contract](https://github.com/s116821/ReMarkableBuddiesDocs/tree/docs/rem-26-explicit-handwriting-transcription/openspec/changes/transcribe-with-confirmed-handwriting-context) fixes exact report/normalization/profile recipes. Existing default notebook normalization is preserved; revision-only context changes alter extractor identity while unchanged prompt bytes keep the same digest. Synthetic tests establish these mechanisms only. Representative model evidence, future Reader/Writer hooks, efficacy and native/sync acceptance remain separate gates.
