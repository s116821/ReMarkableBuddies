# Maintenance guides

Choose the guide for the operation you intend to run. Each interface has its own
platform support, input format, custody checks and size limits. Inventory results
help select data without granting mutation authority. For operations with a
preparation step, follow that guide before applying a change.

These are public manual equivalents for implemented source interfaces. Keep
saved-file and local-data results separate from installed runtime, provider and
native-page qualification. Individual guides identify remaining companion work.

## Saved configuration and credentials

| Task | Guide |
| --- | --- |
| Inspect saved fields and declaration origins | [Saved configuration](saved-configuration.md) |
| Edit trigger corner, log level or diagnostic dump declarations | [Basic settings](basic-settings.md) |
| Edit saved model and endpoint declarations | [Model settings](model-settings.md) |
| Manage the declared protected model credential | [Model credential](model-credential.md) |
| Change the saved private credential reference | [Credential reference](credential-reference.md) |
| Edit saved memory enablement and profile declarations | [Memory settings](memory-settings.md) |

## Subject memory

| Task | Guide |
| --- | --- |
| Run offline memory maintenance | [Host memory](host-memory.md) |
| Query subject memory over stdin | [Memory query](memory-query-stdin.md) |
| Inspect one memory fact over stdin | [Fact inspection](memory-fact-inspection-stdin.md) |
| Prepare one-fact forget and reconcile its receipt | [Prepared memory forget](memory-prepared-forget.md) |

## Storage modes and archives

| Task | Guide |
| --- | --- |
| Inspect or record a saved storage mode | [Storage mode maintenance](host-sync-mode.md) |
| Export or inspect a selected archive | [Selected archives](host-selection-archives.md) |
| Export an ordinary owned archive | [Ordinary export](ordinary-export.md) |
| Restore an ordinary archive into an existing ordinary Store | [Ordinary restore](ordinary-restore.md) |

## Source caches and conversations

| Task | Guide |
| --- | --- |
| Understand the local normalized source cache | [Normalized source cache](normalized-source-cache.md) |
| List source-cache candidates | [Source-cache inventory](source-cache-inventory.md) |
| Prepare cleanup of one derived cache slot | [Source-cache cleanup](source-cache-cleanup.md) |
| Understand historical conversation records | [Historical conversation library](historical-conversations.md) |
| List ordinary conversation-root candidates | [Conversation inventory](conversation-inventory.md) |
| Prepare logical cleanup of an ordinary conversation | [Conversation cleanup](conversation-cleanup.md) |

Prepared-operation guides explain which original confirmation data to retain and
how to reconcile incomplete output. Follow those rules before another deliberate
action.
