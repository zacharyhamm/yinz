# Coding-agent hooks → the pane's agent record

`hocus-agentd` normally works out what an agent pane is doing by *looking at it*: every
five seconds it reads the pane's scrollback, hashes the word sequence, and calls the
pane busy if the hash moved. Whether the agent is *blocked on the user* is not observed
at all — after three unchanged polls (~15s) the daemon sends 120 lines of pane tail to a
model and asks. The same tail, and the same wait, is what an idle summary is made of.

That is the only thing that works for an agent which says nothing about itself, and it
is a guess bought with fifteen seconds and a round trip. Claude Code and Codex say. Install
their hooks and a `claude` or a `codex` pane reports its own state instead:

```sh
zellij hocus hooks install                  # both, user-level
zellij hocus hooks install --agent codex    # one of them
zellij hocus hooks status                   # what is wired, and what would stop it working
zellij hocus hooks uninstall
```

A user-level command with no `--agent` does both. Claude Code's install lands in
`~/.claude/settings.json`; Codex's in `~/.codex/hooks.json`, or `$CODEX_HOME/hooks.json` where
that is set. **Codex will not run a hook it has not been told to trust**, so open `/hooks`
inside Codex afterwards and approve what the installer wrote; nothing reports until you do.

`--project` writes to `<cwd>/.claude/settings.local.json` instead - the file Claude Code
reads beside a project's committed `settings.json` and keeps out of git, so the install (which
carries this machine's path to the binary) never lands in the project's history.
`--project-dir <dir>` names the folder, for a plugin or a script installing for a project it is
not sitting in. Both are Claude Code's alone, and a project command defaults to `--agent
claude` rather than to both: Codex's project file, `<repo>/.codex/hooks.json`, is one that gets
committed, and what this installs is a path that means nothing on anybody else's machine.

Do **not** install into both the user's file and a project's: Claude Code
merges hooks across settings files rather than one overriding the other, so every event
would fire twice. `status` says so if it sees it. An older zellij wrote a project install into
the committed `settings.json`; `install --project` moves those entries out, and `status` lists
that file as `legacy:` while they are still there.

## What is wired

The installer writes one small script per agent - `~/.claude/hocus/hocus-hook.sh`,
`~/.codex/hocus/hocus-hook.sh` - and registers it, with no matcher, against seventeen events
for Claude Code and eight for Codex. The script hands each event's JSON to the session:

```sh
zellij action agent-report --hook-json < "$payload"              # a claude
zellij action agent-report --hook-json --kind codex < "$payload" # a codex
```

`agent-report` reads the event, works out what it means (`zellij-utils/src/agent.rs`,
`hook_outcome`) and sets the pane's **agent record** — `PaneInfo.agent`: the kind,
the state, the tool it is running, what it is blocked on, its folder and its session id. The
action normally reads the pane from `$ZELLIJ_PANE_ID`. A locally supervised pane also inherits
`$ZELLIJ_PANE_REF`, the stable id of its pty-host: before routing an implicit report, the action
asks that supervisor for the pane's current session and terminal id. An `Attach` re-keys both,
so hooks keep reporting after the pane is warped even though the process environment still holds
the session-local values it started with. Explicit `--session` or `--pane-id` targets still win;
non-detachable and remote panes keep the existing environment route. The pane's current agent
record travels in both `AdoptPane` and `AdoptTab`, so the receiving session has it before the next
hook fires. Every reader of the pane manifest then sees the same answer: plugins on `PaneUpdate`,
`zellij events`, `zellij action list-panes --json`, and hocus's agents list. The record is complete
every time it is set; nothing merges — with one stated exception, below.

### The run id, and why it is the one field that is kept

A record has a seventh field, `run_id`, and it is the only one the agent does not say. A hook
event says everything the agent knows about itself; it has no idea whose books it is being kept
in. So `run_id` is written by whoever *is* keeping them — hocus-agentd, through
`PluginCommand::SetPaneAgentRun` / `zellij.set_pane_agent_run`, which sets that field and leaves
the rest of the record as the agent last reported it — and the session carries it across every
subsequent report rather than making each one re-supply it. A report that leaves it unset
inherits the pane's; only clearing the record clears it too.

That exists for the warp. **A pane's agent history is not in its record**: the record is a few
fields on a manifest handed to every plugin on every pane update, and a history is a growing
list of prompts, plans, questions and results. History lives in `agent_run` rows in the shared
plugin data store — one SQLite file per *machine*, not per session, so a run written in one
session is legible in another. What a warped pane needs is not the history but its *name*, and
that is what travels.

For a claude or a codex the name needs no help: a run is `"{kind}:{the agent's own session
id}"`, and the session id does not change when the pane moves, so the adopting session derives
the same id the losing one used. `run_id` is for everything else — a hand-rolled
`agent-report` script, or a legacy hook that reports no session id, whose run would otherwise
be keyed by a pane id and a timestamp that mean nothing in the session it arrives in. Such a
script names its run once with `zellij action agent-report --run-id <id>` and need not repeat
it.

The other half is in hocus-agentd. The session that *loses* a warped pane cannot tell a warp
from a `kill -9` — a pane simply stops being in the manifest — so it writes the weaker of the
two claims: *if nobody else has taken this run, it is over*, as a store update predicated on
the run still being under its own session name. The session that *gains* the pane looks the run
up by id the moment the pane appears, and reopens it under its own name.

The record is also what arms `agent_exit` in the session that gains the pane. The other road
into that hook — the `hocus_agent_pane` marker on the pane's originating plugin — is gone by
construction after a warp: the supervisor clears an `originating_plugin` that names a plugin and
a client in the session the pane left. So the transferred record, and the pane's memory of it,
are the whole of what makes a warped agent's exit fire the hook. Whichever of those two
writes lands second, the history survives: the guard matches nothing once the row has moved,
and adoption keeps every entry, the summary and the original start time.

**The same command also broadcasts the event itself**, whole, on the `agent_hook` pipe. A
record is deliberately smaller than an event: the agent's own final message on a `Stop`, the
prompt behind it on a `UserPromptSubmit`, the permission mode on every one of them — none of
those belong on a manifest that every plugin is handed on every pane update, and all three are
what an idle summary and a well-judged interruption are made of. So one process spawn does
both, and a plugin that wants more than a record holds subscribes to the pipe. It is a
broadcast without a pipe id, so nothing waits to be unblocked and a hook cannot stall the turn
it fired in. The `--args` name the pane it fired in and the agent it came from: the payload is
the agent's own JSON and never says which agent wrote it, and the daemon writes a record of its
own from what arrives there.

"Whole" has one bound on it, `agent::HOOK_BROADCAST_MAX_BYTES` (64 KiB). Two of the fields
Claude Code sends have no size limit in its own schema — `tool_input`, so a `Write` of a
two-megabyte file, and `tool_response`, so a `Read` of anything — and this payload is copied
into every running plugin's memory, twice per tool call, inside a hook with five seconds to
live. An event over the cap goes out through `HookEvent::pruned` instead: every field it
models survives, with each string cut to 256 characters and a budget spent across
`tool_input`, which leaves everything any reader actually takes from it — the `Bash` command,
the `file_path`, the first `AskUserQuestion` question. Anything the struct does not model,
`tool_response` above all, is gone. Under the cap, which is nearly every event, the JSON
arrives exactly as Claude Code wrote it.

The same record is written by two other roads, which is what makes "is this pane an agent" one
question with one answer: the spawn path, when a plugin opens a pane *as* an agent pane
(`AgentSource::Spawned`, state `Busy`, moved to `Exited` when the command exits and back to
`Busy` on re-run), and `PluginCommand::SetPaneAgent` / `zellij.set_pane_agent`, for a plugin
speaking for an agent — hocus's `agent_register` is one such plugin, writing
`AgentSource::Registered` with the kind a person gave and `idle` as a placeholder state. A pane with a record fires the `agent_exit` hook when its command exits,
however the record got there — `claude` typed at a shell becomes an agent pane the moment it
reports for itself. **The memory of a record is enough**: a pane that has ever held one fires
the hook even if the record has since been cleared, because a clean exit clears it first —
`SessionEnd` runs a moment before the process does — and a pane that disarmed itself on the
tidiest of exits would fire the hook only for agents that were killed. The one thing that takes
the memory back is a pane re-run with a *different* command — a plugin reusing a held pane for
`npm test` is not that agent's pane any more, and forgets both the record and the memory of it.
The record's edges fire hooks of their own: `agent_start` when a record
appears (or comes back after `exited`, or changes `session_id`), `agent_waiting` when it moves
to `waiting`. Both go to a headless plugin as `Event::Hook` — see `docs/LUA_PLUGINS.md`,
"Headless hooks". `zellij action agent-report --state waiting --detail "y/n"` is the same road
for a script of your own; `--clear` ends the record and fires nothing.

A `claude` hocus spawns needs none of this: `agent_spawn` (and the `hocus-new-agent` modal)
hands it the same hook wiring inline as `--settings`, built from the one event list in
`zellij_utils::agent`. A user-level install on top double-reports harmlessly — an equal record
is a no-op on the pane. Pass `hooks: false` to spawn without it. A spawned `codex` has no
equivalent and uses the user-level install: its hooks are a file it has been asked to trust,
and handing them in on a command line would be going round exactly the check that makes them
safe to run.

| Event | Agent | State | Why |
|---|---|---|---|
| `SessionStart` | both | idle | Starts the feed, and proves the pane runs an agent |
| `UserPromptSubmit` | both | busy | Carries `prompt`: names the pane at once, and is kept for the summary at the end of the turn |
| `PreToolUse` | both | busy | Names the tool: `busy · Bash cargo` |
| `PostToolUse` | both | busy | Clears the tool name |
| `PostToolUseFailure` | claude | busy | A tool that failed is still the agent working |
| `PreToolUse` (`AskUserQuestion`, `ExitPlanMode`, `request_user_input`) | both | **waiting** | These tools *are* the question - the third is Codex's |
| `PermissionRequest` | both | **waiting** | Fires as the prompt goes up |
| `PermissionDenied` | claude | busy | Auto mode refused a call; the agent carries on |
| `Elicitation` | claude | **waiting** | An MCP server asking for input |
| `ElicitationResult` | claude | busy | The answer came back |
| `Notification` | claude | **waiting** | Backstop; see below |
| `SubagentStart` / `SubagentStop` | claude | busy | A worker under an agent that is working anyway |
| `PreCompact` / `PostCompact` | claude | busy | Work the user did not ask for and is waiting on |
| `Stop` | both | idle | Every turn end. **Carries `last_assistant_message`** |
| `StopFailure` | claude | idle | The turn died on an API error — the only thing that says so |
| `Interrupt` | codex | idle | A turn the user cancelled, which ends it just as flatly |
| `SessionEnd` | both | — | Clears the record, and ends the feed. `agent_exit` still fires |

The reading is by event and tool name, never by agent: one function answers for both, because
the record `agent-report` writes and the feed hocus draws are two different readers of it, and
a mapping only one of them knew would have the second overwrite the first. What Codex does not
send it simply never sends - it has no `Notification`, no `Elicitation` and no subagents, and
its question goes up as an ordinary tool call.

`PermissionRequest` is the primary waiting signal and `Notification` the backstop: the
notification only fires after ~6s of the prompt going unanswered, which is most of what
the feed exists to remove. Its `notification_type` is matched against an allowlist —
`permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`,
`worker_permission_prompt` — so a kind added in a later Claude Code cannot silently start
drawing a `!`. None of these hooks returns a decision: the script exits 0 with empty stdout,
which leaves the permission flow exactly as it was.

**No notification means the turn ended**, and `agent_completed` least of all, though it reads
like the one that would. Claude Code raises it from its background-job monitor, for an async
agent this session started, on the foreground session's own payload — so a pane whose Claude
launched one and kept working would be marked idle mid-turn by it. The turn ending has `Stop`
and `StopFailure`, which arrive earlier and carry the words to describe it. `agent_needs_input`
stays on the waiting list despite coming from that same monitor, because it has a second
source the finish notification does not: the foreground dialog map, which raises it for
teammate setup, a Slack-connect dialog and an offline file sync. Those are this session,
blocked, with a person to tell.

Rules the generated scripts keep, all of which matter:

- **The Claude Code script never writes to stdout.** On `UserPromptSubmit` and
  `SessionStart`, an exit-0 hook's stdout is injected verbatim into Claude's context.
- **The Codex script writes `{}` and nothing else.** Codex wants JSON back from a `Stop`
  hook; a neutral object is the answer that decides nothing.
- **Both always exit 0.** A hook that fails must not interfere with the agent it is
  only reporting on.

Each entry carries an explicit timeout — five seconds for Claude Code, three for Codex, which
caps a `SessionEnd` hook at that anyway. Claude Code's default is ten minutes, and a hook
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
- The pane stays classified as an agent while it shells out to `git` — a hook firing in a
  pane is better evidence than its foreground command, which stops naming the agent the
  moment a tool runs. The kind it is latched as is the one the broadcast named, so a codex
  stays a `codex`.
- The agent's `cwd` comes from the hook, which is the only thing that reports an agent
  told to work in a folder the shell under it never entered.
- **The prompt names the pane, as it is submitted.** A `UserPromptSubmit` is the question
  the turn is about, in hand before the agent has done anything about it, so the pane is
  renamed to it there and then — one line, whitespace collapsed, cut to
  `agent::WAITING_DETAIL_CHARS`. Before this a pane was named only when the turn *ended*,
  from the model's summary title, so for the whole time an agent was working the tab bar
  said `claude`. The summary title replaces the prompt at the end of the turn, when there
  is something better to say about it. Both renames keep the same guard,
  `title_is_ours_to_take`: a pane whose kind is anchored only in its title is left alone,
  because writing over that title would un-detect the pane — a hook-connected pane is
  anchored in its record instead, which `classify` reads first.

### The summary is made from what the agent said

A `Stop` carries `last_assistant_message` — Claude Code's own description of the field
is "avoids the need to read and parse the transcript file" — so for a pane with a feed
the idle summary no longer comes from the screen at all:

- **The trigger is the `Stop`.** No settle wait, no `worth_summarizing` on screen text,
  no baseline comparison. A `Stop` fires once per turn, so it *is* the edge, and it
  cannot ask twice about one turn. There is no cooldown either: three short questions in
  a minute are owed three answers.
- **The input is the run so far.** The request carries the latest `UserPromptSubmit`, the
  prior accepted summary, and the meaningful hook events since it. That keeps each request
  bounded while a long session's summary rolls forward at every turn end.
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
  keeps the name its prompt gave it. Before the feed there was nothing to fall back *to*: a screen tail is
  not a summary of anything.
- **A `Stop` carrying no final message buys nothing, and costs nothing.** The turn still
  lands as `idle` the moment it ends; there is simply no sentence to make one out of. An
  `Interrupt` is the same, and deliberately makes no request at all: a turn the user
  cancelled has nothing to describe.

### The run history is retained

The same hook stream also builds one durable run per agent `session_id`. It stores metadata
(agent kind, pane, folder and timestamps) and only the conversation useful after the fact:

- prompts and plans;
- explicit questions from `AskUserQuestion` and `request_user_input`.

Tool calls, permissions, errors, subagent boundaries and routine lifecycle events are omitted.
Final results are retained as hidden summarization input while a run is active, so a failed or
superseded request can carry them into the next summary. If the run is archived first, its
unsummarized results are drawn as the durable fallback; accepted results remain hidden. Tool
output is never stored. The retained entries stay in `shared:hocus` for the Zellij session.
Summarization advances `summarized_revision` but does not replace or delete entries; a summary
somebody asked for is also kept as a `summary` entry of its own, which is never shown back to
the model. Legacy hook
scripts without a session ID get a pane-and-start-time fallback ID.

**An entry keeps the whole of what the event said.** The run's metadata is one `agent_run` row,
but its timeline is one `agent_run_entry` row per event — so an entry costs a small insert
rather than a rewrite of everything the agent has said so far, and there is nothing to be
bought by cutting it. It used to be cut: a prompt stopped at 600 characters and a question at
80, the latter being `WAITING_DETAIL_CHARS`, which is a *toast's* budget and has no business
bounding a history. The two surfaces are deliberately different now — the sidebar cell and the
toast still get one short line from `waiting_detail`, while the panel gets every question the
agent asked, its header and its options, from `question_text`. The only bound left is at
delivery: the panel is sent a run's newest `RUN_DETAIL_ENTRIES` entries, and no more than
`RUN_DETAIL_BYTES` of them, because that travels as one JSON string into its memory. A row
count alone bounds nothing once an entry can be a whole plan, so both halves are needed.

A run recorded before the split still carries its timeline inside its `payload`, and is read
back from there; there is no migration, and its entries are written out as rows the next time
the run is persisted.

The agents panel is two permanent halves with a rule between them: the list on top, and the
events history of whatever the cursor is on below. The list shows active runs on their live
agent rows and completed runs under an `archive` header, newest first. It loads 50 archive rows
at a time; select `load older runs…` and press Enter for the next page. Selecting a row fetches
that run's full history into the bottom half; the rolling summary is the newest row of that
timeline rather than a block above it, and the rule names the run and says how far it is wound
back.

`Tab` moves the keyboard between the halves, and so does a click — the rule is where the focus
shows. `j`/`k` do whatever the half they are in does with a line: walk the selection, or scroll
the history. The arrows always walk the list, and PageUp/PageDown and the mouse wheel always
scroll the history, from either half. Enter focuses a live agent's pane and leaves the panel up,
so the history stays beside the pane it is about; `Esc` puts the whole panel away — the list and
the conversation beside it, which have one visibility between them. `Ctrl e a` brings it back
with the keyboard here, `Ctrl e c` with the keyboard in the conversation. An
archived row has no pane to focus. `s` asks for a summary of the row under the cursor — see
below.

### A summary you asked for

`s` in the panel describes the selected row now. It works on a live agent and on an archived
run, and the answer is filed on the timeline where it was asked for as well as replacing the
rolling sentence.

- **It skips the guards that exist to stop the daemon asking too often** — the cooldown, the
  screen baseline, the settle count. Those are three ways of asking "is this worth a request",
  and a person pressing a key has answered it. What survives is the guard that is not about
  the budget: one request per row at a time, since a reply is correlated strictly and a second
  request would orphan the first.
- **It reads the whole run, not only what is new.** The automatic summary rolls one sentence
  forward at each turn end, so it is made from what the last one has not already been shown. A
  keypress is a different question — "what has this agent been doing" — and answering it with
  the delta would leave a run whose entries were all absorbed answering with silence, which is
  indistinguishable from the key not working.
- **Nothing makes the answer stale.** A screen summary is void the moment the screen moves and
  a turn summary the moment the agent starts something else, because nobody asked for either.
  Somebody asked for this one and is still waiting for it, so it lands whatever the pane has
  done in the meantime.
- **It takes the path the pane already has.** A hook-fed pane is described from its run; a
  screen-only one from a fresh reading of its pane, refused if the pane is scrolled back off
  its live tail or cannot be read. Asking a hook-fed pane's screen would reintroduce the
  second, worse answer the feed was brought in to retire.
- **Only manual summaries are filed on the timeline.** An automatic one would grow a long run
  a line per turn, and the newest of them is drawn as the timeline's newest row already. Two presses that produce
  the same sentence file it once.
- **A refusal is silent**, and there are several: `summaries "false"`, a pane that has closed
  or gone back to its shell, a run with nothing retained, a screen-only pane with no API key
  (there is no honest local fallback — `local_summary` shortens prose the agent wrote, and a
  screen tail is not that). A hook-fed pane with no API key does get one, made from the newest
  message its run kept.
- **An archived run has no pane**, so its lane is keyed by run id end to end: no rename, no
  toast, no entry in the live summaries map, and the answer written straight back to the store.
  The pane id the run carries names whoever holds that number today.

### A waiting agent interrupts

The `!` in a sidebar was only ever half the answer; the sidebar is very often not what is
being looked at. On the edge into a wait the daemon toasts, over whatever pane the user
is actually in, and the toast focuses the waiting pane when clicked:

A screen-only agent can still have its quiet pane summarised as awaiting input, and that
guess still drives its `!` marker and `awaiting_confirmation`. It does not raise a toast:
only an explicit hook event is reliable enough to interrupt the user over a wait.

```
agent waiting · 7 · claude◆
Which library should we use for date formatting?
```

Three rules, each the same one the summary toast already keeps — and a fourth this one needs
on its own:

- **Not over the pane it describes.** If the user is focused there, they can see it.
- **The edge, on the phase alone.** `PermissionRequest` puts the prompt up and the
  `Notification` for that same prompt follows ~6s later, wording it differently — so
  comparing the two descriptions is exactly the comparison that would let one prompt
  interrupt twice. Nothing is lost: between two genuine questions the agent *works*, and
  that `busy` is what re-arms the edge. For the same reason a `Notification` never
  overwrites a `waiting_on` that is already set — it fills a gap, it does not replace.
- **Not for a prompt the mode answers itself** — and only for those. A mode answers a
  *permission prompt*: `bypassPermissions` and `dontAsk` answer every one, resolving to a
  blanket allow and a blanket deny, and `acceptEdits` answers only the file-editing tools, so
  a `Bash` prompt under it blocks exactly as it would under `default` and still interrupts. An
  `AskUserQuestion`, an `ExitPlanMode`, an MCP `Elicitation` and every waiting `Notification`
  are never suppressed under any mode: the first three address a person directly, and the last
  fires only once its prompt has gone six seconds unanswered, which is itself the proof
  nothing answered it. The row draws the wait either way — it is what the agent said.
- **`auto` is silenced on the odds, and the backstop is what settles up.** It resolves to a
  *classifier*, not to an allow: it clears what it judges safe and puts the rest in front of
  the user exactly as `default` would. Silencing its requests is still right — most of them
  really are answered — but a request has by then moved the feed to `waiting`, so the
  `Notification` behind it has no busy→waiting edge left to cross, and a genuinely blocked
  agent would go unannounced. So a wait drawn but not announced *holds its edge open*
  (`suppressed_waits` in the daemon) until something spends it: the backstop toasts on the
  question the request itself worded, or the agent goes back to work and the wait is over.

### And the pane itself says so

A toast is over whatever pane you are in, and the sidebar row is somewhere else again. The
third surface is the pane the wait is actually in: while an agent there is blocked on a person,
its frame blinks a 🔔 in the top-right corner, beside where SCROLL and PIN are drawn.

- **It is armed on the edge, like everything else here.** The not-waiting → waiting edge, so
  the `Notification` behind a `PermissionRequest` — the same prompt, reported twice — does not
  restart the blink half way through a phase.
- **`waiting_needs_a_person` decides, and now the record carries its answer.** A prompt that
  `bypassPermissions` or `acceptEdits` is about to answer itself is a wait everywhere it is
  *drawn* — the row and the manifest take the agent at its word — and blinks at nobody. That is
  a seventh reported field, `AgentInfo::wait_needs_a_person`: the event that decided it is long
  gone by the time a pane draws anything, so the answer travels with the record and warps with
  it. The two roads that are not a hook say it for themselves: `agent-report --state waiting`
  sets it (a script that troubles itself to say `waiting` means a person), and a plugin's
  `set_pane_agent` leaves it `false` unless it names it, because registering a pane *as* an
  agent is not reporting a prompt anybody is looking at.
- **Answering it takes it down, not the agent noticing.** Typing into the pane, or simply
  focusing it, ends the blink at once rather than waiting for the next hook to say `busy`. The
  record is untouched by either — a person answering the prompt is not the agent saying
  anything, and until the agent says so the row is right to read `waiting`.
- **A wait that arrives while you are already in the pane never blinks**, which is the rule the
  toast keeps too: if you are focused there, you can see it.
- It needs frames. A layout with `pane_frames false` has no title line to draw in, and gets the
  toast and the row as before.

The blink runs on the shared animation clock, at its own cadence between the 60fps one a toast
slides at and the half-second one the plugin loading dots pulse at — see
`Screen::animation_clock_period`. An idle session renders nothing at all, so the clock is what
makes a corner move in a pane that is producing no output; it stops on its own once the last
wait is answered.

## When a feed ends

Nothing here expires on a clock, and that is deliberate: a permission prompt can sit
unanswered all day without the pane's state having changed, and a ten-minute
`cargo build` is one `PreToolUse` and a long silence. A record ends only where an ending
is *observed* — `SessionEnd` clears it, the command exiting marks it `Exited`, the pane
closing takes it with it. The cost is that a single dropped event leaves a pane mislabelled
until the next one arrives, which is what `StopFailure` exists to stop being the common case.

The record going `Exited` is read **even for a pane the daemon hears events from**, which is
the one place the manifest overrules the pipe. An agent killed outright sends no `SessionEnd`,
so without that the feed would never end, and the screen heuristic behind it stays disabled —
a pane parked on `waiting · Bash rm` for the life of the session.

A `session_id` that changes in the same pane is a session replaced rather than ended —
`/clear` — and drops the live summary, in-flight request and held prompt. The previous run is
closed as `replaced` and remains in the archive.

A script from before v4 of the installer pipes the event to hocus-agentd and nothing else
(`zellij pipe --name agent_hook`); the daemon writes the same record through
`set_pane_agent`, so an old install lands in the same place, minus the nine events v4
added. `zellij hocus hooks status` says when the installed script is old.

## Where it lives

| Piece | File |
|---|---|
| The record, and who may write it | `zellij-utils/src/data.rs` (`AgentInfo`, `AgentState`, `AgentSource`, `PaneInfo::agent`) |
| Event → record, and every rule for reading one | `zellij-utils/src/agent.rs` (`HookEvent`, `hook_state`, `hook_detail`, `waiting_detail`, `waiting_needs_a_person`, `hook_outcome`, `WAITING_TOOLS`, `CLAUDE_HOOK_EVENTS`, `CODEX_HOOK_EVENTS`, `AGENT_HOOK_PIPE`) |
| The action, the fan-out, and the installer | `zellij-utils/src/cli.rs` (`AgentReport`), `zellij-utils/src/input/actions.rs`, `src/hocus_hooks.rs` |
| Carrying it on the pane | `zellij-server/src/panes/terminal_pane.rs` (`agent`, `agent_from_run`), `screen/thread_main.rs` (`SetPaneAgent`), `screen/mod.rs` (`fire_agent_exit_hook`, `fire_agent_state_hooks`, `agent_hook_edges`) |
| The blinking bell in the pane's corner | `zellij-server/src/panes/terminal_pane.rs` (`agent_attention_since`, `acknowledge_agent_attention`), `ui/pane_boundaries_frame.rs` (`render_agent_attention`), `screen/mod.rs` (`blinking_agent_panes`, `animation_clock_period`, `advance_agent_attention_blinks`), `zellij-utils/src/animation.rs` (`blink_is_lit`, `BLINK_PERIOD`) |
| What a row draws | `default-plugins/hocus-proto/src/agents.rs` (`HookFeed::from_agent`, `status_cell`, `waiting_marker`, `kind_cell`, `HOOK_MARKER`) |
| What a wait says, and the summary | same (`waiting_toast`, `turn_summary_request_body`, `local_summary`) |
| Run types, reduction and rendering | same (`AgentRun`, `run_entry`, `run_history`, `rows::run_mutations`, `rows::hydrate_entries`) |
| Receiving the broadcast, persisting, publishing, toasting | `default-plugins/hocus-agentd/src/main.rs` (`on_hook_with`, `record_run_event`, `on_turn_end_with`, `finish_local_summary_with`) |
| Summaries somebody asked for | `default-plugins/hocus-agentd/src/main.rs` (`summarize_pane_now_with`, `summarize_run_now_with`, `finish_run_summary`, `Origin::Manual`, `Span`), `default-plugins/hocus-proto/src/agents.rs` (`push_summary_entry`, `RunEntryKind::Summary`), `default-plugins/hocus-agents/src/main.rs` (`request_summary`) |
| Archive paging and selection | `default-plugins/hocus/src/transitions.rs`, `default-plugins/hocus-agents/src/main.rs` |

The pipe's name and its two arguments — the pane, and the kind — are spelled once, in
`zellij_utils::agent`; `hocus_proto::wire` re-exports all three, so the two ends of a broadcast
that is silent when it does not match cannot drift. A broadcast that names no kind is read as a
`claude`, which is what every script old enough to omit it was installed for.

The event payloads are pinned against Claude Code's own shipped schemas rather than
against its documentation. `strings` on the binary finds them: they are Zod definitions,
one per `hook_event_name`, and they are the authority on which fields are optional. Codex's
are not pinned that way. The shared defensive reader still takes a `request_user_input`'s
`questions[0].question`, or its `header` when no question is present; if that shape changes,
the wait falls back to "a question" rather than mistaking another field for the prompt.

Note: `zellij hocus hooks` is unrelated to the `hooks { … }` block in zellij's own
config, which binds a plugin to a moment in the session's life
(`zellij-utils/src/input/hooks.rs`).
