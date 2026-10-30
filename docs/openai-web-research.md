# Explicit OpenAI web research

The concrete `research::openai::OpenAIWebResearch` implements the existing research
Provider separately from the answering engine. It does not automatically select
itself in Reader, change saved policy or resolve credentials. Supply an explicit
model/full Responses endpoint/key and a local progress/admission callback. The
current default application model is preserved; a compatible Chat Completions
endpoint alone does not prove this search profile works.

One admitted attempt makes one POST with required live web search, explicit tool/
output limits and the exact frozen source/memory context. Redirects and transport
retries are disabled. Bounded response parsing requires completed hosted search,
reported query strings, consulted URL lists and strict per-source JSON summaries
correlated with those URLs. Missing queries, unknown/invalid evidence, incomplete
output, HTTP errors or byte/deadline exhaustion refuse; there is no empty/offline
fallback, second-call repair or model substitution.

Summaries are provider-attributed unverified assertions, not fetched quotations or
source-verified facts. Publication dates are not invented. Complete/CompleteEmpty
refer to this completed bounded attempt, not exhaustive web coverage. Existing
NoInternet and configured Disabled decisions in with_research make zero Provider
calls. All prior request/history/context bytes stay unchanged.

`TransportOptions` defaults to90seconds,1MiB request/response bodies and4,096 output
tokens; all have finite validated caps. max_tool_calls bounds all hosted calls,
while reported distinct queries are checked separately. Progress phases are
BeforeDispatch, Waiting, ResponseReceived (after worker join), and BeforeReturn.
Cancellation joins the owned async transport worker; it is not remote billing/
rollback or hard-preemption assurance for synchronous setup/JSON processing.

## Local development qualification
Build the maintained explicit example and exercise its actual entry point with
only loopback and dummy credentials (Windows adds `.exe` to the binary):

```text
cargo build --locked --example research_provider_check
python tests/fixtures/research_example_check.py --binary target/debug/examples/research_provider_check
```

The fixture checks explicit-live admission, refusing an existing output, missing
credentials, configured-endpoint admission, successful exact observation and discarding response capture after a
final composition-budget refusal. It does not qualify hosted provider access.

For a deliberately hosted check, use the existing protected
[local development credential setup](local-development.md). The example reads
OPENAI_API_KEY from its process or only the current repository's ignored `.env`;
it does not search parent projects. Its default full endpoint is the normal OpenAI
Responses endpoint; `--endpoint` is an explicit override, not an inferred conversion
of OPENAI_BASE_URL. A configured nondefault OPENAI_BASE_URL requires an explicit
full research endpoint before credential dispatch. `--model` is explicit and otherwise uses the application default.
Never place keys on the command line or publish raw response bodies.

```text
cargo run --locked --example research_provider_check -- --live --output ABSOLUTE_NEW_PRIVATE_EVIDENCE_JSON
```

This is one production POST, at most4 built-in calls,4 distinct queries,8 selected
sources and90seconds. It sends only the maintained public-authored SI-mole question
with an explicit NoDocument/disabled-memory fixture. It never contacts the tablet.
The context counter is explicitly a UTF8-byte fixture counter, not a model-window
or billed-token claim. Hosted search+strict-schema compatibility must be observed
on the exact selected endpoint/model; separate documented features are not proof.
An unavailable key or refused profile is a real failed gate, not a reason to invent
successful live research or automatically try another model.

## Optional private exchange observation
Ordinary adapter calls retain no response body. Explicit opt-in retains at most
one bounded successful response, exact sent-body SHA, response SHA and research
identity. Taking consumes it; an admitted new attempt clears stale evidence before
validation. Reentry cannot disturb the outer request. Credentials/headers/request
bytes and error bodies are not captured or logged, and payloads have no Debug.

Adapter success precedes final with_research byte/token checks. The example writes
an observation only after complete composition succeeds and discards it otherwise.
Its new absolute private file is never overwritten. Raw successful response bytes
are untrusted potentially sensitive data; keep this approved public fixture's
capture private. The maintained example's evidence includes base64 bytes and exact
hashes from the same request, without another request or HTTP interception.

The [central contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/main/openspec/specs/openai-web-research/spec.md)
records bounds, provenance and separate socket/hosted/runtime qualifications.
