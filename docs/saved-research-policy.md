# Saved research policy

The dedicated producer deliberately edits only `research_policy` in one existing
saved configuration. Its two supported values are `required` and `disabled`.
Omission and explicit null resolve to Required, while retaining their different
declaration origins. Setting accepts only the two strings; clearing removes the
key. A no-op preserves the original file bytes. Other declarations stay intact.

The pure `Config::research_policy_snapshot()` captures a validated immutable saved
policy/default. It performs no file, environment, credential, Store or provider
lookup and does not attest saved-file bytes or runtime state. An existing snapshot
does not change when a later Config changes. The declaration retains absence/null
through typed Config serialization, including existing editor updates; absent
defaults keep their earlier encoding.

Supply that captured value directly to the existing research stage:

```rust
use remarkable_reader_buddy::conversation::receiving::memory::research::ResearchInput;

let saved = config.research_policy_snapshot()?;
let complete = frozen.with_research(
    ResearchInput { policy: saved.policy(), directive, provider, options },
    complete_budget,
    counter,
)?;
```

The caller supplies the authoritative request directive. Explicit `NoInternet`
dominates either saved policy. Disabled and request-no-internet remain distinct
zero-call outcomes; Required still needs an admitted provider or refuses honestly.
No request/history rewrite, configuration auto-reload, live research or model
dispatch follows from the snapshot. Automatic Reader/runtime consumption remains
unfinished, as does Manager policy editing.

## Deliberate saved-file workflow

Select the existing absolute file through `REMARKABLE_BUDDIES_CONFIG`; there is no
`--config` flag or default-file fallback in this interface. An explicit inspect
uses no stdin:

```sh
REMARKABLE_BUDDIES_CONFIG=/absolute/config.json reader-buddy --research-settings-operation inspect
```

For prepare/apply/status, supply bounded JSON on stdin with
`--research-settings-input -`. Other input paths are unsupported and are not opened.
The exact [central contract](https://github.com/s116821/ReMarkableBuddiesDocs/blob/main/openspec/specs/saved-research-policy/spec.md)
fixes wire versions, fields, private fingerprints and outcomes. A prepare request
uses `research_settings_version:1`, a nonnil operation, the inspected original
base/path hashes and exactly one patch:

```json
{"research_policy":{"action":"set","value":"disabled"}}
```

The alternative is `{"research_policy":{"action":"clear"}}`. Preserve the
prepared original private intent and patch; apply accepts both without refreshing
their conditions. It reuses the existing Store/mode custody and conditional
configuration writer, refuses Busy/conflict and never initializes, retargets,
stops or restarts a runtime. Inspection, preparation and status use pure value
validation without declared-root probes; apply retains full current root admission.

A verified save proves exact disk readback only and still reports
`runtime_activation:"unverified"`. A possible publication or output-prefix failure
remains unknown; do not automatically repeat it. Status accepts the original intent
and observes exact desired bytes or unresolved, without another apply, historical
authorship claim or settlement of earlier uncertainty.

Requests and reports are capped at 16,384 bytes, config files at 1,048,576 bytes.
Reports expose only finite policy/origin values and private workflow digests; they
do not echo unrelated configuration or credentials. The existing broad saved-config
v1 view keeps its 25 fields and unsupported-view list unchanged: its research-policy
entry means that view does not expose policy, rather than denying this dedicated
interface. Older binaries can refuse the new field/command; support is not inferred
from equal schema/version strings.

Fixtures use real temporary config/Store/CLI data, all saved declaration states,
existing editors, custody/faults, and real source/memory frames with authored
synthetic research. They contact no tablet, model, provider or service and establish
local saved-policy/composition behavior only.
