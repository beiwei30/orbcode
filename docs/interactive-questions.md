# Interactive questions

`AskUserQuestion` is exposed to the model only for a turn owned by a client
that declares the complete version-1 interaction capability. The default is
off, so unknown clients, ordinary `--print` text/JSON, background turns, and
partial-capability clients cannot accidentally expose a question schema they
cannot complete. ACP declares only its stable single-question option mapping;
that subset does not enable the canonical provider schema.

An app-server client opts in during `initialize`:

```json
{
  "capabilities": {
    "streaming": true,
    "experimental_methods": true,
    "interactive_questions": {
      "single_select": true,
      "multi_select": true,
      "free_text": true,
      "previews": true,
      "annotations": true,
      "special_outcomes": true
    }
  }
}
```

The server then sends `ask_user/request` with `session_id`, `turn_id`,
`tool_use_id`, `request_id`, an optional absolute `deadline`, and one to four
canonical `questions`. Each question has stable question and option IDs. The
legacy one-question `question`/`options` fields remain readable during the
protocol-1.0 compatibility window.

## Duplex stream-JSON

Duplex interaction mode is enabled with `--print --input-format stream-json
--output-format stream-json --verbose`. While stdin is open it advertises the
full capability and emits the request as an ordered
`stream_event` whose nested event has `type: "server_request"`, method
`ask_user/request`, the correlation `request_id`, and canonical `params`.
Respond on stdin with the same ID and a typed outcome:

```json
{"type":"server_response","request_id":"ask-1","response":{"outcome":"answered","answers":{"database":{"kind":"selected","option_id":"postgres"}},"annotations":{}}}
```

Other response outcomes are `rejected`, `clarify`,
`finish_plan_interview`, or `cancelled` with a reason. Invalid answers return a
correlated error and leave the request pending so the host can retry. Unknown,
stale, and duplicate IDs are rejected deterministically. Closing stdin cancels
all pending questions as a disconnect; permission auto-approval never answers
questions.

The checked-in JSON Schema and generated TypeScript declarations under
`app-server-protocol/tests/generated/` are the authoritative wire reference.

## ACP subset

ACP maps a representable single-select, option-only request to
`session/request_permission`. Free-text, multi-question, annotation, preview,
and special-outcome requests are cancelled instead of being misrepresented.
Because this is not the complete version-1 capability, ACP turns keep
`AskUserQuestion` out of provider tool definitions even though a compatible
legacy or otherwise forced option-only request can still be answered.
