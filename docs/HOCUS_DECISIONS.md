# Hocus Decisions tool

`openrouter_decisions` evaluates multiple named questions against one state. It returns answers without taking action on them.
The deferred `decisions` family supports `choice`, `score`, and `noul` (boolean likelihood from 0 to 1).
Use `tools_load family=decisions` to discover the tool, or call it directly. It is available in planning mode.

## Setup

Set `OPENROUTER_API_KEY` before you start the Zellij session. Hocus uses its existing `WebAccess` permission, with no additional confirmation prompt.
The defaults are `typesafe/jev-1.13` and `https://openrouter.ai/api/alpha/decisions`.
See the [official OpenRouter integration](https://github.com/OpenRouterTeam/ai-sdk-provider#evaluation-jev-with-ai-sdk-through-openrouter).

The existing plugin settings `decisions_model` and `decisions_url` override `OPENROUTER_DECISIONS_MODEL` and `OPENROUTER_DECISIONS_URL`.
Set `decisions_api_key_env` to use a credential variable other than `OPENROUTER_API_KEY`. Chat credentials are not a fallback.
Hocus resolves these settings for each call from the plugin configuration and captured session environment.
The optional `model` argument overrides the model for one call. URLs and credentials are not tool arguments.

## Mixed questions

Paste this command into hocus:

```text
/openrouter_decisions {"state":{"ticket":"The checkout page goes blank after Pay."},"questions":{"team":{"type":"choice","instructions":"Which team owns this issue?","criteria":{"payments":"Payment processing failures","frontend":"Rendering problems"}},"urgency":{"type":"score","instructions":"How urgent is this issue?","criteria":["Can wait","Needs attention this week","Blocks checkout"]},"is_bug":{"type":"noul","instructions":"Does this ticket describe a software defect?"}}}
```

`state` and question `instructions` accept strings, objects, or arrays. Choice criteria map option names to guidance, or null to use the name alone.
Score criteria run from lowest to highest. The score is the probability-weighted mean of their zero-based indices.
Optional `noul` criteria must include both `true` and `false` guidance. Optional `user` and `session_id` strings each allow up to 256 characters.

Results include typed answers, model, token usage, cost when supplied, and available request and provider IDs.
Decisions usage stays in the tool result, separate from chat-model accounting. HTTP and decoding errors also appear as tool results.
Requests are not retried automatically. Ctrl+c stops the wait and discards a late response, but does not cancel the server request.
Switching chats keeps the request attached to the chat that started it.
