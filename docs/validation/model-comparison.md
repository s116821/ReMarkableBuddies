# Bounded handwriting model comparison

September 2026. Five immutable RM2 captures were each sent once to three models
using the same corrected topic-selection prompt, overview and native detail strips.
No answers were written during this comparison. This is a small integration sample,
not an accuracy benchmark or a guarantee for arbitrary handwriting.

| Input | GPT-4o | GPT-5.4 (2026-03-05) | GPT-5.6 Terra |
| --- | --- | --- | --- |
| Joined `G unc.?` | Correct number/uncertainty | Correct, plus derived relative uncertainty | Correct, about 14 ppm |
| Clear rotation question | Shortened transcription; inaccurate question coordinates | Incorrect rejection | Correct transcription and relevant explanation |
| Messy `why rot. attr.?` | Confident misreading | Confident misreading | Confident misreading |
| No annotations | Invented question | NONE | NONE |
| Illegible scribble | Invented question | NONE | NONE |

| Model | Mean API latency | Mean estimated standard API cost |
| --- | --- | --- |
| GPT-4o | 5.85 s | $0.01232 |
| GPT-5.4 snapshot | 3.75 s | $0.01375 |
| GPT-5.6 Terra | 5.25 s | $0.01321 |

Costs use returned input/output token counts, including reasoning tokens, and the
published rates below. All five comparison requests per model reported zero cached
input tokens. These estimates exclude taxes and account-specific pricing; individual
requests and latency vary. The selected approach also needs an independent question
reading before writing an answer: the first verified clear-question run added 406
input and 13 output tokens (about $0.00097 at Terra rates) and about 2.95 s.

Terra is the strongest candidate in this limited set at roughly comparable cost
and is now the saved candidate's default. A model switch alone does not fix
ambiguous handwriting. Subsequent live verification rejected the difficult
misreading and accepted a fresh joined `why rotate?`, but falsely rejected
`why flat plate?` after the second reading said `why that plate?`. Independent
verification can also take longer and cost more on ambiguous inputs; the difficult
rotation check took about 9.2 seconds and 522 output tokens, including 512 reasoning
tokens. See the [current installed-build status](2026-09-15-status.md). The final
installed build now passes the live positive, negative and append cases documented there.

## Official API facts checked

### Independent-reading context repair

The narrow question-band verification subsequently rejected valid `G unc.?` on
hardware, even after clarifying that fragments were allowed. A bounded comparison
then used the same five saved captures, the independent transcription-only prompt,
and the full overview/detail images. It correctly transcribed `G unc.?` and
`WHY ROTATE ATTRACTORS?`, and returned NONE for the difficult rotation shorthand,
scribble and absent-question fixtures. No first-pass proposal was supplied.
The final implementation therefore preserves page context in the independent pass.

This comparison averaged 7.07 seconds and an estimated $0.01113 per independent
request at the listed rates (zero cached tokens). The extra image input materially
increases verification cost compared with the narrow-band check. A recognized
question normally makes two requests; absent questions declined in the first pass
make one. Costs vary, and ambiguous reasoning can take longer. These are bounded
sample measurements, not an accuracy or billing guarantee. Raw comparison records
are retained under `evidence/*-independent-context.json`.

Adding the separately measured means gives roughly $0.02434 for a two-pass Terra
question versus $0.01232 for the single-call GPT-4o benchmark: approximately twice
the cost, not nearly identical full-workflow pricing. This is an illustrative
estimate, since first-pass rejection skips verification and individual requests
vary. The user approved the tested Terra default; no reset/capacity credits were
purchased or redeemed.

### Model availability and rates

- [GPT-4o](https://developers.openai.com/api/docs/models/gpt-4o) remains listed for
  text/image API input, at $2.50 per million input tokens and $10 per million output
  tokens. Its successful live calls also confirm access for this test account.
- [GPT-5.4](https://developers.openai.com/api/docs/models/gpt-5.4) is $2.50/$15;
  [GPT-5.6 Terra](https://developers.openai.com/api/docs/models/gpt-5.6-terra) is
  $2/$12. Terra is documented as balancing intelligence and cost, roughly the mini
  tier of earlier GPT-5 families. This report does not call it a designated GPT-4o
  successor; selection is based on this workload and observed results.
- The [API deprecation list](https://developers.openai.com/api/docs/deprecations)
  distinguishes `chatgpt-4o-latest` (removed February 17, 2026) from `gpt-4o` and the
  older `gpt-4o-2024-05-13` snapshot (scheduled October 23, 2026). A ChatGPT retirement
  is not evidence that the separate GPT-4o API endpoint was retired.
- The existing Chat Completions integration now uses the documented
  [`max_completion_tokens`](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
  parameter. Internet search and additional providers are outside this repair.

The account's model list and actual comparison calls confirmed access to all three
models. No API key was printed or stored in the comparison artifacts. Requested
models, latency, returned usage and full text responses are retained in the local
model-comparison output bundle. The [vision probe](../../examples/vision_probe.rs)
captures immutable inputs and evaluates them without changing the document.
