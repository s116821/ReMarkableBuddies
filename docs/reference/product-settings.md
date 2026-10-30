# Saved role and export settings

The optional `product_settings` object uses the existing configuration schema 1:

```json
{"product_settings":{"schema_version":1,"reader_enabled":true,"writer_enabled":true,"export_destination":"mem"}}
```

Omitted settings default to Reader enabled, Writer enabled and Mem export. A missing
or null parent also uses these defaults. Other typed configuration edits preserve the new parent and leaf declarations;
effective defaults are not persisted as new choices. Explicit null leaves, unknown members,
arrays and unsupported values refuse. `export_destination` accepts only `mem` or
`disabled`; it selects a backend, not a Mem account, workspace, collection or note.
The fixed Mem origin and `MEM_API_KEY` credential selection are unchanged.

Reader disabled stops ordinary Reader startup. Explicit Writer commands are
independent of Reader enablement. Writer disabled refuses new generation,
refinement, local export preparation and external exports. Disabling the export
backend still permits local drafts and preparation, but refuses new external
exports, including joined generation/export before model work. Original-only
reconciliation remains available; it cannot dispatch a replacement create/update.
No setting deletes drafts, local data, export history or third-party notes.

`--product-settings-operation inspect` observes the explicitly selected absolute
`REMARKABLE_BUDDIES_CONFIG` file without stdin, credentials or Store startup.
`prepare`, `apply` and `status` additionally require `--product-settings-input -`.
They accept one bounded private JSON request on stdin. Prepare binds dirty leaf
Set/Clear choices to original bytes, path and operation; Apply uses the existing
conditional configuration writer once. Clear removes only the named leaf and
retains siblings. Callers must keep the original private intent for Status and
must not silently refresh, retry or reconstruct an uncertain save.

Only successful Apply reports verified disk readback and `restart-required`.
Status reports desired-file observation or unresolved state; neither proves
historical authorship or runtime activation. Maintenance remains available when
all controls are disabled so they can be re-enabled. No restart is performed by
these operations. Existing saved-settings protocols and the legacy inspector's
projection remain unchanged; the dedicated versioned response negotiates support.

The shared contract and broader remaining product requirements are maintained in
the central Docs `configure-product-settings` OpenSpec change. This control does
not add automatic Reader-to-Writer routing or qualify tablet/runtime integration.
