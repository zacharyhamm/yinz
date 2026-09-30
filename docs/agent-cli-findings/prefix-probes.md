# Claude command-prefix probes

Tested on 2026-09-29 with Claude Code 2.1.285, Haiku 4.5 and manual permissions.
The user approved the trust prompt before any input was sent.
The isolated Git repo was `/tmp/yinz-claude-prefix-4ekl_w07`, in
`glowing-brachiosaur` pane `terminal_9`.
Existing panes and drafts were not used.

Each row was tested with a single line longer than 500 bytes and with multiple lines.
The repo-local `/prefix-probe` command returned `SLASH_COMMAND_EXECUTED` and a unique marker.
Shell probes used only `printf` and comments.
Hash probes forbade memory changes. The repo stayed clean.

| Input | Current instruction lead-in | Bare bracketed paste | Typed command header, then paste |
| --- | --- | --- | --- |
| Ordinary instructions | Returned marker, acknowledged, ready | Returned marker in this conversation | Not needed |
| `/prefix-probe` | Model called `Skill`, returned marker, acknowledged, ready | Model called `Skill`, returned marker | Native command returned marker, no `Skill` call |
| `!printf` | Model called `Bash`, returned marker, acknowledged, ready | Native shell printed marker, no `UserPromptSubmit` | Not needed |
| `#` | Ordinary model turn, acknowledged, ready | Ordinary model turn | Ordinary model turn, no memory editor |

The typed slash header was tested both with and without its trailing space.
Typing only `/prefix-probe` and pasting the separator plus arguments preserved the exact original
text in `UserPromptSubmit.prompt`, for both input shapes.
The fix uses this variant so tabs and newlines are never typed as keys.

Bare-paste and typed-header comparisons used deliberate pane input.
They did not register a guarded Yinz submission, so their hooks alone did not restore readiness.
Native shell execution emitted `Stop` without `UserPromptSubmit`.
An additional guarded, short `!printf` probe printed its marker, then timed out
with `acknowledged: false` and `ready: false`. It used no lead-in.
The regression test preserves this result for long and multi-line shell commands.
Slash-command tests verify exact delivery, acknowledgement and restored readiness.
The existing collapsed-paste acknowledgement test remains in place.

Raw inputs, terminal output, hooks and status responses are retained under
`/tmp/yinz-claude-prefix-4ekl_w07/probe-results/`.
The complete hook log is `.claude/probe-hooks.jsonl` in that repo.
