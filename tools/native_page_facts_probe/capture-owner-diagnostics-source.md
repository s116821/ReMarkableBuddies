# Historical capture owner diagnostic consumer

This source implements the reviewed Docs consumer amendment at
`0aef35c5429e46a0b9db6a66cf19795c0ba270fc` and SDK diagnostic contract at
`fb32d57c1b623ef1ea233a3f2da9a49c6d78f368`. Its SDK implementation reference is
`1d221d73c8e2a2a19bfb2d1b37c09bca41d554ed`. Source references do not select an
artifact, private packet, nonce or native trial.

The operator retains the private root device/inode before arming. After mandatory
stock restoration, it collects `capture-owner-refusal.json` independently of any
capture completion, under the retained root and original attempted PID/start.
Restoration is checked before and after each metadata read. Exact file type,
mode, size, digest and inode are checked around the bounded copy.

Complete returned bytes are verified and saved with CreateNew before decoding.
Malformed or empty complete evidence remains preservation knowledge. A later
binding failure preserves that copy and refuses cleanup. Unknown/partial copies
do not become preservation knowledge. Raw duplicate keys, including decoded
escaped names, are rejected before JSON value conversion; the decoder enforces
exactly 29 typed fields, closed enums, identity, times and the branch/null matrix.
It never changes capture, facts, native, render or UI authority.

The exact diagnostic filename joins the capture preservation/cleanup list.
Cleanup requires a verified local byte copy matching current remote metadata or
positive absence. An empty file is allowed as malformed historical evidence only
for this diagnostic; existing capture output size requirements remain intact.

Validation on 2026-10-07:

- owner diagnostic proof/collector: 292 checks passed, including malformed/empty
  preservation, timeout and later loss, duplicate keys, branch tuples, identities,
  historical deadline latch and cleanup eligibility, every finite progress/allowed/discovery/active-owner/observer registry, and reached-counter/pair requirements and impossible post-traversal overflow/context-phase histories;
- existing capture proof: 156 checks passed;
- existing capture collector: 40 checks passed;
- actual source block integration: 18 mocked checks passed, including restored
  original-identity collection without a completion and unknown-output cleanup
  refusal.

These are host/mocked source results. Independent consumer review and native
qualification remain open. The operator is an unselected source template.
