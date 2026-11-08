# Historical source targets

A source target records the inferred subject and explicit bounded region against
an exact retained image and coordinate frame. `About:` describes that inference;
it does not confirm handwriting. An uncertain target cannot supply regional Writer
input. Typed follow-ups inherit the exact target revision without adding another
About entry. Subject correction retains the target scope; retargeting creates a
new scope and retains the previous history and images.

The source-target feature uses domain schema4 in storage family6, with an immutable
ordinary-notebook or compound-selected base. Startup preparation preserves the
original actor and generation. Legacy sync stays on its existing route. Older
storage readers must refuse this family. No record or command grants native page
mutation authority.

## Stored-target Writer

The existing `--writer-note` command accepts bounded strict JSON on stdin:

```json
{
  "command": "generate-target",
  "conversation": "<conversation UUID>",
  "operation": "<new deliberate operation UUID>",
  "expected_root": ["<current root revision UUID>"],
  "target": {
    "namespace": "source",
    "record": "<stored target UUID>",
    "revision": "<exact target revision UUID>",
    "object": {"sha256": "<exact envelope SHA-256>", "bytes": 1234}
  },
  "request": "Make study notes from this selected region",
  "guidance": null
}
```

The placeholders must be replaced with exact stored identities. The command
resolves retained original bytes, masks outside the proven region to white on the
unchanged canvas and calls the existing complete-image handwriting normalization.
Only the resulting regional source units enter note generation. A durable admission
precedes recognition; an uncertain earlier operation must be reconciled using its
original records, never repeated with a replacement UUID.

Existing `refine`, `prepare-export` and deliberate export-draft flows retain the
saved regional source. Its export scope is
`normalized-document:<historical document UUID>:target:<stable target UUID>`.
The export revision also binds the target revision, image recipe, normalization
and draft. A different target has a different scope even when its image bytes
match. The existing export backend and correlation rules continue to apply.

## Explicit target changes

Use `--conversation-operation target-change --conversation-input -` for a deliberate
ordinary historical subject correction or bounded retarget. Strict stdin JSON has
`operation`, `conversation`, `user` (three distinct nonnil UUIDs), `expected_root`
(one exact root revision), `text` (the literal typed intent) and `change`.

A subject change is
`{"action":"correct-subject","target":<exact reference above>,"subject":"Corrected subject"}`.
A retarget is
`{"action":"retarget","source":"<retained capture UUID>","candidate":<explicit target candidate>}`.
A candidate includes `subject`, canonical `geometry.components` and
`understanding.status` (`interpreted`, or `uncertain` with `reason`). Coordinates
use the original full frame and integer edge scale1,000,000. A point alone cannot
supply a region. Subject changes never update the handwriting profile.

These explicit commands are historical data operations. Native composer Submit,
scope gestures, selected regional Writer ingress and supported-device presentation
remain separate integration and acceptance gates. Scripted/localhost results do
not establish live recognition quality or native display behavior.
