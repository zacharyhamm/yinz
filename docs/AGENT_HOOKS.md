# Coding-agent hooks → the pane's agent record

`hocus-agentd` normally works out what an agent pane is doing by *looking at it*: every
five seconds it reads the pane's scrollback, hashes the word sequence, and calls the
pane busy if the hash moved. That is all it can tell — whether the agent is *blocked on
the user* is not observed at all, and neither is what it has been doing.

That is the only thing that works for an agent which says nothing about itself, and it is a
guess made from a picture. Claude Code and Codex say. Install their hooks and a `claude` or a
`codex` pane reports its own state instead — and, when its session ends, gets one summary of
what the whole session was:

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

One script covers both a local pane and a remote one. It looks for
`ZELLIJ_REMOTE_PANE_TOKEN` **first**, and reports through the broker when it finds one
("Remote panes", below); only failing that does it read `$ZELLIJ_PANE_ID`. That order is the
behaviour, not a preference: a remote pane carries a `ZELLIJ_PANE_ID` as well, and the one it
carries is the *asking* session's, on a machine that has no such session - reading it there
sends every event to whatever session happens to be running on that box, under a pane id that
means something else.

`agent-report` reads the event, works out what it means (`zellij-utils/src/agent.rs`,
`hook_outcome`) and sets the pane's **agent record** — `PaneInfo.agent`: the kind,
the state, the tool it is running, what it is blocked on, its folder and its session id. The
action normally reads the pane from `$ZELLIJ_PANE_ID`. A locally supervised pane also inherits
`$ZELLIJ_PANE_REF`, the stable id of its pty-host: before routing an implicit report, the action
asks that supervisor for the pane's current session and terminal id. An `Attach` re-keys both,
so hooks keep reporting after the pane is warped even though the process environment still holds
the session-local values it started with. Explicit `--session` or `--pane-id` targets still win;
non-detachable panes keep the existing environment route. Remote panes have a route of their
own, below. The pane's current agent
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
plugin data store — with the default SQLite backend, one file per *machine*, not per session, so a run written in one
session is legible in another; with `plugin_data_url` pointing at Postgres, one database per
however many machines share it. What a warped pane needs is not the history but its *name*, and
that is what travels.

### Reading it back

"Legible in another" was for a long time only true in principle: the agents panel drew the
newest entries of the run it had selected, and nothing could look anything up. The in-session
model can now ask, through three read-only tools in `hocus`:

| Tool | Answers |
|---|---|
| `agent_runs` | Which runs exist — newest first, with filters. The way in. |
| `agent_run` | One run's timeline, in order, paged. Takes a `run_id` or a live `pane_id`. |
| `agent_history_search` | Full-text over every entry ever recorded. |

**The default scope is the machine, not the session**, which is the store's shape rather than a
choice: one database holds every zellij server on the machine, so a run recorded in a session
that has since been closed is as readable as one still going. `this_session`, `project`, `kind`,
`since` and `active` narrow it.

A search hit names a `run_id` and a `revision` — together the entry's own row key,
`"{run_id}#{revision}"` — and `agent_run` takes both, reading a short lead-in *before* the
anchor so a result arrives with the prompt it was answering. That round trip is the whole
navigation model, and it is exact rather than heuristic.

What makes the search possible is one attribute: `agent_run_entry.text` is declared
`.full_text()` (`hocus-proto/src/store.rs`). Declaring it migrates the entity, and the
migration rebuilds the FTS index from the rows already in the table — so history recorded
before the index existed is searchable the first time a new build loads, with no import step.
The index is external-content, so it stores no second copy of the text.

The reading half lives in `hocus-proto/src/run_history.rs`, apart from `agents.rs` because
reading is a different job from recording. It never writes. Its one discipline worth knowing:
a local tool result is not passed through hocus's `MAX_TOOL_OUTPUT`, so each page budgets its
own size, cutting entries to a per-entry cap once a page holds more than one — and a page of
*one* is exempt, which is how a single enormous plan is read whole.

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

**The same command also sends the event itself**, typed, as `Event::AgentEvent` to every plugin
subscribed to it - and to `zellij events --type agent_event`. A record is deliberately smaller
than an event: the agent's own final message on a `Stop`, the prompt behind it on a
`UserPromptSubmit`, the permission mode on every one of them — none of those belong on a
manifest that every plugin is handed on every pane update, and all three are what a run's
history and a well-judged interruption are made of. So one process spawn does both. The event
is read once, in the session (`agent::AgentEvent::from_hook`): `pane_id`, `kind`, `hook`, the
`state` the record moved to, `detail`, `prompt`, `last_message`, `tool_name`, `cwd`,
`session_id` - and `raw`, the agent's own JSON, for the one reader that keeps a run's history
and needs the rest. The whole event is bounded (`agent::HOOK_BROADCAST_MAX_BYTES`, 64 KiB): over
it `raw` is pruned, prose to 8 K characters, and `prompt` and `last_message` are cut the same way,
so the typed fields and the JSON never disagree. It goes only to subscribers, gated on `read_application_state` like the
manifest, and nothing waits on it, so a hook cannot stall the turn it fired in. A Lua plugin
reads it with `zellij.agent.on`, or awaits it with `zellij.async.turn` (`docs/LUA_PLUGINS.md`,
"Agents").

`raw` has one bound on it, `agent::HOOK_BROADCAST_MAX_BYTES` (64 KiB). Two of the fields
Claude Code sends have no size limit in its own schema — `tool_input`, so a `Write` of a
two-megabyte file, and `tool_response`, so a `Read` of anything — and this payload is copied
into every subscribed plugin's memory, twice per tool call, inside a hook with five seconds to
live. An event over the cap goes out through `HookEvent::pruned` instead: every field it
models survives, with each string in `tool_input` cut to 256 characters and a budget spent
across the rest of it, which leaves everything any reader actually takes from it — the `Bash`
command, the `file_path`, the first `AskUserQuestion` question. A prompt, a plan, a question or
a final message is prose somebody reads at length, so each of those is cut to 8 KiB instead. Anything the struct does not model,
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
| `UserPromptSubmit` | both | busy | Carries `prompt`: names the pane at once, and is kept for the summary made when the session ends |
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
| `Stop` | both | idle | Every turn end. **Carries `last_assistant_message`**, kept on the run's timeline |
| `StopFailure` | claude | idle | The turn died on an API error — the only thing that says so |
| `Interrupt` | codex | idle | A turn the user cancelled, which ends it just as flatly |
| `SessionEnd` | both | — | Clears the record, ends the feed, closes the run and buys its one summary. `agent_exit` still fires |

The reading is by event and tool name, never by agent: one function answers for both, because
the record `agent-report` writes and the feed hocus draws are two different readers of it, and
a mapping only one of them knew would have the second overwrite the first. What Codex does not
send it simply never sends - it has no `Notification`, no `Elicitation` and no subagents, and
its question goes up as an ordinary tool call.

`PermissionRequest` is the primary waiting signal and `Notification` the backstop: the
notification only fires after ~6s of the prompt going unanswered, which is most of what
the feed exists to remove. That six seconds is a plain timer armed when the dialog goes up,
not a verdict: under `auto` the classifier is routinely still deciding a `Bash` call when it
fires, and the notification carries no `permission_mode` of its own. Its `notification_type`
is matched against an allowlist —
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

## Remote panes: reporting through the broker

A hook inside an ssh or Firecracker pane runs on the other machine, where there is no session to
send `zellij action agent-report` to. It reports to that machine's remote pane broker instead:

```sh
printf '%s' "$payload" | zellij --pty-broker report claude   # a claude in a remote pane
```

This is the branch the installed script takes by itself, so `zellij hocus hooks install` on the
far machine is all a remote pane needs. **A Firecracker VM installs it without being asked**: the
provider runs the install in the guest when the VM is provisioned (`provision_guest` in
`zellij-server/src/firecracker/provider.rs`), wiring both agents, so a `claude` or a `codex`
typed at a shell in a VM pane reports like any other. The install runs again whenever a session
*joins* a VM that is already up, which is the only moment a VM left running across a Zellij
upgrade meets the new one; it rewrites rather than appends, so joining a current VM changes
nothing. Codex still has to be told to trust what was written - `/hooks`, once per VM.

The broker put `ZELLIJ_REMOTE_PANE_TOKEN` in every pane's environment - and only in a pane it
minted one for: a pane's spawn drops any token it would otherwise inherit, so a zellij session
started *inside* a remote pane gives its own panes none, and an agent in one of them reports
that pane rather than the VM pane around it. The report names the pane by the token, on a
connection of its own, and the broker relays the event to the pane's server on the
pane's control channel (`BrokerToServerMsg::AgentReport`). The server applies it exactly as it
would a local report: the pane's agent record is set and the event goes to the plugins on the
hook pipe. Because the event arrives over the transport rather than out of the pane's bytes,
nothing a program prints into the pane can forge one, and ssh and Firecracker panes are treated
alike. A report for a pane no server is attached to is refused, not queued, as a local hook's
would be with no session. hocus hands a spawned `claude` in a VM **no** inline `--settings`, for
the same reason a person should not install into both a user file and a project one: Claude Code
merges hooks across settings sources rather than overriding, so a second copy would fire every
event twice. The consequence to know about is that `hooks: false` on a spawn no longer suppresses
hooks inside a VM - they belong to the guest, not to the launch.

## What the feed changes

hocus-agentd reads a feed two ways, and prefers the first. A pane whose events it hears as
`agent_event` is read from those, because the event stream carries what a record cannot — see
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
  said `claude`. A summary somebody asks for replaces it with its title, when there is
  something better to say about it. Both renames keep the same guard,
  `title_is_ours_to_take`: a pane whose kind is anchored only in its title is left alone,
  because writing over that title would un-detect the pane — a hook-connected pane is
  anchored in its record instead, which `classify` reads first.

### A prompt is not always something a person typed

`UserPromptSubmit` carries "what the user just asked for", and sometimes nobody asked. Claude
Code wakes an agent whose background task has finished by submitting an envelope on its behalf:

```
<task-notification>
<task-id>a10fe836c6a9fa4ca</task-id>
<output-file>/private/tmp/claude-501/…/a10fe836c6a9fa4ca.output</output-file>
<status>completed</status>
<summary>Agent "/code-review" finished</summary>
<note>…boilerplate addressed to the agent…</note>
<result>…possibly tens of kilobytes…</result>
</task-notification>
```

It also staples a `<system-reminder>` onto prompts people did type, and spells a slash command as
`<command-name>` plus `<command-args>` plus `<local-command-stdout>`. Taken at face value, one of
those names the pane `<task-notification> <task-id>a10fe836…` for the whole of a turn, fills the
four rows the agents pane gives a timeline entry with opening tags, and is handed to the
summariser as the question the turn was about.

`hocus-proto`'s `envelope` module reads such a prompt apart — **without changing what is
recorded**. `run_entry`'s "Nothing here is cut" still holds: the raw prompt is what the store
keeps, what `HistoryEvent::text` carries, and what the reader's `r` shows. Four readings are
computed where each is wanted:

| Reading | Who asks | What it gives |
|---|---|---|
| `digest` | `agents::run_history` | markdown for the timeline row: status glyph, short task id, summary, and a row saying what is folded away |
| `title` | `agents::prompt_title`, `run_title` | `task · Agent "…" finished`, for the tab bar and the archive row |
| `question` | `hocus-agentd`'s `kept_prompt` and `run_delta` | a sentence for the summariser, with the note and the path dropped. Both the header a request carries and the run history under it go through it, so one request cannot say two things about what the turn was for |
| `sections` | the agents pane's reader | the folds `Enter` opens, `o` opens a file from, and `y` copies |

`digest` and `sections` are both asked only of a `prompt:` entry, which is the one kind a harness
writes as well as a person. An agent's own message quoting a `<system-reminder>` at the start of a
line is prose *about* markup: the timeline draws it as it arrived, and the reader opens it whole.

A run's title prefers the first prompt carrying any **prose**, so a run woken by a task and later
typed into by a person is named for what the person typed.

Recognition is deliberately narrow: an envelope has to open a line of its own and its tag has to
be lowercase *and hyphenated*, so `Vec<String>`, `if x < y` and `<b>` in somebody's prompt are
prose and stay prose. A tag the module has never seen still gets a readable row and an openable
fold rather than its own markup — `envelope::KINDS` is the table, and adding a kind is a function
per reading and a line in it.

The one bend in "opens a line of its own" is indentation, and only for a tag already in that
table: the slash-command expansion indents `<command-message>` and `<command-args>` by twelve
spaces, and `text::sanitize` turns a leading tab into four more, so a rule wanting the `<` in
column zero loses two thirds of that shape to prose — where it is neither escaped nor cut. A tag
nobody registered still has to start its line, which is what keeps an indented code block prose.
Which of the three elements a harness writes first does not matter either: they are absorbed into
one envelope in both directions, once per tag, so a second `<command-message>` is a second
command rather than a repeat of the first.

A `UserPromptSubmit` carries no `tool_use_id`, so a prompt used to be compared only against the
entry immediately before it and a redelivered notification could land twice. A task notification
now supplies its own identity — the task id plus a hash of the element *as the entry records it*
— which the existing whole-run dedupe uses. The hash is the point: the envelope's own note says
the same task id may notify more than once, and a resumed agent reports again with a *different*
result, which is a second thing happening rather than a second delivery of the first. A prompt
carrying two notifications is identified by both, because a prompt is one delivery.

Two conditions bound it. The hash is taken over the **recorded** text — `run_entry`'s sanitised,
trimmed prompt — and not over the raw payload, so an entry's id can always be worked out again
from the entry. That is what makes the comparison mean anything across a restart, when a run's
entries are read back from the store, and it is what lets a run recorded before the field existed
be compared at all: such a run comes back out of its JSON `payload` blob with no ids on it, and
`agents::entry_event_id` recomputes them rather than matching nothing forever.

And an element that arrived **cut** has no identity whatever. The 64 KiB broadcast cap takes a
long `<result>` at a fixed 8 KiB with no marker, so two different reports for one task can arrive
byte for byte the same, and the bytes that would have told them apart are exactly the ones that
did not arrive. Such a prompt falls back to the adjacent-only rule. The cost is a large
notification redelivered across a turn showing twice; the alternative was a real report being
dropped and never drawn, because the ingest path drops on a match rather than showing one.

### The summary is made once, when the session ends

Nothing is summarised on a schedule. The daemon buys exactly one summary of its own accord,
for a pane whose agent reports for itself, when that agent's `SessionEnd` says the session is
over:

- **The trigger is the `SessionEnd`.** No settle wait, no `worth_summarizing` on screen text,
  no cooldown, no baseline comparison — and no per-turn summary either. A turn ending is a
  fragment of a session, and a sentence about one fragment is not what anybody reads. A
  `SessionEnd` fires once, so it *is* the edge, and it cannot ask twice about one session.
- **The input is the whole run.** The request carries every prompt, plan, question and result
  the run's timeline kept, and none of the tool calls — `run_delta`, cut to
  `SUMMARY_LINES`/`SUMMARY_CHARS`. The run's own prior summary is left out on purpose: the
  entries beside it are what that summary was made from, and including it would ask the model
  to weigh its own paraphrase against its source.
- **It goes down the *run* lane, not the pane lane.** By the time this fires the pane is a
  shell prompt where an agent was, so there is nothing to rename and nothing to toast. The
  request is correlated by run id (`summarize_run_now_with` → `finish_run_summary`) and the
  answer is written to the run row, which is where the agents panel draws it from. The run
  the `SessionEnd` just closed is sitting in `dirty_runs`, which `load_run` reads first, so
  no store round trip is needed to build the request.
- **A session with an empty timeline buys nothing.** Every hook it saw was a tool call or a
  lifecycle event; asking a model to describe nothing would cost a round trip to be told so.
- **`summaries "false"` switches it off**, as it switches off the key.

### Nothing else is summarised unless somebody asks

The screen-diff heuristic keeps running, but only for the idle *clock*: `activity` is what
`status_cell` counts minutes with, so a hook-fed pane's `idle 3m` reads the same as everyone
else's. It never buys a request.

That includes an agent with no hooks at all. Such a pane gets no summary and no
"blocked on the user" marker on its own — the marker was only ever a model's guess made from
120 lines of terminal, and it is not worth a request per settled pane per minute. Pressing the
summarise key still buys one; see [A summary you asked for](#a-summary-you-asked-for).

With no API key a hook-fed pane still has an honest answer: `local_summary` takes the agent's
last message, drops fenced blocks and markdown markers, keeps the opening sentences and gives
no title, so the pane keeps the name its prompt gave it. A screen-only pane has none — a screen
tail is not a summary of anything — and gets nothing rather than an invention.

### The run history is retained

The same hook stream also builds one durable run per agent `session_id`. It stores metadata
(agent kind, pane, folder and timestamps) and only the conversation useful after the fact:

- prompts and plans;
- explicit questions from `AskUserQuestion` and `request_user_input`.

Tool calls, permissions, errors, subagent boundaries and routine lifecycle events are omitted.
Final results are retained as hidden summarization input while a run is active, so every turn's
result reaches the summary made when the session ends. If the run is archived first, its
unsummarized results are drawn as the durable fallback; accepted results remain hidden. Tool
output is never stored. The retained entries stay in `shared:hocus` for the Zellij session.
Summarization advances `summarized_revision` but does not replace or delete entries; a summary
is also kept as a `summary` entry of its own, which is never shown back to the model. Legacy
hook scripts without a session ID get a pane-and-start-time fallback ID.

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
that run's full history into the bottom half; a summary is the newest row of that timeline
rather than a block above it, and the rule names the run and says how far it is wound back.

Two rows are pinned under that rule, above the timeline, and do not scroll with it: the exact
date and time the session started, and its **whole** session id. Both are things the timeline
cannot say. The rule's `short_run_id` keeps eight characters of a uuid, which tells one of a
pane's runs from another but is not an id you can paste into a `--resume`, a log grep or an
`agent_run` call; and every row below the header wears a *relative* age (`12m`, `3h`, `5d`),
which stops being an answer the moment the question is which afternoon a run was. A half with
no room for both the header and a line of timeline drops the header rather than squeezing the
timeline, and a run whose agent supplied no session id is headed by its run id instead — that
shape is keyed by pane and start time, and calling it a session would be a claim it cannot
make.

Pressing `r` on an archive row resumes that run rather than making you paste its id anywhere:
the divider asks `resume this run? y/n`, and `y` opens a pane running the same agent in the
folder the run recorded, resuming the session — `claude --resume <id>`, `codex resume <id>`,
`cursor --resume <id>`, whichever the run's kind spells. It asks first because it is the only
key in the panel that starts a process and it sits beside the ones the list is walked with;
`n`, `Esc` or simply walking on drops the question. The panel names the row and nothing else
(`wire::AGENT_RESUME`), and hocus — which holds the archive headers, each with its session id,
kind, cwd and host — decides what resuming it means. A refusal comes back
(`wire::AGENT_RESUME_RESULT`) and is shown on the divider, which it takes the whole of: a run
whose hook script reported no session id has nothing to resume, an agent kind with no resume
spelling cannot be asked for one, a run recorded in a Firecracker VM cannot be rebuilt from
what it stored, and a run recorded in a folder the plugin mounts claim — anything under
`/tmp`, `/host`, `/data` or `/cache` — cannot be opened where it was, because the host reads a
plugin's local paths through those mounts and would put the pane somewhere else entirely. A
run that went live again between the row being drawn and the `y` is refused too: one resumed
or warped into *this* session by naming the pane it is in, and one picked up by another zellij
session sharing the store by reading the run's record back rather than the archive page the
row was drawn from — that page is a snapshot, and stale by however long it has been up. Live
agent rows ignore `r` for the same reason: `Enter` already walks to the pane, and a second
process on one session id would be two agents writing one conversation.

Because a resumed agent keeps its session id and is handed the hooks like any spawned one, the
daemon adopts the *same* run back: its run id is unchanged, so the row leaves the archive and
reappears among the live agents rather than a second run starting beside it.

The start time is drawn as local wall clock, which a plugin cannot work out for itself: wasm
has no timezone, WASI inherits none of the session's environment, and nothing that reads a
zoneinfo database links into a plugin. So there is one host command for it,
`get_local_utc_offset(epoch_ms)`, answered server-side with `chrono`. It is asked about the
instant being drawn rather than about *now*, so a run that began before a daylight-saving
change reads back with the offset that was in force then. It needs no permission — an offset
from UTC says less about the machine than `GetPluginIds` already does — and the offset it
returns is the *server's*, so a session attached from another timezone reads times as the host
keeps them. The offset is always named in the output (`+01:00`), so a fallback to UTC is
legible as UTC rather than silently an hour or eight wrong.

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

`s` in the panel describes the selected row now, and it is the only thing that summarises an
agent while its session is still running. It works on a live agent and on an archived run, and
the answer is filed on the timeline where it was asked for as well as becoming the row's
summary.

- **There is nothing to get past.** Nothing is bought on a schedule any more, so there is no
  cooldown, screen baseline or settle count standing between the key and a request. The one
  guard that survives is not about the budget: one request per row at a time, since a reply is
  correlated strictly and a second request would orphan the first.
- **It reads the whole run, not only what is new.** A run whose entries an earlier summary
  already absorbed would otherwise answer with silence, which is indistinguishable from the
  key not working.
- **Nothing makes the answer stale.** Somebody asked for it and is still waiting, so it lands
  whatever the pane has done in the meantime.
- **It takes the path the pane already has.** A hook-fed pane is described from its run; a
  screen-only one from a fresh reading of its pane, refused if the pane is scrolled back off
  its live tail or cannot be read. Asking a hook-fed pane's screen would reintroduce the
  second, worse answer the feed was brought in to retire.
- **The answer is filed on the timeline**, because *when* somebody asked is half of what it
  says. Two presses that produce the same sentence file it once.
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

A summary somebody asked for can still come back marking a pane as awaiting input, and that
guess still drives its `!` marker and `awaiting_confirmation`. It does not raise a toast:
only an explicit hook event is reliable enough to interrupt the user over a wait. Since
nothing summarises a screen unasked any more, an agent with no hooks raises no `!` of its own.

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
  blanket allow and a blanket deny; `auto` answers most of them, through a classifier that
  clears what it judges safe and puts the rest in front of the user; `acceptEdits` answers only
  the file-editing tools, so a `Bash` prompt under it blocks exactly as it would under `default`
  and still interrupts. An `AskUserQuestion`, an `ExitPlanMode` and an MCP `Elicitation` are
  never suppressed under any mode: they address a person directly. The row draws the wait
  either way — it is what the agent said.

- **A promise is held to; a guess is not.** A silenced request has by then moved the feed to
  `waiting`, so the `Notification` behind it has no busy→waiting edge left to cross. Where the
  silence was a *promise* — `bypassPermissions` and `dontAsk`, which resolve every prompt, and
  `acceptEdits` on a file-editing tool — that edge is held open for the backstop instead
  (`suppressed_waits` in the daemon, gated on `agent::suppression_is_a_guarantee`): a dialog
  still up six seconds later means the promise did not hold — a `deny` or `ask` rule, a
  `PreToolUse` hook that asks anyway, an edit outside the workspace — and the toast goes out in
  the question the *request* worded, which is better than the notification's "Claude needs your
  permission to use Bash". Under `auto` nothing is held and the notification says nothing. It
  used to: a notification that fired at all was taken as proof the classifier had refused. But
  it is a six-second timer Claude Code arms when the dialog goes up, the classifier is routinely
  still deciding a `Bash` call when it fires, and it names no `permission_mode` for anyone to
  read — so toasting on it meant a `Bash cargo` interruption for a prompt that approved itself a
  moment later. A prompt the classifier does refuse is still drawn in the row and marked `!`; it
  is just not interrupted over. A notification with no request before it — a Claude Code without
  the `PermissionRequest` hook — enters the wait itself and is announced as before.

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
  agent is not reporting a prompt anybody is looking at. One seam remains, and is accepted:
  the record is written per event by a CLI that keeps no state, and the `Notification` behind a
  request `auto` silenced names no mode, so its record says a person is needed and the bell
  starts on it six seconds in where the toast does not — the daemon has seen the request and the
  pane has only the record. Closing it would mean the record carrying *which kind* of silence it
  was, a field on `AgentInfo` and so on the plugin and IPC wire, to spare a blink that is
  passive and that the agent's next `busy` report clears by itself.
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

A script from before v4 of the installer piped the event to hocus-agentd on a pipe that no
longer exists; it sets no record and feeds nothing. `zellij hocus hooks status` says when the
installed script is old, and `zellij hocus hooks install` rewrites it.

## Where it lives

| Piece | File |
|---|---|
| The record, and who may write it | `zellij-utils/src/data.rs` (`AgentInfo`, `AgentState`, `AgentSource`, `PaneInfo::agent`) |
| Event → record, and every rule for reading one | `zellij-utils/src/agent.rs` (`HookEvent`, `hook_state`, `hook_detail`, `waiting_detail`, `waiting_needs_a_person`, `hook_outcome`, `WAITING_TOOLS`, `CLAUDE_HOOK_EVENTS`, `CODEX_HOOK_EVENTS`, `AGENT_HOOK_PIPE`) |
| The action, the fan-out, and the installer | `zellij-utils/src/cli.rs` (`AgentReport`), `zellij-utils/src/input/actions.rs`, `src/hocus_hooks.rs` |
| Carrying it on the pane | `zellij-server/src/panes/terminal_pane.rs` (`agent`, `agent_from_run`), `screen/thread_main.rs` (`SetPaneAgent`), `screen/mod.rs` (`fire_agent_exit_hook`, `fire_agent_state_hooks`, `agent_hook_edges`) |
| The event, typed, and its route to plugins | `zellij-utils/src/agent.rs` (`AgentEvent`, `from_hook`), `zellij-utils/src/input/actions.rs` (`hook_report_actions`, `Action::AgentEvent`), `zellij-server/src/route.rs` (to the plugin thread as `Event::AgentEvent`), `zellij-server/src/pty.rs` (`handle_remote_agent_report`, the broker's road to the same event) |
| The blinking bell in the pane's corner | `zellij-server/src/panes/terminal_pane.rs` (`agent_attention_since`, `acknowledge_agent_attention`), `ui/pane_boundaries_frame.rs` (`render_agent_attention`), `screen/mod.rs` (`blinking_agent_panes`, `animation_clock_period`, `advance_agent_attention_blinks`), `zellij-utils/src/animation.rs` (`blink_is_lit`, `BLINK_PERIOD`) |
| What a row draws | `default-plugins/hocus-proto/src/agents.rs` (`HookFeed::from_agent`, `status_cell`, `waiting_marker`, `kind_cell`, `HOOK_MARKER`) |
| What a wait says, and the summary | same (`waiting_toast`, `run_summary_request_body`, `local_summary`) |
| Run types, reduction and rendering | same (`AgentRun`, `run_entry`, `run_history`, `rows::run_mutations`, `rows::hydrate_entries`) |
| The header pinned above a run's timeline | same (`run_header`, `wrapped_field`, `wall_clock`, `civil_from_days`, `HistoryLines::header`), drawn by `default-plugins/hocus-agents/src/main.rs` (`header_rows`, `render_history`) |
| The local UTC offset a wall clock needs | `zellij-tile/src/shim.rs` (`get_local_utc_offset`), `zellij-server/src/plugins/zellij_exports.rs` (`get_local_utc_offset`), `zellij-utils/src/data.rs` (`PluginCommand::GetLocalUtcOffset`) |
| Receiving the broadcast, persisting, publishing, toasting | `default-plugins/hocus-agentd/src/main.rs` (`on_hook_with`, `record_run_event`, `summarize_run_now_with`, `finish_local_summary_with`) |
| Summaries somebody asked for | `default-plugins/hocus-agentd/src/main.rs` (`summarize_pane_now_with`, `send_run_summary`, `summarize_run_now_with`, `finish_run_summary`, `run_delta`), `default-plugins/hocus-proto/src/agents.rs` (`push_summary_entry`, `RunEntryKind::Summary`), `default-plugins/hocus-agents/src/main.rs` (`request_summary`) |
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
