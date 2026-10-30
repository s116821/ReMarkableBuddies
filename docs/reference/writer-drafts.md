# Generate and refine an unrendered Writer draft

`reader-buddy --writer-note` reads one JSON object from stdin, at most 256 KiB.
It consumes explicitly supplied normalized text; it does not extract handwriting,
capture drawings, start device input, render a page or send a Mem note. The
selected conversation must already exist in the owned Store. Installed
Reader/Writer module admission and native interaction remain separate work.

Use the existing `REMARKABLE_BUDDIES_CONFIG` file and protected model credential
reference or `OPENAI_API_KEY`. Endpoint selection follows ordinary configuration
and `OPENAI_BASE_URL`. No credential argument or automatic `.env` loading is
added. The command resolves model credentials before opening/changing the Store;
export preparation needs no model credential. Store ownership is still exclusive.

## Generate from supplied text

These are invented example identities and text. Document identity/digest and
extractor revision are supplied provenance, not proof of original-byte retention
or successful extraction:

```json
{
  "command": "generate",
  "conversation": "10000000-0000-4000-8000-000000000001",
  "operation": "20000000-0000-4000-8000-000000000002",
  "request": "Make study notes, preserving x = 7 exactly.",
  "document": {
    "document": "30000000-0000-4000-8000-000000000003",
    "format": "notebook",
    "source_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "extractor_revision": "supplied-text-v1",
    "units": [{"page": 1, "section": null, "text": "Equation: x = 7."}]
  },
  "selection": {"scope": "page", "page": 1},
  "guidance": "Keep useful page references."
}
```

Use `{"scope":"supplied-notebook"}` to select every supplied unit in its given
order. This does not prove the physical notebook was completely extracted. Page
selection includes every supplied unit with that page label; section/page labels
are retained as given. Explicit guidance is separate from note content; ordinary
text is never autonomously classified as handwriting instructions.

The complete encoded document/guidance input is at most 40 KiB; its durable typed
source record is at most 48 KiB. Requests are nonempty, at most 4096 UTF-8 bytes;
guidance is at most 4096 bytes. History preserves at most 64 eligible turns and
16 KiB exact text; excess refuses with `too-large`. There is no silent truncation
or relevance-hit substitution for full supplied-unit coverage. Large-notebook
chunk/reduction remains unfinished. The encoded provider payload is at most
96 KiB, and proposed Markdown at most 24 KiB. These are local byte/work limits,
not proof that any selected provider has a particular token capacity.

The existing provider receives separate task/preference/data content parts. Its
response must start `# Title`, followed by a blank line and nonempty Markdown.
Title length is at most 256 bytes, with no control characters or `[RMB:]` token.
Malformed, missing, oversized or failed replies do not become generated drafts.
Prompt instructions and fixture responses do not establish model quality.

## Refine a selected draft in the same conversation

```json
{
  "command": "refine",
  "conversation": "10000000-0000-4000-8000-000000000001",
  "operation": "40000000-0000-4000-8000-000000000004",
  "request": "Keep the equation and remove the suggested deadline.",
  "draft": "50000000-0000-4000-8000-000000000005"
}
```

Use the actual returned draft identity. Refinement preserves the original source,
eligible exact Reader/Writer request/history and selected draft. It creates new
user/draft turns and a correction link; it does not edit old text or enable
personalization learning automatically. User request text remains verbatim.
Drafts have `Generated` status, never fabricated native `Completed` evidence.
Ordinary context still excludes unrendered drafts; the selected draft is supplied
explicitly. No new conversation, tablet page or rendering follows refinement.

## Deliberately prepare an export payload

```json
{
  "command": "prepare-export",
  "conversation": "10000000-0000-4000-8000-000000000001",
  "draft": "50000000-0000-4000-8000-000000000005",
  "correlation": "60000000-0000-4000-8000-000000000006",
  "effect_operation": "70000000-0000-4000-8000-000000000007"
}
```

The result's `prepared_note` is the existing PreparedNote artifact. Review it and
use the separately deliberate [prepared export command](prepared-note-export.md)
to send it; `prepare-export` itself makes no provider or Mem call. Reuse the stable
correlation for the same note, with a new effect UUID for a deliberate new export.
The first draft heading becomes the note title and the exact remaining Markdown
becomes its body. Source/draft digests stay in local association provenance.
Export source scope is stable for the document identity and explicit page/notebook
selection. Regenerating changed text under a new operation changes its revision,
not that scope, so the same correlation can update the same exported note.

## Reports, cancellation and retries

`generated` means a local unrendered draft was saved. `historical-generated`
reports the current saved draft for that old operation without another model
call. `historical-pending` means a prepared operation lacks a current generated
result; it cannot authorize a retry. `busy` excludes overlapping Writer calls
sharing the same Store. `stale`, `refused`, `failed` and `canceled` do not mean a
note was generated, rendered or exported. Reports have fixed sanitized failure
codes. Successful report emission is not itself successful generation.

One owner spans preparation, provider waiting and result publication, independent
of the Store index mutex. Fresh preparation alone can dispatch; lost preparation
acknowledgments and old receipts cannot send another paid call. Source/draft
compare-and-swap guards and current root checks reject stale dispatch/results.
Same-source guard revisions preserve content-bound immutable facts.

Ctrl+C stops the existing cancellable provider wait and suppresses observed late
results. Remote computation or billing may already have occurred. A crash or
unknown post-dispatch state cannot be automatically retried; a deliberate repeat
uses a new operation UUID. The command owns and joins its blocking operation and
the existing provider owns/joins its I/O. Synchronous Store publication is not
preemptible. Alternative LLMEngine implementations must poll progress for prompt
cancellation; every returned reply is checked again before publication.

Source basis: implementation and actual local loopback HTTP/Store tests. These
tests use invented content/credentials and establish transport/persistence
behavior, not live-model quality, original extraction or native qualification.
The supplied-text generation/refinement increment is delivered. Joined export
is described below; full native Writer acceptance remains separate.

## Joined export commands

The same `--writer-note` stdin entrypoint also accepts:

- `generate-and-export`: `outer_operation`, nested `generation` (the complete
  existing `generate` request), stable `correlation`, new `effect_operation`.
- `refine-and-export`: the same outer identities and nested `refinement`
  (the complete existing `refine` request).
- `export-draft`: `outer_operation`, `conversation`, selected `draft`,
  `correlation`, `effect_operation`; no model call.
- `reconcile-export`: `conversation`, `correlation`; reads only the previously
  known destination note ID and never creates or updates it.

Configure existing model settings when generation/refinement is required and set
`MEM_API_KEY`. The production Mem destination is fixed; no origin override is
accepted. Required credentials resolve before opening the Store. Legacy commands
retain their report shape and behavior, including local-only `prepare-export`.

Use distinct nonnil UUIDs. Deliberate re-export keeps the correlation and uses new
outer/model/effect identities as applicable. Joined reports include optional
generation/export reports and actual `destination_polled`, `write_polled`,
`no_send_recorded` and `selection_changed` observations.

Fresh outer admission permits only this invocation to continue. Replay returns
history or reconciliation-required without model/destination polling. An
interruption after generation requires a new explicit `export-draft`; a saved
`generated` report alone cannot grant automatic send authority. Exact selected
root/draft/source revisions are guarded into export admission. Later checks use
the revisions that admission actually published.

Cancellation before the first write poll suppresses the write. Zero observed
write polls and successful durable no-send recording are separate facts: deletion
or conflict can prevent recording. After a write was polled, a lost response
requires explicit known-ID reconciliation, never automatic resend. Writer/export
owners span the joined invocation and contend with standalone calls. Existing
model/export cooperative budgets remain 95/45 seconds with transport bounds;
these are not a hard outer deadline on synchronous Store work.

## Bounded supplied notebooks

The same stdin entrypoint adds four explicit commands. Existing inline requests
and their identities remain unchanged:

- `generate-notebook`: `conversation`, new `operation`, exact `request`,
  `document_file`, lowercase raw-file `document_sha256`, `selection`, optional
  `guidance`, explicit `max_model_calls`, optional `history_range`.
- `refine-notebook`: `conversation`, new `operation`, exact `request`, archived
  notebook `draft`, `max_model_calls`, optional `history_range`.
- `generate-notebook-and-export`: new `outer_operation`, nested
  `generation` containing a complete `generate-notebook`, stable `correlation`,
  new `effect_operation`.
- `refine-notebook-and-export`: the corresponding complete `refinement` and
  outer identities.

`document_file` contains one complete strict `NormalizedDocument` JSON object.
Its path is transport information; only validated content/provenance enters the
archive and identities. Raw input is bounded at 8 MiB, all supplied text at
4 MiB, selected text at 256 KiB/256 units. The caller's allowance must cover
every ordered map and the one reduction; at most eight maps/nine calls are
permitted. Encoded payload and Store publication bounds can refuse a request
below these individual ceilings. Choose a smaller explicit history range when
necessary; history and original requests are never silently truncated.

File admission currently supports Linux ext4/XFS/Btrfs with retained directory
handles, no-follow/nonblocking regular-file checks and raw-hash verification.
Other platforms/filesystems refuse when those guarantees are unavailable.
The command requires an existing owned conversation. Required credentials and
the complete read-only request/payload preflight precede explicit notebook
format adoption. Adoption preserves CURRENT, generation identity and immutable
history/media while fencing unsupported predecessors; it is not native access.

Maps and reduction use the existing model provider and retained stage receipts.
Only the request and final proposal are visible conversation turns. Reports
separate local model entries, confirmed stage results, observed publication loss
and possible remote cost. Each call has the existing 95-second cooperative
budget, within a 600-second aggregate budget; joined export uses the smaller of
its existing 45-second budget and the remaining aggregate deadline. Synchronous
Store work is not forcibly preempted.

An unchanged retry reads its original frozen context and retained archive, even
if the input file has disappeared. It returns historical information with no new
model or export effects. Changed inputs/selections refuse; failed or uncertain
stages never auto-resume. Deliberate refinement uses the retained archive after
restart, including the exact selected draft revision. A new explicit
`export-draft` can export a verified saved final proposal. Joined export retains
the current invocation's fresh final witness and guards root, source, plan,
stages and media through destination polls. Correlation and source scope remain
stable across deliberate inline-to-archive re-export and subsequent refinement.

These supplied-input paths do not establish physical notebook completeness,
handwriting extraction, live model quality, native rendering or live Mem writes.
