# Claude Code hooks → the pane's agent record

`hocus-agentd` normally works out what an agent pane is doing by *looking at it*: every
five seconds it reads the pane's scrollback, hashes the word sequence, and calls the
pane busy if the hash moved. Whether the agent is *blocked on the user* is not observed
at all — after three unchanged polls (~15s) the daemon sends 120 lines of pane tail to a
model and asks. The same tail, and the same wait, is what an idle summary is made of.

That is the only thing that works for an agent which says nothing about itself, and it
is a guess bought with fifteen seconds and a round trip. Claude Code says. Install its
hooks and a `claude` pane reports its own state instead:

```sh
zellij hocus hooks install     # ~/.claude/settings.json
zellij hocus hooks status      # what is wired, and what would stop it working
zellij hocus hooks uninstall
```

`--project` writes to `<cwd>/.claude/settings.local.json` instead - the file Claude Code
reads beside a project's committed `settings.json` and keeps out of git, so the install (which
carries this machine's path to the binary) never lands in the project's history.
`--project-dir <dir>` names the folder, for a plugin or a script installing for a project it is
not sitting in. Do **not** install into both the user's file and a project's: Claude Code
merges hooks across settings files rather than one overriding the other, so every event
would fire twice. `status` says so if it sees it. An older zellij wrote a project install into
the committed `settings.json`; `install --project` moves those entries out, and `status` lists
that file as `legacy:` while they are still there.

## What is wired

The installer writes `~/.claude/hocus/hocus-hook.sh` and registers it, with no matcher,
against seventeen events. The script hands each event's JSON to the session:

```sh
zellij action agent-report --hook-json < "$payload"
```

`agent-report` reads the event, works out what it means (`zellij-utils/src/agent.rs`,
`hook_outcome`) and sets the pane's **agent record** — `PaneInfo.agent`: the kind (`claude`),
the state, the tool it is running, what it is blocked on, its folder and its session id. The
pane is `$ZELLIJ_PANE_ID`, which the action reads itself. Every reader of the pane manifest
then sees the same answer: plugins on `PaneUpdate`, `zellij events`, `zellij action list-panes
--json`, and hocus's agents list. The record is complete every time it is set; nothing merges.

**The same command also broadcasts the event itself**, whole, on the `agent_hook` pipe. A
record is deliberately smaller than an event: the agent's own final message on a `Stop`, the
prompt behind it on a `UserPromptSubmit`, the permission mode on every one of them — none of
those belong on a manifest that every plugin is handed on every pane update, and all three are
what an idle summary and a well-judged interruption are made of. So one process spawn does
both, and a plugin that wants more than a record holds subscribes to the pipe. It is a
broadcast without a pipe id, so nothing waits to be unblocked and a hook cannot stall the turn
it fired in.

The same record is written by two other roads, which is what makes "is this pane an agent" one
question with one answer: the spawn path, when a plugin opens a pane *as* an agent pane
(`AgentSource::Spawned`, state `Busy`, moved to `Exited` when the command exits and back to
`Busy` on re-run), and `PluginCommand::SetPaneAgent` / `zellij.set_pane_agent`, for a plugin
speaking for an agent — hocus's `agent_register` is one such plugin, writing
`AgentSource::Registered` with the kind a person gave and `idle` as a placeholder state. A pane with a record fires the `agent_exit` hook when its command exits,
however the record got there — `claude` typed at a shell becomes an agent pane the moment it
reports for itself. The record's edges fire hooks of their own: `agent_start` when a record
appears (or comes back after `exited`, or changes `session_id`), `agent_waiting` when it moves
to `waiting`. Both go to a headless plugin as `Event::Hook` — see `docs/LUA_PLUGINS.md`,
"Headless hooks". `zellij action agent-report --state waiting --detail "y/n"` is the same road
for a script of your own; `--clear` ends the record and fires nothing.

A pane hocus spawns needs none of this: `agent_spawn` (and the `hocus-new-agent` modal) hands
`claude` the same hook wiring inline as `--settings`, built from the one event list in
`zellij_utils::agent`. A user-level install on top double-reports harmlessly — an equal record
is a no-op on the pane. Pass `hooks: false` to spawn without it.

| Event | State | Why |
|---|---|---|
| `SessionStart` | idle | Starts the feed, and proves the pane runs Claude Code |
| `UserPromptSubmit` | busy | Carries `prompt`, kept for the summary at the end of the turn |
| `PreToolUse` | busy | Names the tool: `busy · Bash cargo` |
| `PostToolUse` | busy | Clears the tool name |
| `PostToolUseFailure` | busy | A tool that failed is still the agent working |
| `PreToolUse` (`AskUserQuestion`, `ExitPlanMode`) | **waiting** | These two tools *are* the question |
| `PermissionRequest` | **waiting** | Fires as the prompt goes up |
| `PermissionDenied` | busy | Auto mode refused a call; the agent carries on |
| `Elicitation` | **waiting** | An MCP server asking for input |
| `ElicitationResult` | busy | The answer came back |
| `Notification` | **waiting** / idle | Backstop; see below |
| `SubagentStart` / `SubagentStop` | busy | A worker under an agent that is working anyway |
| `PreCompact` / `PostCompact` | busy | Work the user did not ask for and is waiting on |
| `Stop` | idle | Every turn end. **Carries `last_assistant_message`** |
| `StopFailure` | idle | The turn died on an API error — the only thing that says so |
| `SessionEnd` | — | Clears the record, and ends the feed |

`PermissionRequest` is the primary waiting signal and `Notification` the backstop: the
notification only fires after ~6s of the prompt going unanswered, which is most of what
the feed exists to remove. Its `notification_type` is matched against an allowlist —
`permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`,
`worker_permission_prompt` for waiting, `agent_completed` for idle — so a kind added in a
later Claude Code cannot silently start drawing a `!`. None of these hooks returns a
decision: the script exits 0 with empty stdout, which leaves the permission flow exactly
as it was.

Two rules the generated script keeps, both of which matter:

- **It never writes to stdout.** On `UserPromptSubmit` and `SessionStart`, an exit-0
  hook's stdout is injected verbatim into Claude's context.
- **It always exits 0.** A hook that fails must not interfere with the agent it is
  only reporting on.

Each entry carries an explicit `"timeout": 5`. The default is ten minutes and a hook
blocks the turn.

## What the feed changes

hocus-agentd reads a feed two ways, and prefers the first. A pane whose events it hears on
`agent_hook` is read from those, because the event stream carries what a record cannot — see
`event_fed`. Every other pane is read off the manifest: a record the agent reported itself
(`AgentSource::Reported`, not `Exited`) is a feed too, which is what gives a pane hocus
spawned, an `agent-report --state` script, or a plugin speaking for an agent the same row.
Either way a pane with a feed is read off it and nothing else:

- **`◆` on the kind**, so `claude` reads `claude◆` and a remote one `claude◆@box`. Every
  other row on the strip is a guess made by reading a screen; a row carrying this one is
  being told. There is no other way to see that the install took, or that a feed has gone.
- `busy` / `waiting` / `idle` in the agent strip, with no settle delay — a `Stop`
  lands the idle label on the message that carries it.
- **`waiting · <what for>`**: the tool a permission prompt is about, the question an
  `AskUserQuestion` is asking, the sentence an MCP server wrote. The row cuts it to the
  sidebar's budget; the toast below gets the longer form.
- The `!` marker and `agent_list`'s `awaiting_confirmation` come from the feed. The
  model is no longer asked whether an agent is blocked on the user; where it answers
  anyway, `finish_summary_with` overwrites it. `agent_list` also carries `hooks: true`
  and `waiting_on`.
- The pane stays classified as an agent while Claude shells out to `git` — a hook
  firing in a pane is better evidence than its foreground command, which stops naming
  the agent the moment a tool runs.
- The agent's `cwd` comes from the hook, which is the only thing that reports an agent
  told to work in a folder the shell under it never entered.

### The summary is made from what the agent said

A `Stop` carries `last_assistant_message` — Claude Code's own description of the field
is "avoids the need to read and parse the transcript file" — so for a pane with a feed
the idle summary no longer comes from the screen at all:

- **The trigger is the `Stop`.** No settle wait, no `worth_summarizing` on screen text,
  no baseline comparison. A `Stop` fires once per turn, so it *is* the edge, and it
  cannot ask twice about one turn. There is no cooldown either: three short questions in
  a minute are owed three answers.
- **The input is prose written for a person**, plus the `UserPromptSubmit` that asked for
  it. A description of an answer written without sight of the question is half a
  description.
- **`awaiting` is not asked for.** The feed already knows, so asking would only create a
  disagreement to resolve.
- **The screen stops asking about these panes.** Two answers per turn, one of them a
  guess, and whichever landed last would win.
- **Staleness is the opposite rule.** A screen summary is void the moment the screen
  moves. A turn summary is void when the session is replaced, or when any later work
  begins — a `cargo build` printing throughout the round trip does not make the sentence
  about it wrong, but the next `busy` event does, and cancels the request where it
  arrives rather than testing the phase when the answer lands. A short turn can start
  *and finish* inside one round trip, and checking only the phase would find that pane
  idle again and take the stale answer for a current one.
- **With no API key it still works.** `local_summary` takes the message's opening
  sentences, drops fenced blocks and markdown markers, and gives no title — so the pane
  keeps its name. Before the feed there was nothing to fall back *to*: a screen tail is
  not a summary of anything.

### A waiting agent interrupts

The `!` in a sidebar was only ever half the answer; the sidebar is very often not what is
being looked at. On the edge into a wait the daemon toasts, over whatever pane the user
is actually in, and the toast focuses the waiting pane when clicked:

```
agent waiting · 7 · claude◆
Which library should we use for date formatting?
```

Three rules, each the same one the summary toast already keeps:

- **Not over the pane it describes.** If the user is focused there, they can see it.
- **The edge, on the phase alone.** `PermissionRequest` puts the prompt up and the
  `Notification` for that same prompt follows ~6s later, wording it differently — so
  comparing the two descriptions is exactly the comparison that would let one prompt
  interrupt twice. Nothing is lost: between two genuine questions the agent *works*, and
  that `busy` is what re-arms the edge. For the same reason a `Notification` never
  overwrites a `waiting_on` that is already set — it fills a gap, it does not replace.
- **Not for a prompt the mode answers itself** — and only for those. A mode answers a
  *permission prompt*: `bypassPermissions`, `dontAsk` and `auto` answer every one, and
  `acceptEdits` answers only the file-editing tools, so a `Bash` prompt under it blocks
  exactly as it would under `default` and still interrupts. An `AskUserQuestion`, an
  `ExitPlanMode`, an MCP `Elicitation` and every waiting `Notification` are never
  suppressed under any mode: the first three address a person directly, and the last
  fires only once its prompt has gone six seconds unanswered, which is itself the proof
  nothing answered it. The row draws the wait either way — it is what the agent said.

## When a feed ends

Nothing here expires on a clock, and that is deliberate: a permission prompt can sit
unanswered all day without the pane's state having changed, and a ten-minute
`cargo build` is one `PreToolUse` and a long silence. A record ends only where an ending
is *observed* — `SessionEnd` clears it, the command exiting marks it `Exited`, the pane
closing takes it with it. The cost is that a single dropped event leaves a pane mislabelled
until the next one arrives, which is what `StopFailure` exists to stop being the common case.

A `session_id` that changes in the same pane is a session replaced rather than ended —
`/clear` — and drops the summary, the in-flight request and the held prompt with it.

A script from before v4 of the installer pipes the event to hocus-agentd and nothing else
(`zellij pipe --name agent_hook`); the daemon writes the same record through
`set_pane_agent`, so an old install lands in the same place, minus the nine events v4
added. `zellij hocus hooks status` says when the installed script is old.

## Where it lives

| Piece | File |
|---|---|
| The record, and who may write it | `zellij-utils/src/data.rs` (`AgentInfo`, `AgentState`, `AgentSource`, `PaneInfo::agent`) |
| Event → record, and every rule for reading one | `zellij-utils/src/agent.rs` (`HookEvent`, `hook_state`, `hook_detail`, `waiting_detail`, `waiting_needs_a_person`, `hook_outcome`, `CLAUDE_HOOK_EVENTS`, `AGENT_HOOK_PIPE`) |
| The action, the fan-out, and the installer | `zellij-utils/src/cli.rs` (`AgentReport`), `zellij-utils/src/input/actions.rs`, `src/hocus_hooks.rs` |
| Carrying it on the pane | `zellij-server/src/panes/terminal_pane.rs` (`agent`, `agent_from_run`), `screen/thread_main.rs` (`SetPaneAgent`), `screen/mod.rs` (`fire_agent_exit_hook`, `fire_agent_state_hooks`, `agent_hook_edges`) |
| What a row draws | `default-plugins/hocus-proto/src/agents.rs` (`HookFeed::from_agent`, `status_cell`, `waiting_marker`, `kind_cell`, `HOOK_MARKER`) |
| What a wait says, and the summary | same (`waiting_toast`, `turn_summary_request_body`, `local_summary`) |
| Receiving the broadcast, publishing, toasting | `default-plugins/hocus-agentd/src/main.rs` (`on_hook_with`, `on_turn_end_with`, `finish_local_summary_with`) |

The pipe's name and the pane argument are spelled once, in `zellij_utils::agent`;
`hocus_proto::wire` re-exports both, so the two ends of a broadcast that is silent when it
does not match cannot drift.

The event payloads are pinned against Claude Code's own shipped schemas rather than
against its documentation. `strings` on the binary finds them: they are Zod definitions,
one per `hook_event_name`, and they are the authority on which fields are optional.

Note: `zellij hocus hooks` is unrelated to the `hooks { … }` block in zellij's own
config, which binds a plugin to a moment in the session's life
(`zellij-utils/src/input/hooks.rs`).
