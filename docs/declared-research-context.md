# Declared research policy and evidence

`FrozenSourceMemoryContext::with_research` consumes the existing frozen context and
returns `FrozenResearchContext`. Import its types from
`conversation::receiving::memory::research`. This is transient ordinary historical
data composition, not an installed Reader path, network client, model prompt/call,
configuration lookup, durable evidence write or native/effect admission. Its central
capability is `declared-research-context` in ReMarkableBuddiesDocs.

Supply `ResearchInput` with `ResearchPolicy`, `ResearchDirective`, optional borrowed
`Provider` and `ResearchOptions`. Policy defaults to Required; absence or failure
of a required provider refuses. It does not imply offline mode or empty success.
Directive defaults to UsePolicy. Explicit NoInternet dominates either policy and
produces Disabled/RequestNoInternet with zero provider calls. Otherwise policy
Disabled produces Disabled/ConfiguredDisabled, also zero calls. Skipped paths have
no dispatch identity, query, citation or basis. Their research options are unused;
complete output budgets still apply. This library does not parse natural-language
opt-outs or persist settings: future request/config resolution must supply the
authoritative directive and retain those semantics.

Required validates options: positive explicit `now_ms`,1–4 queries,1–8 sources and
256–16,384 complete encoded evidence bytes. After byte preflight, it calls the
provider once, with no retry. `ResearchRequest` exposes the entire immutable prior
context and options, plus identity binding conversation, request turn, SHA256 of
exact prior `encoded_json()` and the supplied observation time. The provider can
read the original question, history, original source/passages and memory through
existing accessors. No question truncation, rewritten turn or context summary is
substituted, including a64KiB NoDocument request with memory disabled.

Providers return an owned `ResearchResult` with exactly matching identity. Claimed
search-query strings are separate untrusted data, not replacements for the user's
question/history. CompleteEmpty has no sources/diagnostic; Complete has nonempty
sources and no diagnostic; Partial requires a typed diagnostic and may have no
sources. Each admitted called-provider result includes1–configured-max distinct
query claims. Unavailable, callback error, missing provider or invalid/contradictory
results refuse instead of becoming empty/partial success. Error displays omit
provider errors, URLs, questions, excerpts, credentials and paths.

Evidence basis is explicitly Synthetic or ProviderAsserted. The report always
labels its authority `provider-asserted-unverified`. Neither basis proves actual
network activity, source truth, source ownership, publication time, excerpt accuracy
or native/model authority. An authored synthetic Provider is valid fixture data;
it cannot qualify delivered live external research. A future live backend and
runtime consumer need their separate acquisition/privacy/authorization/model gates.

Query strings are nonblank and at most2,048UTF8 bytes. Citations have nonblank
title<=512bytes, excerpt<=4,096bytes, URL<=2,048bytes and optional positive
`published_ms` no later than observation time. Text retains ordinary CR/LF/tab but
refuses other controls. The pinned `reqwest::Url` parser checks absolute HTTPS
with a host, without userinfo/password/fragment; raw URL whitespace/controls also
refuse. Parsed-canonical URL duplicates and exact query duplicates refuse without
repair or silent merging. Accepted raw strings remain unchanged. No URL is fetched,
host resolved, credential read or new network/SSRF policy implemented by this layer.

Vector counts and individual lengths are checked before returned-data traversal or
serialization. Complete report JSON includes identity, policy/directive/status,
diagnostics, queries/citations, basis/authority and escaping, and must fit the
evidence budget. Providers supply already-allocated data: these validations do not
bound callback acquisition, allocation, blocking time or process memory/CPU.

Reuse `CombinedBudget`:512–524,288 complete encoded bytes and a positive explicit
caller token limit. An outer `research:null` frame proves only a byte lower bound
before a required call. The counter runs exactly once on the final whole encoding;
tokenization need not be monotonic and prior/component counts are not added.
Byte/token exhaustion, zero/error count or malformed evidence refuses without
dropping research, shortening text or rewriting prior context.

`source_memory_context()` retains the original object and byte-identical
`encoded_json()`, including exact request/history/source/memory and missing-media
evidence. The transient output nests the same original combined frame under
`context` and adds `research`; accessors expose immutable report data and one final
count. No Store/source/memory read or write occurs here. A callback may reenter and
mutate local data while these captured assertions remain historical; no atomic
snapshot, currentness lease or publication permission follows. Existing schemas,
IndexKeys, cache behavior and prepared evidence APIs remain unchanged.

Authored local controls use actual temporary Store/Index/LocalMemory and synthetic
research callbacks:

```sh
cargo test --locked --test source_request_context research::
```

They cover policy/default/zero calls, honest outcomes, full input/correlation,
untrusted malformed/oversized evidence, exact output/token limits, reentrant Store
mutation, sanitized failures and preserved prior bytes. They contact no network,
model, tablet or service and do not qualify live research or installed integration.
