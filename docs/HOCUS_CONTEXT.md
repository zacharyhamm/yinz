# Hocus context limits

Hocus keeps the original chat and archive. Before each request, it removes repeated tool payloads and applies these limits:

- Tool-result bodies use at most 64 KiB in total, with newer results first.
- Each result uses at most 8 KiB, including its omission notice.
- Completed plugin write, edit, and check calls older than the latest two tool-call batches lose large source and replacement strings.
- Existing deduplication can also remove superseded source strings in recent batches.

Call IDs, result pairing, user instructions, assistant prose, and required reasoning data stay in the request. Pending tool arguments stay unchanged. There is no summary or compaction request.

## Read omitted content

Use `tool_result_read tool_call_id=<id> part=result offset=0` to read an original result. Use `part=arguments` for original tool arguments.

Offsets count Unicode characters from zero. Each response contains `text` and `next_offset`. Pass `next_offset` to read the next page. A null value means the end. Each page contains at most 4,000 characters and 8 KiB of serialized JSON.

The tool reads only the active chat. It rejects unknown IDs and never repeats the original action. Reopened chats use their original archived messages.

## Request ceiling

Set `context_max_bytes` in the Hocus plugin configuration to change the complete serialized request ceiling. The default is 262,144 bytes. Missing, invalid, or zero values use the default.

This ceiling includes the system prompt, tools, message envelopes, reasoning, and JSON escaping. It measures bytes, not model tokens. If the request still exceeds the ceiling after elision, Hocus stops before sending it. The error shows the size, limits, and largest request pieces. Too many retrieval references can also exceed the result budget and stop a request.

## Authoring and usage

Search documentation before reading a section. Use outlines, searches, and ranged reads to locate code. Batch independent edits and load tool families only as needed.

Use `plugin_check name=clock.ts` to check a saved file. Give `source` only for an unwritten draft. Writes and edits already check syntax. An additional check is useful for types and imports.

The lower input border shows **last in** (the last request's input) only when the provider reports it, including zero. This count includes conversation context sent with that request. Available cumulative **in** and **out** totals and cost remain visible even when the latest request has no input count. When no usage is available, the border is a plain line.

Switching chats keeps each chat's measurement separate. Reopened chats omit last-request input until another request reports it. The usage note uses the existing input border and leaves chat-tab space unchanged. Very narrow panes clip the note.

Request logs include original and transmitted payload bytes, omitted-result counts, and final request bytes. Large-request breakdowns describe the actual serialized body. Budget refusals have a separate log event.

Cumulative usage can still exceed one million tokens. The limits bound individual requests and reduce the cost of replaying old tool output.

The [Decisions tool](HOCUS_DECISIONS.md) reports its usage separately in each tool result.

## Measured replay savings

The deterministic test runs 24 authoring cycles, with 168 requests per transport. Each cycle includes documentation lookup, a separate file, write/edit/check/reload calls, and extension output.

The comparison uses the prior deduplication policy with identical tool schemas and prompts. It isolates the effect of payload limits.

| Transport | Peak request bytes, before → after | Cumulative request bytes, before → after |
|---|---:|---:|
| Chat Completions | 1,736,029 → 167,835 | 148,838,720 → 20,972,285 |
| Responses | 1,729,091 → 160,981 | 148,262,312 → 20,395,877 |

This fixture reduces peak size by about 90% and cumulative replay bytes by about 86%. These are fixture measurements, not predicted token or billing savings.

Run `cargo x test -p hocus authoring_replay --success-output immediate` to reproduce the measurement.
