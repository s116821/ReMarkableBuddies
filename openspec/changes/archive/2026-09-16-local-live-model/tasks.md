## 1. Implementation

- [x] 1.1 Add validated explicit live model configuration and substring expectations without changing scripted defaults.
- [x] 1.2 Share execution/reporting and add bounded live adapter using the existing OpenAI client with opt-in timeout.
- [x] 1.3 Document portable secret setup and maintain an explicit live Reader example.

## 2. Verification and delivery

- [x] 2.1 Add meaningful offline checks for configuration, request bounds, timeout and provider delegation; pass existing regressions, formatting, strict lint and ARM builds.
- [x] 2.2 Configure restricted ignored credentials on the authorized laptop without exposing or committing values.
- [x] 2.3 Complete representative live-model local Reader acceptance and inspect PNG/JSON evidence.
- [x] 2.4 Verify requirements/design, synchronize canonical specifications and archive the completed change in this implementation PR.

Latest-head CI, final independent review and normal merge are post-archive delivery gates.
