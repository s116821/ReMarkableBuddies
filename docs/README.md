# Application documentation

| Guide | Contents |
| --- | --- |
| [Local development](local-development.md) | Host setup and maintained development workflow |
| [Simulator](simulator.md) | Modeled scenarios and evidence limits |
| [Technical reference](TECHNICAL.md) | Application implementation and validation references |
| [Release workflow](release-workflow.md) | Tag-derived builds, public release fixtures and retry behavior |
| [Current-tool fixtures](fixtures/current-tool-reference.md) | Exact crop provenance and matching limits |
| [Native history fixtures](fixtures/native-history.md) | Persistence, text/style preservation and failure cases |
| [Status-style fixtures](fixtures/status-style.md) | Captured layout/style evidence and limits |

Runnable Rust examples and their fixtures live under `docs/examples/`, with
explicit targets in `Cargo.toml`. Developer tools live under `scripts/`; build,
release and service installation assets live under `cicd/release/` and
`cicd/deploy/`. Historical evidence retains its recorded paths and provenance.
OpenSpec requirements and delivery workflow remain in the central
[Docs repository](https://github.com/s116821/ReMarkableBuddiesDocs). New reusable
documentation belongs here; root project guidance and conventional discovery files
are exceptions. Historical plans are not current behavior or execution authority.
