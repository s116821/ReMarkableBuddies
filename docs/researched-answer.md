# Answer a frozen source, memory and research context

`conversation::receiving::answer::answer` is an explicit ordinary model consumer.
Supply an existing `FrozenResearchContext`, an exclusively borrowed `LLMEngine`,
`AnswerOptions`, a counter and progress callback. It invokes the engine once and
returns a bounded transient `AnswerDraft`. The normal tablet workflow does not
automatically select it. No Store record, source, configuration or native page is
changed by this call.

The engine receives two text parts: stable buddy instructions and the exact
complete frozen JSON. Original question/history/evidence are not rewritten or
reacquired. These are separate text parts, not separate system/developer roles.
The existing trait cannot express those roles. No image is loaded; media references
remain historical metadata and do not mean the model can see the image.

Complete ordered text-parts JSON is bounded and counted once before execution.
That descriptor includes instructions/escaping but does not promise to count every
engine's HTTP overhead or establish a model context window. Earlier component
counts are not added. Default limits are1MiB input,128000 caller-counted tokens,
64KiB returned text,16KiB answer text,32 citations and128KiB complete result.
The engine separately owns its transport/output-token limits.

The call clears prior engine text/images at entry and clears again on success,
refusal, errors and unwinding. Reuse starts empty rather than restoring an earlier
prompt. The selected engine must implement `clear_content` correctly without
panicking; this is not proof of physical/remote erasure or malicious-engine safety.
Callbacks are BeforeDispatch, EngineProgress, AfterResponse and BeforeReturn.
An observed callback failure is latched even if an engine ignores it. The generic
trait's default progress implementation calls only before execution: no generic
hard preemption or worker-lifetime guarantee follows. The existing OpenAI engine
has its own bounded cooperative transport and joined cleanup.

## Generated output and references

The model must return exactly `status`, `text` and `citations` in one strict JSON
object. Status is `answered` or `clarification`; a clarification has no citations.
Duplicate keys, extra fields, malformed JSON, empty/NONE text, controls or bounds
violations refuse. No second call repairs malformed output.

| ID | Captured metadata resolved by code |
| --- | --- |
| S1, S2... | Retrieved passage's original index key, unit/page/section and UTF8 range |
| M1, M2... | Associative fact record/revision and original asserted provenance |
| R1, R2... | Research URL/title and original Synthetic/ProviderAsserted basis |

The response lists unique existing IDs and uses matching inline markers `[S1]`,
`[M1]`, `[R1]`. The distinct marker set must match the listed IDs exactly; repeated
valid inline markers are allowed. IDs have no signs, zero or leading zeros.

The reserved scanner begins at `[` followed by uppercase S/M/R and immediately a
digit or sign. Thus `[S0]`, `[R-1]`, `[S1 x]`, unknown `[R999]` and an unterminated
`[S1` refuse. Ordinary `[Source]`, `[R, S]`, `[S x]`, `[S]` and interval brackets
remain prose/math. A malformed ID in the citations array always refuses, even if
its spelling is not a reserved inline marker. Lowercase prose is not silently
promoted to a citation.

Metadata comes only from captured arrays, never generated URL/title/locator fields.
Source text is declared historical text; memory remains author-asserted; research
remains synthetic or provider-asserted/unverified. The draft is an unverified
generated historical answer. Structural admission does not prove factual truth,
exhaustive attribution or that every instruction was followed. An answer can have
no citations; a caller must assess meaningful use of evidence. Research URLs can
be rendered as clickable links by the eventual presentation consumer.

## Development checks

Offline actual-engine/example checks use loopback and dummy credentials:

```text
cargo test --locked --test source_request_context answer::
cargo build --locked --example researched_answer_check
python tests/fixtures/researched_answer_example_check.py --binary target/debug/examples/researched_answer_check.exe
```

The maintained explicit caller constructs real temporary Store/Ledger/Index/memory
data: an authored SI-mole statement, a deliberately wrong older associative note
and **synthetic research**, then calls the existing OpenAI answering engine. Its
tiny synthetic image is only a historical source marker, never handwriting/native
evidence, and is not supplied to the model. The caller checks expected numerical
facts and captured references; inspect the private answer for semantic meaning.

An authorized actual hosted answer is separate from those loopback controls:

```text
cargo build --locked --example researched_answer_check
path/to/researched_answer_check --live --output ABSOLUTE_NEW_PRIVATE_FILE --timeout-seconds 90
```

Run the already-built executable from the approved protected credential directory
when that is where the project's development setup lives. It loads only current-
directory `.env`, never searches parents or copies keys. Existing output is never
overwritten (Unix mode0600); use a protected private directory on Windows. Without
`--live`, no credential lookup/request occurs. A nondefault configured base refuses
without deliberate `--base-url`; allowed bases are HTTPS host-only or numeric
loopback fixtures, and the existing engine appends `/v1/chat/completions`.

There is at most one engine invocation, with no added retry/model substitution.
This is not a promise about an arbitrary engine's internal network retries/billing.
Private evidence retains complete authored context, prompt/response hashes, bounded
returned model text and the admitted draft or finite failure. A successful engine
return is not necessarily a valid draft or successful representative assertion.
It captures no HTTP headers/error bodies/credentials and prints no payload.

This fixture qualifies answering of declared input, not a new hosted acquisition.
Do not relabel its synthetic research as fetched/verified evidence or replay a
closed acquisition test. A failed hosted result remains a real qualification gate;
retain it without automatic repair/fallback. Installed runtime, selected/native
and whole-MVP acceptance remain separate.
