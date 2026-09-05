# Self-programming: what the workstream set out to do, and what landed

The `self-programming-vision` branch turned this fork into a terminal workspace that can drive
and rewrite itself: coding agents (Claude Code first; codex and cursor alongside) are spawned,
observed and steered by the system, and the system's own behaviour - plugins, hooks, keybindings,
config - can be rewritten at runtime by those agents, or by the in-session model (`hocus`),
without rebuilding the binary. This document is the summary of that work: the goal, the gaps it
found, what landed where, and the notes a later session needs. It replaces the roadmap it grew
out of (`docs/SELF_PROGRAMMING_ROADMAP.md`, in history).

Two scope decisions framed it. **Self-modification means the Lua/config layer**, not rebuilding
the Rust binary; a rebuild-and-reinstall loop is out of scope. **The `assembly-lines` branch stays
parked**; nothing here depends on it, and the agent signal it lacked now exists.

## The gaps the survey found

The fork was further along than its docs admitted - an in-session control plane with sixty-odd
model tools, a launch planner, a state-guessing daemon, Lua plugins over the whole
`PluginCommand` surface, a SQLite store, work queues, runtime reconfigure - but the pieces did not
meet:

1. **"Agent" was not a server concept.** The server gated `agent_exit` on a context key; hocus
   sniffed commands to decide what to list; agent *state* lived only in `hocus-agentd`'s memory.
2. **The hook feed was plugin-private**, hand-threaded through a named pipe with the pane id as a
   string argument, with the event→state mapping spelled in two places.
3. **Observation from outside was polling.** No channel onto the plugin event bus; `zellij run
   --block-until-exit` discarded the exit code.
4. **The authoring loop was blind and hocus-only.** No syntax check before a reload; the plugin
   directory was not a plugin filesystem root, so hocus shelled out to write it.
5. **Config self-write was KDL-only** and refused under a Lua config; no CLI could bind a hook.
6. **Hooks had one shape** - replace this pane with a plugin - and two points.
7. **Permission gating failed open**: a new command or event was ungated by default.
8. **Nothing persisted** in the orchestration layer.

## What landed

Every item below is on the branch, tested, and documented in the user-facing guide it belongs
to (`docs/AGENT_HOOKS.md`, `docs/LUA_PLUGINS.md`, `docs/LUA_CONFIG.md`).

### An agent is a server concept

`PaneInfo.agent: Option<AgentInfo { kind, state, source, detail, cwd, session_id }>` is the one
answer to "what runs in this pane and what is it doing". Three roads write it: the spawn path
(`AgentSource::Spawned`), `PluginCommand::SetPaneAgent` / `zellij.set_pane_agent` (hocus's
`agent_register` writes `Registered`), and `zellij action agent-report` (`Reported`) - what a
Claude Code pane's own hooks call, so `claude` typed at a shell becomes an agent pane the moment
it speaks. `hocus-agentd` derives its feeds from the pane manifest instead of a private map; its
scrollback fingerprinting remains for agents that say nothing about themselves.

The hook wiring uses one shared event reader and one event list per agent. `zellij hocus hooks
install` writes Claude Code hooks to `~/.claude/settings.json` and Codex hooks to
`~/.codex/hooks.json` (or `$CODEX_HOME/hooks.json`). A spawned `claude` receives the Claude list
inline as `--settings`, so it reports with nothing installed. Codex requires its installed hooks
to be reviewed and trusted with `/hooks`.

Entry points: `zellij-utils/src/data.rs` (`AgentInfo`), `zellij-utils/src/agent.rs` (hook
reading, the event list), `zellij-server/src/panes/terminal_pane.rs`, `screen/thread_main.rs`
(`SetPaneAgent`), `src/hocus_hooks.rs`, `default-plugins/hocus-agentd/src/main.rs`.

Where an agent pane *goes* is the server's too. Every spawn route - hocus's tools and modal,
`zellij run -- claude`, a keybinding's `Run` - ends in one `NewPane` on the screen thread, and
`screen/agent_placement.rs` decides there, from the same two signals the record starts from: the
plugin context marker, or a command line that names a known agent (`zellij_utils::agent::KINDS`,
the one list `hocus-agentd` detects with). The rule is a size, not a count: the tab's tiled
panes and the newcomer are imagined sharing the viewport equally, and a share below
`agent_pane_min_columns` x `agent_pane_min_rows` (80x24 by default; 0 turns it off) sends the
pane to the roomiest other tab that clears the floor, or to a tab of its own. Area rather than
rows and columns because an equal share fixes an area and not a shape.

### Observation from outside

- **`zellij events [--type …] [--json]`** streams the state events (`PaneUpdate`, `TabUpdate`,
  `ModeUpdate`, `SessionUpdate`, `CommandPaneExited`, `CwdChanged`, …) to a CLI client. The
  plugin thread keeps the subscriber list, forwards each state report once per batch, and the
  screen reports to a `CLI_EVENTS_ONLY_TARGET` when no plugin wants the manifest, so it works in
  a plugin-less layout. Never `Key` or `PaneRenderReport` - those have `zellij subscribe`.
- **`zellij run --block-until-exit` exits with the child's status**; a plain `--blocking` no
  longer returns 0 when an unrelated pane closes.
- **`PipeMessage.pane_id`**: a `zellij pipe` from a pane carries its pane identity, so no
  CLI→plugin integration hand-threads `pane_id=$ZELLIJ_PANE_ID`. Convenience, not
  authentication.

Entry points: `zellij-client/src/cli_client.rs` (`start_events_client`),
`WasmBridge::forward_events_to_cli_subscribers`, `EventType::CLI_STREAMABLE`.

### The authoring loop, from a pane or from hocus

`zellij plugins write|check|reload|log|dir <name>` is the loop: write a Lua plugin from stdin
(refused if it does not compile), check one offline, reload every running instance without
opening a pane (each reload sends `BeforeClose` to the instance it replaces), read its log
lines. The same code answers hocus's `plugin_write`/`plugin_check`/`plugin_reload`/`plugin_log`
tools, which go through the new `/plugins` filesystem root and the `AuthorPlugins` permission -
the shell script, the 64 KB argv cap and the hand-rolled symlink guard are gone.

hocus's orienting half reads through the same root: `plugin_list` with each plugin's length,
`plugin_outline` (functions, handlers, requires, permissions and tool registrations with line
numbers), a ripgrep-shaped `plugin_search`, and a range `plugin_read`. `plugin_edit` takes one
replacement or an `edits` batch under one confirmation and answers with the lines it changed; a
miss says which lines nearly matched instead of asking for a re-read. `plugin_docs` takes a
`query=` and a page range as well as a section. The outline extractor is
`zellij_utils::plugin_authoring::outline`, beside `lint`, so a `zellij plugins outline` can reuse
it. All of it exists because hocus re-sends the transcript every turn: a page read to find one
function is paid for on every round after it, and a search hit is not.

Server side: `PluginCommand::CheckLuaPlugin`, `ReloadPluginByUrl`, `ReadPluginLog`; the `/plugins`
root in `zellij_exports::fs_roots`; `zellij-tile` gained `fs_read`/`fs_list`/`fs_metadata`/
`fs_write`/`fs_remove`/`fs_create_dir`/`fs_rename`. File work lives in
`zellij_server::plugin_authoring` (`write`, `log_tail`) and
`zellij_utils::plugin_authoring::resolve_plugin_file`, so the CLI, hocus and the tests run one
code path.

### Config that writes itself

`zellij action reconfigure --kdl|--file [--write]` applies a fragment to every attached client;
`zellij action set-hook <point> <url> [--write]` is sugar over it. `--write` and a plugin's
`reconfigure(_, true)` merge into `<config>/config.d/zellij-managed.kdl` - an overlay layered in
name order by both loaders and watched - so programmatic writes persist whatever the user's own
config format is, and a human's `config.lua` stays untouched.

Entry points: `Action::Reconfigure` → `ServerInstruction::ReconfigureAllClients`,
`Config::with_overlay_dir` / `write_managed_fragment` / `merge_kdl_fragment`,
`home::get_config_overlay_dir`.

### Hooks with a headless shape, and more of them

`HookPoint::shape()` is `ReplacePane` for `agent_exit` and `Headless` for the rest. A headless
hook is one long-lived plugin receiving `Event::Hook { point, payload }` per firing
(`deliver_to_headless_hook` in `plugins/mod.rs` is the find-or-load path). New points:
`agent_start` (a record appearing, coming back from `exited`, or a new `session_id`),
`agent_waiting` (the edge into `waiting`), `pane_exit` (any command pane), `session_start`. Enter
in a held pane re-runs through the screen, so a hand-restarted agent fires `agent_start` too.
`zellij:agent-policy` is the shipped example, a Lua hook that logs every firing and notifies on
`agent_waiting`; `docs/LUA_PLUGINS.md` "Headless hooks" opens with binding it.

### Safety: nothing is ungated by omission

`zellij_exports::command_permission` and `wasm_bridge::check_event_permission` match
exhaustively on `CommandType` / `EventType` with no catch-all; a new command or event that is
not classified is a compile error, not a silent grant. The two compatibility holes left open on
purpose are labelled where they are (`ScanHostFolder`, `WatchFilesystem`, the `FileSystem*`
events). `main`'s round-trip properties over every `Action` and `PluginCommand` cover the new
variants.

### hocus: persistence and richer launches

Jobs, agent latches, summaries and hook-backed run histories live in `shared:hocus`
(`store::shared_defs()`), written as each change lands and restored once at startup. Run history
keeps prompts, plans and explicit agent questions under the agent's session id; completed runs
remain selectable in the agents panel archive, and are legible to the in-session model through
`agent_runs`, `agent_run` and `agent_history_search` — machine-wide by default, since the store
is per machine and not per session, so hocus can find what an agent concluded in a session that
has since been closed rather than commissioning the work again. `agent_run_entry.text` is
full-text indexed for it (`agent_run` and `agent_run_entry` are both at version 2); the reading
half is `hocus-proto/src/run_history.rs`, and `docs/AGENT_HOOKS.md` "Reading it back" is the
write-up. A restored job follows its chat, waiting in
`State::held_jobs` until `/history` reopens it. Scheduled runs decline confirmation-gated tools
instead of parking on a y/n. `launch::Spec` gained `env`, `permission_mode`, `resume` and
`hooks`, with a per-kind argument table (`launch::shape_args`) for claude, codex and cursor. `r`
on an archive row in the agents panel spends that: it asks `resume this run? y/n` on the
divider and, on `y`, opens the same agent resuming the session the run recorded
(`launch::resume_command`, `wire::AGENT_RESUME`). The session id is unchanged, so the daemon
adopts the same run back rather than starting a second one.

## The loop, end to end

`hocus_plugin_authoring::a_written_hook_is_bound_and_fired_by_a_reporting_agent_end_to_end`
scripts the whole thing in one session (~1.5 s): `zellij plugins write` a hook, `set-hook
--write` it to `agent_start`, `agent_waiting`, `pane_exit` and `agent_exit`, run a fake `claude`
under `zellij run --block-until-exit` that reports through `agent-report`, and read the three
headless firings off the log and busy → waiting → exited off `zellij events`. Loop it after any
hook change: `cargo x integration-test -- -j 16 -E 'test(a_written_hook)'`.

## Notes for the next session

**Proto numbers in use** (next free): plugin `CommandName` 280 (`GetLocalUtcOffset`) / payload
field 221 (next 281 / 222) - this note had gone stale once already, having missed
`DataCatalog` 278 / 219 and `SetQuitNotice` 279 / 220, so read the `.proto` rather than
trusting it; plugin `EventType` 57 (`Hook`), event payload field 51; IPC `Action`
oneof 159 (`set_pane_agent`) and 160 (`reconfigure`; `main` holds 155–158; next 161); IPC
`ClientToServerMsg` 28 (`subscribe_to_events`; `main`'s `host_terminal_focus_changed` is 27),
`ServerToClientMsg` 20 (`event_update`); `PermissionType` 21 (`AuthorPlugins`); `PaneInfo` field
30 (`agent`); `AgentInfo` field 8 (`run_id`, in *both* `plugin_api/event.proto` and
`client_server_contract/common_types.proto`); `PipeMessage` field 8 (`pane_id`); `AgentSource` 2
/ IPC `AGENT_SOURCE_REGISTERED` 3.

**Two things learned about plugin loading**, for anyone touching hooks:

- `WasmBridge::all_plugin_and_client_ids_for_plugin_location` reads `cached_plugin_map`, a copy
  warmed on first use and cleared only by `unload_plugin` and `apply_cached_events`. During a
  load it is stale; `loading_plugins` (`loading_plugin_ids_for_plugin_location`) is the
  authoritative in-flight list. With an *empty* plugin map the copy never warms, so a bare
  plugin-thread test does not see the staleness a session does - load another plugin first
  (`a_firing_during_the_headless_hooks_load_reaches_the_loading_instance`).
- `grant_permissions_and_log_actions_in_thread!` answers permission requests for plugin id 0.
  With more than one plugin loaded, write the fake screen thread by hand and answer with the
  request's own `plugin_id`.

**Test recipes.**

```sh
cargo x test -p zellij-utils -p zellij-server -p zellij -p hocus -p hocus-agentd -p hocus-proto \
  -E 'test(/agent/) | test(/events/) | test(/reconfigure/) | test(/set_hook/) | test(/overlay/) \
      | test(/plugin_authoring/) | test(/lua/) | test(/pipe/) | test(/hook/) \
      | test(/job/) | test(/chat/) | test(/plugin_/) | test(/shipped/)'
# nextest flags go after `--`
cargo x integration-test -- -j 16 -E 'binary(cli) | binary(/hocus_/) | binary(new_agent_modal) | binary(plugins)'
```

The whole integration suite exceeds ten minutes at `-j 16`; under a foreground limit run it in
three chunks by binary (`ls zellij-integration-tests/tests` is the list): `about_pane`…`mouse`,
`nested_fullscreen`…`resurrection`, `scroll`…`warp`, each as
`-E 'binary(/^(a|b|…)$/)'`.

**Known red, none of it this branch's** (CLAUDE.md keeps the list): `misc::quit_with_keybinding`,
the `nested_*` group, `stacked_pane_list::a_stack_list_survives_session_resurrection`.
`cli::send_command_through_the_cli` is a snapshot race under load. The box is shared
with sibling worktrees running their own gates: run a red test alone before believing it.

One race worth knowing about, fixed in its test: since `main`'s `9003d98e6` a tab's plugins hear
about mode changes only while a client is on that tab, and get a fresh `ModeUpdate` when one
lands - asynchronously, so a snapshot taken the moment the tab bar changes can still show the
landed-on tab's status bar in the mode the client *left* it in. `tabs::move_tab_to_right` now
waits for the Normal-mode ribbon too (`status_bar_appears()`); do the same in any test that
switches tabs and snapshots the status bar.

## Not done, by decision

- **Binary self-rebuild** - out of scope.
- **Landing `assembly-lines`** - parked; `AgentInfo` and `zellij events` give it the signal it lacked.
- **Streaming `RunCommand` stdout, per-pane `PaneRenderReport` subscriptions, plugin-source
  watching** - served well enough by `zellij subscribe`, `read_pane` and explicit reload.
- **Lua `load()` of generated code** - stays gone; write-to-disk + check + reload is the loop.
- The ~580 `sleep()` calls in older tests (CLAUDE.md, "Waiting in tests") remain a mechanical
  follow-up; the tests this workstream added use `Watched`.
