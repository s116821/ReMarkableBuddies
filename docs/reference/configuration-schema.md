# Offline configuration schema discovery

Run `reader-buddy --configuration-schema` to emit the embedded current configuration
object schema as UTF8 JSON followed by LF. The selector is exclusive: it accepts no
other mode, runtime override or input selector. It does not consume stdin, select a
configuration file, load `.env`, resolve credentials, open Store or start services.
Its output is at most65,536 bytes. An older executable can refuse the unknown flag;
do not substitute its historical storage schema or infer support from an app tag.

The document uses JSON Schema2020-12 and the independent identity
`urn:remarkable-buddies:configuration-object-schema:v1`. This is a document revision,
not a new runtime configuration or storage version. Runtime `config_schema_version`
remains1. Legacy storage contracts1/2/3 and their capabilities remain unchanged.
No separate installed schema file, running query service or schema engine is needed.

The root object has thirteen supported properties: config_schema_version, model,
base_url, trigger_corner, log_level, debug_dump, model_credential, paths, sync,
memory, personalization, research_policy and product_settings. Unknown properties
refuse. Whole objects may be omitted where Config supplies defaults; a present
paths object still requires data, cache and credentials. Ordinary optional values
and research/product parents accept null, while paths/sync/memory/personalization
parents do not. Product settings accept null/empty/version-only/partial parents,
with true/true/mem effective defaults and non-null declared leaves. Defaults are
annotations only: consumers must not insert them as new saved choices.

This schema describes the canonical object form supported by saved inspection.
The legacy loader's permissive Serde positional-array spellings are unchanged and
are outside that form. Schema success is not complete configuration admission:

- Original-byte parsing still enforces duplicate-member and lexical integer rules;
  a JSON Schema integer can accept numeric1.0 where the Rust integer parser refuses.
- `maxLength` counts Unicode code points; Rust limits relevant strings by UTF8
  bytes. Trimmed nonempty checks and complete UUID/nil/identity validation remain
  typed checks. Compact, uppercase, URN and braced UUID forms must not be rejected
  merely because a generic validator assumes a narrower UUID-format annotation.
- Memory profile/selection, sync collection and mode relationships remain typed
  semantic admission. Absolute, nonoverlapping, owned, symlink-safe roots and
  credential permissions require actual filesystem checks.
- Credential source precedence, provider connectivity and runtime activation are
  separate. This command neither reads secrets nor verifies or changes live state.

For development, `scripts/check_configuration_schema.mjs` replays the actual CLI
document through an existing Ajv Draft2020-12 module and the shared40-case corpus:
pass `--validator /existing/ajv/dist/2020.js --binary /built/reader-buddy` to Node.
It validates the meta-schema, checks expected structural outcomes and proves that
defaults do not mutate inputs. Rust separately checks original corpus bytes through
the typed saved inspector, including deliberately different parser/semantic results.
The development checker is not a runtime dependency or an installer.
