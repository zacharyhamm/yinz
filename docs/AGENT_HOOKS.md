# Claude Code hooks → the pane's agent record

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
against eight events. The script hands each event's JSON to the session:

```sh
zellij action agent-report --hook-json < "$payload"
```

`agent-report` reads the event, works out what it means (`zellij-utils/src/agent.rs`,
`hook_outcome`) and sets the pane's **agent record** — `PaneInfo.agent`: the kind (`claude`),
the state, the tool it is running, its folder and its session id. The pane is
`$ZELLIJ_PANE_ID`, which the action reads itself. Every reader of the pane manifest then sees
the same answer: plugins on `PaneUpdate`, `zellij events`, `zellij action list-panes --json`,
and hocus's agents list. The record is complete every time it is set; nothing merges.

The same record is written by two other roads, which is what makes "is this pane an agent" one
question with one answer: the spawn path, when a plugin opens a pane *as* an agent pane
(`AgentSource::Spawned`, state `Busy`, moved to `Exited` when the command exits and back to
`Busy` on re-run), and `PluginCommand::SetPaneAgent` / `zellij.set_pane_agent`, for a plugin
speaking for an agent. A pane with a record fires the `agent_exit` hook when its command exits,
however the record got there — `claude` typed at a shell becomes an agent pane the moment it
reports for itself. The record's edges fire hooks of their own: `agent_start` when a record
appears (or comes back after `exited`, or changes `session_id`), `agent_waiting` when it moves
to `waiting`. Both go to a headless plugin as `Event::Hook` — see `docs/LUA_PLUGINS.md`,
"Headless hooks". `zellij action agent-report --state waiting --detail "y/n"` is the same road
for a script of your own; `--clear` ends the record and fires nothing.

| Event | State | Why |
|---|---|---|
| `SessionStart` | idle | Starts the feed, and proves the pane runs Claude Code |
| `UserPromptSubmit` | busy | |
| `PreToolUse` | busy | Names the tool: `busy · Bash cargo` |
| `PostToolUse` | busy | Clears the tool name |
| `PermissionRequest` | **waiting** | Fires as the prompt goes up |
| `Notification` | **waiting** | Only for `permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input` |
| `Stop` | idle | Every turn end |
| `SessionEnd` | — | Clears the record |

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

hocus-agentd reads the records off the pane manifest: a record the agent reported itself
(`AgentSource::Reported`, not `Exited`) is a *feed*, and a pane with one is read off it and
nothing else:

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
`cargo build` is one `PreToolUse` and a long silence. A record ends only where an ending
is *observed* — `SessionEnd` clears it, the command exiting marks it `Exited`, the pane
closing takes it with it. The cost is that a single dropped event leaves a pane mislabelled
until the next one arrives.

A script from before v3 of the installer still pipes the event to hocus-agentd
(`zellij pipe --name agent_hook`); the daemon writes the same record through
`set_pane_agent`, so an old install lands in the same place. `zellij hocus hooks status`
says when the installed script is old.

## Where it lives

| Piece | File |
|---|---|
| The record, and who may write it | `zellij-utils/src/data.rs` (`AgentInfo`, `AgentState`, `AgentSource`, `PaneInfo::agent`) |
| Event → record | `zellij-utils/src/agent.rs` (`HookEvent`, `hook_state`, `hook_detail`, `hook_outcome`) |
| The action, and the installer | `zellij-utils/src/cli.rs` (`AgentReport`), `zellij-utils/src/input/actions.rs`, `src/hocus_hooks.rs` |
| Carrying it on the pane | `zellij-server/src/panes/terminal_pane.rs` (`agent`, `agent_from_run`), `screen/thread_main.rs` (`SetPaneAgent`), `screen/mod.rs` (`fire_agent_exit_hook`, `fire_agent_state_hooks`, `agent_hook_edges`) |
| What a row draws | `default-plugins/hocus-proto/src/agents.rs` (`HookFeed::from_agent`, `status_cell`, `waiting_marker`) |
| The old pipe road | `default-plugins/hocus-agentd/src/main.rs` (`State::on_hook`) |

Note: `zellij hocus hooks` is unrelated to the `hooks { … }` block in zellij's own
config, which binds a plugin to a moment in the session's life
(`zellij-utils/src/input/hooks.rs`).
