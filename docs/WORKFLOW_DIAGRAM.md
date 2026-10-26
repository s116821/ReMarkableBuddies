# CI/CD workflow

```mermaid
flowchart TD
  Event[PR or exact main push] --> Paths[Maintained path and scoped PR checks]
  Paths --> Docs{Documentation only?}
  Docs -->|Yes| Done[Finish noncompiling checks]
  Docs -->|No| Queue[Application pipeline queue]
  Queue --> Checks[Lint, unit tests and maintained tool controls]
  Checks --> Candidate[Read-only semantic candidate]
  Candidate --> Build[Build candidate and retain artifact IDs]
  Build --> Integration[Download and exercise exact built artifacts]
  Integration --> Release[One final main release job]
  Release --> Proof[Verify exact merged source and tested bytes]
  Proof --> Tag[Maintained tag creation and remote identity proof]
  Tag --> Draft[Exact-ID draft and retained-byte checks]
  Draft --> Missing[Upload only missing assets without overwrite]
  Missing --> Complete[Verify same ID and all remote bytes]
  Complete --> Publish[Finalize release]
```

PRs cannot enter the final release job. Documentation never enters the application
queue. Canonical main pushes require a single first-parent commit; ambiguous
batches/initial history refuse instead of erasing an application edit/revert.
Queue order is arrival order, not guaranteed dispatch chronology; conflicting
candidates refuse rather than retargeting a tag. Squash merges qualify.

An earlier failed source remains untagged unless its own gates pass. Actions
Re-run failed jobs retains successful build outputs for partial-draft recovery;
a later application merge does not backfill failed commits. Published bytes are
verified, never overwritten. See [release tools and limits](../release/README.md)
for exact maintained sources, evidence, retry policy and remaining qualifications.
