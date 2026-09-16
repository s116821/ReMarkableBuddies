# Local live-model development

The default simulator is offline. To call the configured provider, explicitly use
an `llm` object with `mode: live`, as in
[the live Reader example](simulator/live/reader.json). This sends the scenario's
page images and Reader prompts to that provider and incurs its normal API costs.

## Credentials on a trusted development machine

Set `OPENAI_API_KEY` in the launching process, or create an ignored `.env` in the
repository root using a trusted local editor. `OPENAI_BASE_URL` is optional and
uses the same endpoint root as the tablet (the client appends `/v1/chat/completions`).
Omitting it selects the normal OpenAI endpoint. Existing process environment values
take precedence over `.env`. Do not put secrets in scenario files, command-line
arguments, tracked examples, screenshots or PR comments.

Example `.env` shape, replacing the placeholder only in your local ignored file:

```dotenv
OPENAI_API_KEY=your-private-key
# OPENAI_BASE_URL=https://your-compatible-provider.example
```

Check `git check-ignore .env` before adding credentials. Restrict read access to
your own user (and OS administrators as required). On Linux/macOS use `chmod 600
.env`; on Windows use the file's Security settings to remove inherited access and
grant your account access. Never copy a populated `.env` into fixtures or commit it.
For a new worktree, configure its ignored `.env` or launch with the same process
environment. Share setup instructions, never the secret itself.

## Run one live Reader iteration

```sh
cargo run -- --simulate docs/simulator/live/reader.json
```

This example uses a captured scientific-paper page with the cursive question
“why flat plate?” and a blank simulated successor. The provider independently
transcribes the question, then the normal workflow writes the accepted answer.
Outputs are under `target/simulator/live-reader/`: inspect `report.json` and
`page-1.png`. The report must identify `live-provider`, two provider calls, an
unchanged source, one navigation and one answer block. It checks stable question
text; assess the scientific explanation separately. Live answers vary.

`llm` accepts `mode: live`, optional `model` (otherwise the application's normal
default), `max_calls` (default 2, range 1–200), and `timeout_seconds` (default 90,
range 1–300, per request). Live scenarios cannot include `replies`. The request
allowance is checked before network access; exceeding it becomes a workflow
failure. The client bounds each request, including response reading. Reports and
page images are retained before assertion failures. Missing/blank credentials
fail before execution and produce no run report. Reports omit credentials; local
logs and output can still contain document text and answers, so keep them private
unless intentionally publishing test material.

This uses the production LLM abstraction and Reader policy with simulated pages.
It establishes laptop provider access and model interpretation, not native tablet
input, typography, insertion or physical gesture fidelity. CI remains offline;
its provider tests use a local HTTP fixture with dummy credentials.
