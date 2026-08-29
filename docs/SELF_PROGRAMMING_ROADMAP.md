# Toward a self-programmable zellij: architecture survey and roadmap

## Context

The goal of this fork is a terminal workspace that can program itself — where coding agents
(Claude Code first, codex/cursor alongside) are spawned, observed and steered by the system, and
where the system's own behaviour (plugins, hooks, keybindings, config) can be rewritten at runtime
by those agents or by the in-session model (`hocus`) without a rebuild.

This plan is the result of a survey of what exists, and a prioritized set of changes. Scope
decisions made with the user:

- **Self-modification means the Lua/config layer**, not rebuilding the Rust binary. A
  rebuild-and-reinstall loop is out of scope.
- **The `assembly-lines` branch stays parked.** Nothing here depends on it, and nothing here
  blocks landing it later; recommendation R3 is the event it would want when it does land.

## What exists today (the short version)

The fork is further along than its docs admit. Things that look missing but are present:

| Capability | Where |
|---|---|
| In-session LLM control plane with 65 tools, incl. `agent_spawn/send/control/list`, `plugin_write/reload/read/log/docs`, `schedule` | `default-plugins/hocus/src/tools.rs:556`, `tool_loop.rs` |
| Single agent-launch planner (kind → binary, `--model`, prompt, branch/worktree) | `default-plugins/hocus-proto/src/launch.rs:94` |
| Agent state daemon: 5 s scrollback fingerprinting + Claude Code hook feed (`zellij hocus hooks install`) | `default-plugins/hocus-agentd/src/main.rs`, `hocus-proto/src/agents.rs`, `src/hocus_hooks.rs`, `docs/AGENT_HOOKS.md` |
| Hooks: `agent_exit` (pane-replacing) and the headless points `agent_start`, `agent_waiting`, `pane_exit`, `session_start`, `context_menu` | `zellij-server/src/screen/mod.rs` (`fire_*_hook`), `plugins/mod.rs` (`run_hook`, `deliver_to_headless_hook`), `zellij-utils/src/input/hooks.rs` |
| Lua plugins reach the **whole** `PluginCommand` surface (~243 variants, 96 replying) via one `from_value` | `zellij-server/src/plugins/lua/command.rs`, `lua/response.rs` (TECH_DEBT B2 is done; the doc is stale) |
| SQLite plugin data store with watches, shared namespaces | `zellij-utils/src/plugin_data_store/`, `hocus-proto/src/store.rs` |
| Work queues (headless plugin jobs, per-queue concurrency) | `zellij-server/src/plugins/queues.rs` |
| Runtime reconfigure + config/theme/layout file watching | `PluginCommand::Reconfigure`, `zellij-utils/src/input/config.rs:487` |
| Lua config (`config.lua`, full stdlib, `hook`/`bind` builders) | `docs/LUA_CONFIG.md` |
| `git checkout` primitive for spawning in a branch/worktree | `PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin`, `zellij_exports.rs:2492` |
| CLI from inside a pane: 81 `zellij action`s (most `*ByPaneId`), `zellij run --block-until-exit*`, `zellij subscribe --pane-id --format json`, `zellij pipe`, `list-panes --json`, `zellij recordings` | `zellij-utils/src/cli.rs:258-334, 860+, 1150` |
| Hocus extensions: Lua plugins register model tools over `message_to_plugin` | `default-plugins/hocus/src/extensions.rs` |
| Detachable pane supervisors (panes outlive the server; warp between sessions) | `zellij-server/src/pty_host/`, `docs/ARCHITECTURE.md` |

## The gaps that matter

1. **"Agent" is not a server concept.** Two disjoint definitions: the server's
   `HOCUS_AGENT_PANE_CONTEXT` key (gates `agent_exit`; `data.rs:3726`) and hocus-proto's
   command-sniffing `classify()` (`agents.rs:223`; gates listing/control). An agent typed into a
   shell is controllable but gets no exit hook; a hooked pane may not be listed. Agent *state*
   (busy/waiting/idle) lives only inside `hocus-agentd`'s memory, reachable by no CLI, no event, no
   other plugin, and lost on reload.
2. **The hook feed is plugin-private and hand-threaded.** Claude Code's hooks pipe JSON to a
   plugin by name (`agent_hook`), carrying `pane_id=$ZELLIJ_PANE_ID` as a string arg because
   `PipeSource::Cli` has no pane identity (`data.rs:3461`, `wire.rs:93`). The pipe name and
   event→state mapping are duplicated across the binary and the plugin crate (`AGENT_HOOKS.md:97`).
3. **Observation from outside is polling.** `zellij subscribe` streams render bytes only; there is
   no CLI/IPC channel onto the plugin event bus (`PaneUpdate`, `CommandPaneExited`, `CwdChanged`…).
   `zellij run --block-until-exit` discards the exit code (`data.rs:40`).
4. **The authoring loop is blind and hocus-only.** No `ParseLuaPlugin`: a syntax error is only
   visible in the plugin's own pane after an async reload (`TECH_DEBT.md:463`). `plugin_write`
   exists only as a hocus model tool; the plugin dir is not a plugin fs root, so hocus shells out
   to write it (`tools.rs:2081`). Reload wipes Lua state with no handoff signal.
5. **Config self-write is KDL-only and refused under Lua config** (`lib.rs:2519`). There is no
   CLI for `Reconfigure`/`RebindKeys`/hook binding at all — binding a `hooks {}` entry
   programmatically requires being a wasm plugin.
6. **Hook points are one shape, two points.** *(closed by R14)* `RunHook` meant "replace this
   pane with a plugin"; `context_menu` was the one headless exception. No `agent_start`,
   `agent_waiting`, `session_start`.
7. **Permission gating fails open** in both maps: `command_permission`'s `_ => return None`
   (`zellij_exports.rs:7186`) and event delivery's `_ => Granted` (`wasm_bridge.rs:2601`). Any new
   command or event is ungated by default — the wrong default for a system about to hand plugins
   authoring authority.
8. **Nothing persists** in the orchestration layer. *(closed by R15 for jobs, latches and
   summaries; hook feeds are derived from the pane manifest since R4 and need no store.)*

---

## Recommendations, prioritized

Phases are ordered by dependency and payoff. Each item is independently landable.

### Phase 0 — Foundations (small, do first)

**R1. Make permission classification exhaustive.** *(done)*
Replace `_ => return None` at `zellij-server/src/plugins/zellij_exports.rs:7186` with explicit
arms for every permissionless command (`Subscribe`, `Unsubscribe`, `SetSelectable`, `ShowCursor`,
`GetPluginIds`, `GetZellijVersion`, `HideSelf`, `ShowSelf`, `SetTimeout`, `ReportPanic`,
`RequestPluginPermissions`, `PostMessageTo`, `PostMessageToPlugin`, `CloseSelf`, `FinishQueueJob`,
…), so the match is compiler-exhaustive. Same for the event map at `wasm_bridge.rs:2601`.
Add a test that iterates `CommandType::iter()` / `EventType::iter()` and asserts each resolves
(same shape as `every_reply_the_shim_reads_is_decoded`, `lua/unit_tests.rs:2293`).
*Why first:* every later recommendation adds commands/events; this makes forgetting a gate a
build error rather than a silent grant.

**R2. `PluginCommand::CheckLuaPlugin { name, source }`** *(done)* (the `ParseLuaPlugin` from
TECH_DEBT). Mirror `ParseLayout` (`data.rs:4387`, dispatch ~`zellij_exports.rs:227`). The Lua
loader already compiles the chunk (`plugins/lua/loader.rs`); return the compile error
synchronously. Resolve the path exactly as hooks do (`file:` bare name → plugin dir). Then:
- hocus: new `plugin_check name=` tool, and make `plugin_write` run the check **before** writing
  (refuse and return the error). Add the command to `REPLYING_COMMANDS` + `lua/response.rs`.
- Cost is the nine-file transcription in `TECH_DEBT.md:340` (data.rs, proto, prost regen via
  `cargo x proto`, `plugin_command.rs` ×2, `zellij_exports.rs`, `shim.rs`, response decoder).

**R3. Docs corrections** *(done)* (cheap, prevents misdirection): `docs/TECH_DEBT.md:8-9, 260-287` say
B2 is untouched — it is done. `docs/LUA_PLUGINS.md` "Current limits" is otherwise accurate. Move
the stray plan file `continue-vectorized-lynx.md` out of the repo root (to `docs/` as
`ASSEMBLY_LINES_DESIGN.md`, marked parked) or delete it.

### Phase 1 — Agent lifecycle as a server concept

**R4. One definition of "agent pane", carried on `PaneInfo`.** *(done — `AgentInfo { kind, state, source, detail, cwd, session_id }` on `PaneInfo.agent`, held by `TerminalPane`; set by the spawn path (`Spawned`), `PluginCommand::SetPaneAgent` and `zellij action agent-report` (`Reported`); `fire_agent_exit_hook` keys on it. hocus-agentd derives its feeds from the manifest instead of a private map. `hook_phase` and friends moved to `zellij-utils/src/agent.rs`, re-exported through `zellij_tile::agent`.)*
Add `agent: Option<AgentInfo { kind: String, state: AgentState, detail: Option<String>,
source: AgentSource }>` to `PaneInfo` (`data.rs:2565`) and to the server's `TerminalPane`.
`AgentState = { Busy, Waiting, Idle, Exited }` — the same three phases `HookFeed` already uses
(`agents.rs:397`), plus exit. Set by:
- the git-checkout spawn path (`zellij_exports.rs:2492`) and `launch::agent_pane_context()`
  (replace the `HOCUS_AGENT_PANE_CONTEXT` map key with the typed field; keep the key readable
  for one release);
- a new `PluginCommand::SetPaneAgent { pane_id, agent: Option<AgentInfo> }` (permission:
  `ChangeApplicationState`) — agentd uses it to publish its heuristic verdicts and latches, hocus
  `agent_register` uses it for manual registration;
- **R5** below for self-reporting agents.
`Screen::fire_agent_exit_hook` (`screen/mod.rs:2660`) keys on the field. `agentd` keeps its
fingerprinting, but writes results *out* through `SetPaneAgent` instead of holding them
privately; `agent_list`, the strip, and the summary box read from `PaneManifest`. Move the
pure state model (`HookFeed`, `hook_phase`, `status_cell` inputs) from `hocus-proto/src/agents.rs`
to `zellij-utils/src/agent.rs` so the binary and plugins share it — this also retires the
"spelled in two places" note in `AGENT_HOOKS.md:97`.

**R5. `zellij action agent-report` replaces the `agent_hook` pipe.** *(done — the installed script is v3; the daemon still accepts the old pipe and writes the same record.)*
New `CliAction::AgentReport { pane_id (default `$ZELLIJ_PANE_ID`, as `SetPaneColor` does at
`cli.rs:1780`), --stdin JSON | --state --detail --cwd }` → `Action::SetPaneAgent` → screen. The
generated `hocus-hook.sh` (`src/hocus_hooks.rs:96`) calls it instead of `zellij pipe`. The
event→state mapping runs in the binary using the shared `zellij-utils::agent` module. Keep the
`agent_hook` pipe accepted by agentd for one release (it just forwards to `SetPaneAgent`).
Also emit `Event::AgentUpdate(Vec<(PaneId, AgentInfo)>)` — or simply rely on `PaneUpdate` now
carrying the field; prefer the latter unless the manifest churn proves noisy.

**R6. `zellij run` returns the exit status.** *(done — it already did via `ExitReason::CustomExitStatus`; what landed is the fixes the trace found: a plain `--blocking` no longer returns 0 when an unrelated pane closes, the pane id now reaches a blocking caller, `pipe_client` honours the status, and the lossy `ProtoExitReason` decode errors instead of answering 0. `--print-exit-status` was dropped as redundant with the process exit code.)*
`UnblockCondition::is_met` (`data.rs:40`) consumes the code; carry it in
`ServerToClientMsg::Exit`/`UnblockCliPipeInput` to the blocking client and exit the CLI with it.
Add `--print-exit-status`. Small change; turns `zellij run --block-until-exit` into a usable
remote-exec primitive for an orchestrating agent.

**R7. `zellij events [--type …]`** *(done)* — an IPC channel onto the plugin event bus.
Sibling of `SubscribeToPaneRenders` (`ipc.rs:220`, `route.rs:2902`, client loop at
`cli_client.rs:262`). Add `ClientToServerMsg::SubscribeToEvents { types }`; in the plugin thread,
where `PluginInstruction::Update` fans out to plugins, also serialize (`Event` is serde-`Serialize`;
`EventType` is the strum discriminant, `data.rs:966`) and push to registered CLI subscribers via
`ServerToClientMsg`. Gate to the "state" events (`PaneUpdate`, `TabUpdate`, `SessionUpdate`,
`CommandPaneOpened/Exited`, `CwdChanged`, `CommandChanged`, `AgentUpdate`, `FileSystem*`,
`ModeUpdate`); never `Key`/`PaneRenderReport` (those have `subscribe`). With R4/R5 this is the
line an agent in a pane runs to wait for a sibling agent to go `waiting` or `exited`.

### Phase 2 — The self-programming loop

**R8. `/plugins` as a fifth plugin fs root + `AuthorPlugins` permission.** *(done — root and WASI preopen; writes under it need `AuthorPlugins`. hocus writes through it since R15c.)*
Add the user plugin dir (`get_user_plugin_dir`) to the preopens / `resolve_fs_path_with_permission`
roots (`zellij_exports.rs:5139-5225`) as `/plugins`. Reading it needs `ReadFiles`; writing it needs
a new `PermissionType::AuthorPlugins` (proto `plugin_permission.proto`, `data.rs:1172` list),
which also covers R9's headless reload. Hocus's `plugin_write` then uses `FsWrite` instead of a
shell (`tools.rs:2081`), dropping the 64 KB `MAX_ARG_STRLEN` cap (`tools.rs:2118`); keep its
symlink guard and path normalization by moving them into the server-side root resolution.

**R9. Reload without a pane, and a state handoff.** *(done — `PluginCommand::ReloadPluginByUrl`, and every reload now sends `BeforeClose` to the instance it replaces.)*
`PluginCommand::ReloadPluginByUrl(url)` reloads every instance of a location without opening or
focusing a pane (`WasmBridge::reload_plugin`, `wasm_bridge.rs:1003`, already does the work;
`StartOrReloadPlugin` is what adds the pane). Fire `Event::BeforeClose` on reload if it is not
already (check `wasm_bridge.rs:624`), documented as the moment to persist to `zellij.data`.
Hocus `plugin_reload` for headless plugins/extensions/hooks uses this.

**R10. `zellij plugins write|check|reload|log|dir <name>` CLI.** *(done — `plugins`, since `zellij plugin <url>` already launches one. The name rules live in `zellij_utils::plugin_authoring`; `reload` is `start-or-reload-plugin` on the resolved file.)*
Lift `plugin_write`/`plugin_reload`/`plugin_log` out of `hocus/src/tools.rs:2230-2400` into
`src/commands.rs`: `write` (stdin or `--file`, same guards, runs R2's check first), `check`
(offline: the binary links `zellij-server`, so `lua/loader.rs` is reachable), `reload` (R9 via
`Action`), `log` (grep `[lua:<name>.lua]` in `zellij.log`). This makes authoring a plugin
something a Claude Code pane can do directly, and hocus's tools become thin wrappers over the
same code.

**R11. `zellij action reconfigure` and `zellij action set-hook`.** *(done — one `Action::Reconfigure`, applied to every attached client; `set-hook` is sugar over it; a fragment is validated client-side. `RebindKeys` is not exposed: a `keybinds { }` fragment does it.)*
Expose `ServerInstruction::Reconfigure` (`lib.rs:133,1648`) as `CliAction::Reconfigure { --kdl |
--file, --write }` and add `CliAction::SetHook { point, url | "" , --write }` that patches only the
`hooks {}` block (`hooks_from_kdl`/`hooks_to_kdl`, `kdl/mod.rs:5867, 5998`). `HookPoint::ALL`
drives validation. Also expose `RebindKeys`. With R10, the full loop — write hook → check →
bind → trigger — needs no wasm plugin and no hocus.

**R12. A config overlay so programmatic writes always persist.** *(done — `config.d/*.kdl` layered in name order by both loaders, watched; `--write` and a plugin's `reconfigure(_, true)` merge their fragment into `config.d/zellij-managed.kdl` by top-level node, children one level deep, validated before the atomic write. `RebindKeys` writes still take the whole-file path.)*
`update_new_saved_config` (`lib.rs:2494-2570`) refuses to rewrite `config.lua`. Add
`<config>/config.d/*.kdl`, loaded after `config.kdl`/`config.lua` (same merge as the existing
KDL merge), watched by `watch_config_file_changes` (`config.rs:487`). All programmatic writes
(`Reconfigure(_, true)`, `RebindKeys{write_config_to_disk}`, R11 `--write`) target
`config.d/zellij-managed.kdl` — one answer regardless of config format, and a human's Lua stays
untouched.

**R13. Pane identity on `PipeSource::Cli`.** *(done — as `PipeMessage.pane_id`, leaving `PipeSource::Cli(String)` intact for third-party plugins; agentd reads it first and the `pane_id=` argument second.)*
`PipeSource::Cli(String)` → `Cli { pipe_id, pane_id: Option<PaneId> }` (`data.rs:3461`), filled
by the client from `ZELLIJ_PANE_ID` in `zellij pipe`. Removes the `--args pane_id=` hand-thread
from every CLI→plugin integration (`wire.rs:93`). Note honestly in docs: this is convenience and
consistency, not authentication — the env var is as trustworthy as the process that set it.

### Phase 3 — Generalize hooks, persist the orchestrator

**R14. A headless hook shape and new hook points.** *(done — `HookPoint::shape()` is `ReplacePane` for `agent_exit` and `Headless` for everything else; `RunHook.pane_id` is optional; `deliver_to_headless_hook` in `plugins/mod.rs` is the find-or-load-then-`update_plugins` path both `context_menu` and `Event::Hook { point, payload }` use. `agent_start`/`agent_waiting` fire off `agent_hook_edges` in `screen/mod.rs` (SetPaneAgent, pane creation, re-run); `pane_exit` from `HoldPane` and the pty's `ClosePane`; `session_start` from the plugin thread's first `AddClient` — not thread start, because `add_client` re-instantiates every plugin loaded before it. Payloads and the one known gap (Enter in a held pane with no `agent_exit` bound) are in `docs/LUA_PLUGINS.md`.)*
`HookPoint` (`hooks.rs:35`) gains `AgentStart`, `AgentWaiting`, `SessionStart`, `PaneExit`
(any command pane). `PluginInstruction::RunHook` gains a `shape`: `ReplacePane` (today's
`agent_exit`) or `Headless` (the `context_menu` load path at `plugins/mod.rs:1627`, but delivering
`Event::Hook { point, payload }` so one long-lived plugin receives many firings instead of being
re-loaded with the payload as config). Config surface is already generic over `HookPoint::ALL`
(`kdl/mod.rs:6005`; `config.lua` `hook` builder). `agent_start` is where "spawn the next agent
on branch X when Y starts" and "auto-answer this permission prompt" policies live, in user Lua.

**R15. Persist hocus jobs, latches and agent state in `shared:hocus`.** *(done — `job`, `agent_latch` and `agent_summary` entities in `store::shared_defs()`, row code in `hocus-proto/src/jobs.rs` and `agents::rows`; written as each change lands (`persist_job` on arm and fire, `forget_jobs`, `persist_latch`/`forget_latches`, `persist_summaries` diff-only) and read back once in `State::restore_persisted` from `open_store`. Every restored job lands in the foreground chat. `job_auto_confirm: bool` became `scheduled_run: Option<ScheduledRun { job_id, auto_confirm }>`, and `pump_tools` answers a confirmation-gated call on a scheduled run without `auto_confirm` with a decline instead of parking.)*
`Job` (`message.rs:159`), `manual_kinds`/latches, and summaries go through `hocus-proto/src/store.rs`
defs (constructor-only, `Backend` trait, `Local` for tests) so a plugin reload or session restart
restores them. R4 already moves live agent state to the pane manifest; this covers what is
hocus-owned. Fix the `transitions.rs:775` stall at the same time: scheduled runs auto-decline
`confirm` tools rather than parking every job on a y/n.

**R15b. hocus `agent_register` writes the record.** *(done — `agents::registered_agent` builds an `AgentSource::Registered` record (new variant, `idle` placeholder state) that `agent_register` writes through `set_pane_agent`; `manual_kinds` and its latch row stay, for the daemon's classifier.)*

**R15c. hocus `plugin_write` on `fs_write`.** *(done — `zellij-tile` gained `fs_read`/`fs_list`/`fs_metadata`/`fs_write`/`fs_remove`/`fs_create_dir`/`fs_rename` (and `data::FsEntry`); hocus's `plugin_write`, `plugin_read`, `plugin_list`, `plugin_reload` and its startup extension scan go through them against `/plugins` and answer synchronously. The shell script, the 64 KB argv cap and the hand-rolled symlink guard are gone; the host's root resolution is the containment. `plugin_log` still shells out - the log file is outside every plugin root.)*

**R16. Richer launch specs.** *(done — `Spec` gains `env`, `permission_mode`, `resume` and `hooks`; `launch::shape_args` is the per-kind table (claude `--permission-mode`/`--dangerously-skip-permissions`, `--resume`/`--continue`; codex `--ask-for-approval`/`--full-auto`/`--dangerously-bypass-approvals-and-sandbox`, `resume` as a leading subcommand; cursor `--force`, `--resume`/`resume`), an unknown mode being an error that names the accepted ones. `env` is an `env(1)` wrapper in the command, and `zellij_utils::agent::agent_kind_of` looks through it so the pane's record is named for the agent. A `claude` spawn is handed the hook wiring inline as `--settings`, built by `zellij_utils::agent::claude_hook_settings` — the same list `zellij hocus hooks install` writes — so a spawned pane reports its state with nothing installed.)*
`launch::Spec` (`launch.rs:46`) gains `env`, `permission_mode`, `resume`/`session_id`, and a
per-kind arg shaper (today only `--model` is understood; everything else is raw `args`). Keep
`plan()` pure and host-tested. With R5, a spawned Claude Code pane's hooks are installed
per-project automatically (`zellij hocus hooks install --project` is already idempotent).

### Explicitly not recommended now

- **Binary self-rebuild** — out of scope by decision.
- **Landing `assembly-lines`** — parked by decision; R4/R7 give it the agent signal it lacked.
- **Streaming `RunCommand` stdout, per-pane `PaneRenderReport` subscriptions, plugin-source
  watching** — real gaps, but each is served well enough by `zellij subscribe`, `read_pane`, and
  explicit reload; revisit after Phase 2.
- **Lua `load()` of generated code** — stays gone (`lua/CLAUDE.md`); write-to-disk + check +
  reload is the loop, and R2/R9/R10 make it fast.

## Status

Phase 0 (R1–R3) landed with this document, then Phase 1 (R4–R7), Phase 2 (R8–R13) and
Phase 3 (R14–R16). `zellij events` keeps its subscriber
list on the plugin thread (the one place every event passes), forwards each state report once
per batch however many plugins it was addressed to, and has the screen report pane/tab state to
a `CLI_EVENTS_ONLY_TARGET` when no plugin wants it, so it works in a plugin-less layout too. R1 found two real inconsistencies while making the
map explicit — `FocusPreviousPane` and `FocusLastPane` were ungated while `FocusNextPane` needed
`ChangeApplicationState` — and two deliberate compatibility holes it left open and labelled
(`ScanHostFolder`, `WatchFilesystem`, and the `FileSystem*` events; see the comments in
`zellij_exports::command_permission`). Deciding those is part of R8.

## Suggested order

R1 → R2 → R3 → R4 → R5 → R6 → R7 → R8 → R10 → R9 → R11 → R12 → R13 → R14 → R15 → R16.
Phase 0 and R6 are each under a day; R4+R5 is the one design-heavy item and should land as
its own branch with the `AGENT_HOOKS.md` rewrite.

## Verification

Per item, the narrow recipes in `CLAUDE.md` (`cargo x test -p zellij-server -E 'test(...)'`,
`cargo x test -p zellij-utils`); `cargo x proto` + `cargo x assets` whenever `data.rs`/proto
change; `cargo x ci check` before each handoff; `cargo x integration-test -- -j 16` for R4–R7
and R11–R12 (they cross screen/plugin/client).

End-to-end scenario to script as an integration test once Phase 2 lands (uses
`write_git_repository`/`write_git_worktree` from `zellij-integration-tests/src/test_env.rs`):

1. From a pane, `zellij plugin write my-agent-start < hook.lua` → `zellij plugin check` passes.
2. `zellij action set-hook agent_start file:my-agent-start.lua --write` → `config.d` updated,
   `PluginInstruction::Reconfigure` observed.
3. `zellij run --block-until-exit -- claude ...` (a shell script standing in) with the hook
   script calling `zellij action agent-report`.
4. `zellij events --type AgentUpdate --json` in another pane shows busy → waiting → exited;
   `zellij run` exits with the child's status.
5. `agent_exit` hook fires; `plugin_log` shows the hook's output.

## Handoff for Phase 3

Everything above R14 is on `self-programming-vision`, committed. What the next session needs to
know without re-deriving it:

**Where things are now.**

| Piece | Entry points |
|---|---|
| Agent record | `zellij-utils/src/data.rs` (`AgentInfo`, `AgentState`, `AgentSource`, `PaneInfo::agent`), `zellij-utils/src/agent.rs` (hook reading), `zellij-server/src/panes/terminal_pane.rs` (`agent`, `agent_from_run`), `screen/thread_main.rs` (`SetPaneAgent`), `screen/mod.rs` (`fire_agent_exit_hook`) |
| Reporting it | `zellij action agent-report` (`cli.rs` `AgentReport` → `actions.rs`), `PluginCommand::SetPaneAgent`, hook script v3 in `src/hocus_hooks.rs`; agentd derives feeds from the manifest (`hocus-agentd/src/main.rs` `rebuild_panes`, `HookFeed::from_agent`) |
| Event stream | `zellij events` (`cli_client.rs` `start_events_client`), `WasmBridge::forward_events_to_cli_subscribers`, `CLI_EVENTS_ONLY_TARGET` in `plugins/mod.rs`, `EventType::CLI_STREAMABLE` in `data.rs` |
| Config | `Action::Reconfigure` → `ServerInstruction::ReconfigureAllClients` (`lib.rs`), `ConfigPersistence` + `update_new_saved_config`, `Config::with_overlay_dir` / `write_managed_fragment` / `merge_kdl_fragment` (`input/config.rs`), `home::get_config_overlay_dir` |
| Plugin authoring | `zellij plugins …` (`src/commands.rs` `plugins`, `plugin_authoring` module), `zellij_utils::plugin_authoring::resolve_plugin_file`, `zellij_server::check_lua_plugin`, `/plugins` root + `AuthorPlugins` (`zellij_exports.rs` `fs_roots`, `resolve_fs_path_with_permission`), `ReloadPluginByUrl` |
| Pipes | `PipeMessage.pane_id` (`data.rs`, `pipe_message.proto` field 8), set in `plugins/mod.rs` `CliPipe` arm |

**Proto numbers in use** (next free): plugin `CommandName` 274 / payload field 215 (next 275 /
216); IPC `Action` oneof 156 (next 157); IPC `ClientToServerMsg` 27, `ServerToClientMsg` 20;
`PermissionType` 21; `PaneInfo` field 30; `PipeMessage` field 8.

**Test recipes that cover the new surface.**

```sh
cargo x test -p zellij-utils -p zellij-server -p zellij -p hocus -p hocus-agentd \
  -E 'test(/agent/) | test(/events/) | test(/reconfigure/) | test(/set_hook/) | test(/overlay/) \
      | test(/plugin_authoring/) | test(/lua/) | test(/pipe/) | test(/hook/)'
cargo x integration-test -E 'binary(cli) | binary(hocus_plugin_authoring) | binary(hocus_agent_hooks)'
```

Known environment noise: the 19 `zellij-client web_client_tests` time out on this machine at
the moment **on `main` as well** (verified in a control worktree); they passed earlier in the
day. The integration harness cannot end a session (`quit()` runs into the quit-confirmation
modal and times out twice, ~20s per test), so no integration test can assert "the stream ended
with the session". The box is shared: check for sibling `cargo x test` runs before believing a
timeout.

**Phase 3 landed** (one commit per item on `self-programming-vision`): R14, R15, R15b, R15c,
R16. What the next session should know:

- Hooks: `HookPoint::shape()` decides pane-replacing vs headless; `deliver_to_headless_hook`
  in `plugins/mod.rs` is the one find-or-load path. `session_start` fires on the plugin
  thread's first `AddClient`, not at thread start — `WasmBridge::add_client` re-instantiates
  every plugin loaded before the first client, and would lose an event sent to the first
  instance. A second client attaching while a hook plugin is still loading wipes that plugin's
  cached events (`add_client` resets `cached_events_for_pending_plugins`); pre-existing, and
  worth fixing if a lazily-loaded hook ever misses its first firing under multi-client use.
- Persistence: `job`, `agent_latch`, `agent_summary` are in `store::shared_defs()`; both
  hocus and hocus-store declare them. Restored jobs all land in the foreground chat; the row
  keeps `chat_id` for a future that reattaches them to a reopened chat.
- Proto numbers now in use: plugin `EventType` 57 (`Hook`), event payload field 51,
  `AgentSource` 2 / IPC `AGENT_SOURCE_REGISTERED` 3.
- Left open by design: `plugin_log` still shells out (the log file is outside every plugin
  root); Enter in a held pane with no `agent_exit` bound restarts the command without an
  `agent_start`; a per-project `hooks install` is unnecessary for spawned panes and still
  writes `.claude/settings.json` (committed) for hand-started ones.

**What's next** is the end-to-end scenario under "Verification" as an integration test, and
the items under "Explicitly not recommended now" as they come due.
