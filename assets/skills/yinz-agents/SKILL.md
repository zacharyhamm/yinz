---
name: yinz-agents
description: Launch, inspect, and coordinate Yinz agents with guarded prompts and correlated outcomes, and inspect their durable archives.
---

Inside a Yinz pane, `YINZ_SESSION_NAME` selects the current session. Start with `yinz agent list --json`.
Outside a session, run `yinz list-sessions --short`, select the intended name, and add `--session NAME` to each command below.
A sandboxed Codex cannot reach Yinz ("connection was not permitted"). Spawn Codex orchestrators with `--allow-yinz`, which also allows internet access, or `--permission-mode skip`.
Use `--project NAME` to filter by project. Explicit tab assignments take precedence over checkout discovery.
Basic live operations do not require Hocus chat, `hocus-agentd`, or `hocus-store`.
Registration requires `hocus-agentd`. Archive queries require `hocus-store`.

A pane ID identifies the current terminal target, such as `terminal_3`.
A conversation ID identifies the provider conversation and guards against replacement.
A run ID identifies a durable archive record. Do not substitute one ID for another.

Use this sequence. Replace `KIND` with the requested provider, such as `claude` or `codex`.
Choose a model for that provider. The kind must match the task, not an example.

```sh
yinz agent spawn --kind KIND --project work --branch feature --json
yinz agent wait terminal_3 --until idle --timeout 2m --json
yinz agent status terminal_3 --json
yinz agent prompt terminal_3 --file prompt.md --conversation CONVERSATION_ID --wait --timeout 10m --json
yinz agent read terminal_3 --conversation CONVERSATION_ID --lines 100 --json
```

Claude Haiku launch example:

```sh
yinz agent spawn --kind claude --model haiku --project work --branch feature --json
```

Use the actual pane and conversation IDs from the responses, not the example `terminal_3`.
Check `ready`, `status_available`, and `prompt_supported` before submission.
An idle state alone does not establish readiness or prove task success.
A completed turn does not verify the resulting work. Run the checks that the task requires.
Guarded prompts support Claude Code and Codex. Other agent kinds support launch and inspection.
Use general pane input for deliberate interaction with other kinds. Do not fall back automatically to raw input.

Live commands return `AgentResponse` with `--json`. Check `outcome` even when the command exits nonzero.
`submitted` means the provider acknowledged the prompt. `completed` means that correlated turn ended.
Handle `needs_input`, `failed`, `interrupted`, `target_changed`, and `target_closed` before continuing.
`timed_out` or a lost connection leaves the result uncertain. Inspect before retrying a mutation.
The `acknowledged` and `turn_id` fields describe the original submission.

`agent read --lines N` selects the last N nonblank lines, preserves internal blank rows, and removes blank rows at both ends.
Whitespace-only rows count as blank. Text and JSON retain the same content, including ghost suggestions.
Plain terminal text cannot establish whether the composer contains a draft. Do not infer a draft from suggestion text or strip suggestions heuristically.
If a submission is uncertain, inspect the composer in the terminal UI and clear any actual draft deliberately.
Use `--confirm-empty` only after deliberately confirming the composer is empty and the conversation is unchanged.
Recovery example (Esc alone does not confirm an empty composer):

```sh
yinz agent send-keys terminal_3 Esc --conversation CONVERSATION_ID --json
yinz agent recover terminal_3 --conversation CONVERSATION_ID --confirm-empty --json
```

For `send-keys`, pass one key per argument: `Esc`, `Enter`, or `Down Enter`. Quote modifiers such as `"Ctrl u"`.
`Ctrl+u`, `ctrl-u`, `C-u`, `Escape`, and `Return` are unsupported aliases.
Recovery requires a live idle agent and no active submission or paste write.
Claude reports nothing when Esc interrupts a turn, so it stays `Busy`. If `agent read` shows the
agent stopped, clear any draft (Esc can restore the interrupted prompt), then add
`--interrupted` to reset it to idle. This also applies to a `Waiting` agent whose dialog you
dismissed. It needs input, such as Esc, after the agent's last hook and 3 seconds without a
hook since.
Use ordinary pane commands to close or restart a pane.

`agent spawn --file PATH` or `--file -` reads launch options from JSON. CLI options override file values.
Spawn rejects `prompt` and `reuse_prompt`, including fields in JSON. Use spawn, wait, then prompt.
Use `--reuse` only when the existing branch or worktree is the intended target.
Otherwise collisions produce a fresh branch and worktree. Project defaults supply worktree directories and VM images.
Read `agent spawn --help` for model, effort, permissions, resume, and Firecracker options.
For SSH, provide an absolute `--cwd` and `--remote-host`. Verify the remote worktree and explicitly use `--allow-outside-worktree`.
SSH launches cannot use local project, branch, worktree, or VM options. General pane creation owns arbitrary programs.
Register an existing pane with `agent register NUMERIC_PANE_ID KIND`.

```sh
yinz agent runs --project work --limit 20 --offset 0
yinz agent run RUN_ID --from-revision 1 --limit 20
yinz agent history-search 'rollback' --project work
yinz agent predecessors RUN_ID
```

Archive commands return their existing JSON shapes without `--json`.
Preserve pagination cursors and filters. Use both host and session filters for one server's records.
Workflow runs are separate from agent turns and agent archive runs.
