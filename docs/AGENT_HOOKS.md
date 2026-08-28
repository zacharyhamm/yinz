# Claude Code hooks → hocus agent state

`hocus-agentd` normally works out what an agent pane is doing by *looking at it*: every
five seconds it reads the pane's scrollback, hashes the word sequence, and calls the
pane busy if the hash moved. Whether the agent is *blocked on the user* is not observed
at all — after three unchanged polls (~15s) the daemon sends 120 lines of pane tail to a
model and asks.

That is the only thing that works for an agent which says nothing about itself, and it
is a guess bought with fifteen seconds and a round trip. Claude Code says. Install its
hooks and a `claude` pane reports its own state instead:

```sh
zellij hocus hooks install     # ~/.claude/settings.json
zellij hocus hooks status      # what is wired, and what would stop it working
zellij hocus hooks uninstall
```

`--project` writes to `<cwd>/.claude/settings.json` instead. Do **not** install into
both: Claude Code merges hooks across settings files rather than one overriding the
other, so every event would fire twice. `status` says so if it sees it.

## What is wired

The installer writes `~/.claude/hocus/hocus-hook.sh` and registers it, with no matcher,
against eight events. The script forwards each event's JSON to the daemon:

```sh
zellij pipe --name agent_hook --args "pane_id=$ZELLIJ_PANE_ID" -- "$payload"
```

| Event | State | Why |
|---|---|---|
| `SessionStart` | idle | Starts the feed, and proves the pane runs Claude Code |
| `UserPromptSubmit` | busy | |
| `PreToolUse` | busy | Names the tool: `busy · Bash cargo` |
| `PostToolUse` | busy | Clears the tool name |
| `PermissionRequest` | **waiting** | Fires as the prompt goes up |
| `Notification` | **waiting** | Only for `permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input` |
| `Stop` | idle | Every turn end |
| `SessionEnd` | — | Ends the feed |

`PermissionRequest` is the primary waiting signal and `Notification` the backstop: the
notification only fires after ~6s of the prompt going unanswered, which is most of what
the feed exists to remove. Neither one returns a decision — the script exits 0 with
empty stdout, which leaves the permission flow exactly as it was.

Two rules the generated script keeps, both of which matter:

- **It never writes to stdout.** On `UserPromptSubmit` and `SessionStart`, an exit-0
  hook's stdout is injected verbatim into Claude's context.
- **It always exits 0.** A hook that fails must not interfere with the agent it is only
  reporting on.

Each entry carries an explicit `"timeout": 5`. The default is ten minutes and a hook
blocks the turn.

## What the feed changes

A pane with a live feed is read off that feed and nothing else:

- `busy` / `waiting` / `idle` in the agent strip, with no settle delay — a `Stop`
  lands the idle label on the message that carries it.
- The `!` marker and `agent_list`'s `awaiting_confirmation` come from the feed. The
  model is no longer asked whether an agent is blocked on the user; where it answers
  anyway, `finish_summary_with` overwrites it.
- The pane stays classified as an agent while Claude shells out to `git` — a hook
  firing in a pane is better evidence than its foreground command, which stops naming
  the agent the moment a tool runs.
- The agent's `cwd` comes from the hook, which is the only thing that reports an agent
  told to work in a folder the shell under it never entered.

Idle summaries are **unchanged**. They still hang off the screen heuristic, because
what they produce is a description — the toast, the summary box, the pane's new name —
and a description of a screen is fairly made by reading it.

Every other agent kind, and a `claude` pane with no feed, behaves exactly as before.

## When a feed ends

Nothing here expires on a clock, and that is deliberate: a permission prompt can sit
unanswered all day without the pane's state having changed, and a ten-minute
`cargo build` is one `PreToolUse` and a long silence. A feed ends only where an ending
is *observed* — `SessionEnd`, the pane closing, or the agent latch being released
because the pane is demonstrably back at a shell prompt. The cost is that a single
dropped event leaves a pane mislabelled until the next one arrives.

## Where it lives

| Piece | File |
|---|---|
| Pipe name, argument, payload | `default-plugins/hocus-proto/src/wire.rs` (`AGENT_HOOK`, `HOOK_PANE_ID`, `HookEvent`) |
| Event → state, and what a row draws | `default-plugins/hocus-proto/src/agents.rs` (`hook_phase`, `HookFeed`, `status_cell`, `waiting_marker`) |
| Receiving and publishing | `default-plugins/hocus-agentd/src/main.rs` (`State::on_hook`) |
| The command and the installer | `src/hocus_hooks.rs`, `zellij-utils/src/cli.rs` |

The pipe's name is spelled in two places — `wire.rs` and `src/hocus_hooks.rs` — because
the binary does not link the plugin crate. A mismatch is silent, so both are worth a
grep together.

Note: `zellij hocus hooks` is unrelated to the `hooks { … }` block in zellij's own
config, which binds a plugin to a moment in the session's life
(`zellij-utils/src/input/hooks.rs`).
