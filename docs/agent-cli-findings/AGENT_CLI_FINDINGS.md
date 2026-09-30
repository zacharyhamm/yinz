# Agent CLI live shakedown — findings

Session `considerate-aardvark`, `~/.cargo/bin/yinz` 0.1.0 built from main `61cf86cdc`.
Agents: Claude Code 2.1.284 (`--model haiku`), Codex 0.158→0.159 (auto-updated mid-run, `--model gpt-6-luna`).
Scratch repo: a throwaway Git repo holding a Python calc module with one failing test. The run was on 2026-09-29.
Pane IDs such as `terminal_4` refer to panes in that session. All of them were closed at teardown.

## Evidence

Everything lives under [`agent-cli-findings/`](agent-cli-findings/):

| Path | What it is | Where it came from |
|---|---|---|
| `logs/NN-name.json` | The full `--json` response of one `yinz agent …` command. "log NN" in the tables below refers to these files. | Captured by the test driver, which piped each command's stdout to a file. |
| `logs/v2-NN-name.{json,err}` | The same capture for the [re-verification run](#re-verification-run-2026-09-29-second-pass). | Captured by the second test driver. |
| `logs/v3-NN-name.json` | The live check of the #6 fix. | Captured from `yinz agent … --json` in session `rusty-peach`. |
| `logs/v4-09-*.json`, `logs/v4-10-*.json` | The live startup-dialog and permission-answer checks for #9/#10. | Captured from this worktree's `target/debug/yinz` in session `verify-agent-dialogs`. |
| `logs/v5-11-*.json` | The live `/clear` check for #11. | Captured from this worktree's `target/debug/yinz` in session `verify-agent-clear-fixed`. |
| `logs/NN-name.err` | stderr for the same command, kept only when non-empty (for example, client-side validation errors). | Same as above. |
| `transcripts/claude-haiku-orchestrator.jsonl` | The complete Claude Code session log of the **haiku orchestrator** (pane `terminal_9`, conversation `9d5cb9af-…`). It shows every `yinz` command it ran and the result it got, including the `--session main` copy, the Codex spawn, the raw-input fallbacks, and the cross-session messages. The first user message shows the `<pasted_content>` wrapper from finding #3. | `~/.claude/projects/<scratch-repo>/9d5cb9af-3a31-4870-934e-4c6dbf4cfe29.jsonl` |
| `transcripts/codex-luna-orchestrator.jsonl` | The complete Codex rollout of the **luna orchestrator** (pane `terminal_13`, thread `01a0edf8-…`). It shows the successful spawn→wait→prompt→needs_input→send-keys sequence and its final report. | `~/.codex/sessions/2026/09/29/rollout-2026-09-29T11-21-47-01a0edf8-….jsonl` |
| `codex/title-thread-rebind.csv` | Codex daemon log rows from 11:02 to 11:04 for pane `terminal_4`'s real conversation (`01a0ede6-…`) and for the hidden title-generation thread (`01a0ede8-4a80-…`) that stole the pane binding (finding #2). | Queried from `~/.codex/logs_2.sqlite`, truncated to 600 characters per row |
| `codex/binding-and-hooks.md` | Manually captured evidence for findings #2, #7, and #8. It covers the `binding.json` contents before and after the rebind, the `outcome=unbound` Yinz log line, hook replay experiments, Codex's `/hooks` view, the environment inside Codex tool commands, and two Codex log rows that Codex pruned before archiving. | Terminal output recorded during the run |

Reading the transcripts: each JSONL line is one event. For Claude, use
`jq -r 'select(.type=="assistant")|.message.content[]?|select(.type=="tool_use")|.input.command'`
to list the commands it ran. For Codex, look for `"type":"custom_tool_call"` entries; their outputs are the matching `custom_tool_call_output` entries.

Not included: the tester's running notes, the Claude and Codex *worker* sessions, and `yinz.log`. That log is shared with
test runs, and its hook trace lines have no timestamps.

Severity: **P0** = documented flow cannot work / no CLI escape; **P1** = wrong outcome or stuck state with a workaround;
**P2** = misleading output, docs/skill gaps; **P3** = polish.

## Re-verification run (2026-09-29, second pass)

Session `outstanding-xylophone`, `~/.cargo/bin/yinz` release-built at 17:07 from `fd0023a98` plus this branch's working
tree (it contains `PASTE_LEAD_IN` and `recover --interrupted`). Claude Code 2.1.285 `--model haiku`, Codex 0.159.0
`--model gpt-6-luna`. Scratch repo: the same calc module, in the tester's scratchpad. Evidence is `logs/v2-NN-name.json`
(and `.err`); "v2 NN" below refers to those files. Session and Codex rollouts are named by conversation ID where cited.

### Status of every finding

| # | Status | Evidence and notes |
|---|---|---|
| 1 | **Fixed** | Codex spawn → chained `wait --until idle`: `ok`/`ready:true` in 0.27–0.33s (v2 64 after trust, 78, 80). |
| 2 | **Fixed** | Consecutive `prompt --wait` calls completed with turn IDs (v2 69, 70, 92, 93). `binding.json` kept the real thread. |
| 3 | **Fixed** (residual #37) | 5-line, 1304-char, `--file task.md`, 55 KB, `#`-prefixed and multi-line `/probe` prompts all `completed`, and the model did the task (v2 08, 09, 10, 17, 50, 114). The archive and pane title store the unwrapped prompt. |
| 4 | **Fixed** | Chained spawn→wait: Claude `ok` in 0.6–0.9s (v2 49-*), Codex 0.33s (v2 78). It still fails when stray input arrives first (#9, #36). |
| 5 | **Fixed** | After Esc: the no-flag `recover` names `--interrupted` (v2 37). `--interrupted` inside 3s is refused (v2 38) and after 3s returns `Idle/ready` (v2 39). The next prompt completes, including identical text (v2 40, 41). |
| 6 | **Fixed**, verified live (v3 01–07) | Before the fix: Codex Esc during `sleep 25` went Idle, then **Busy** again about 24s later while the composer was idle (v2 71, 72), and `--interrupted` was refused (v2 73). Only a second Esc, 3s, then `--interrupted` escaped it (v2 74–76), and that Esc arms Codex's "esc again to edit previous message". Busy or waiting hooks carrying the interrupted turn's `turn_id` are now dropped. Live in session `rusty-peach` with Codex `on-request`: one Esc during `sleep 25` (v3 04). Codex logged `turn_aborted` at 18:20:12, then the sleep's `item_completed` for the same `turn_id` and a hook at 18:20:35. Status stayed `Idle` for 51s (v3 05). A plain `recover` (no `--interrupted`) returned `ok` and `ready:true`, and the next `prompt --wait` completed "AFTER" (v3 06, 07). |
| 7 | **Fixed on `agent-cli-verify-more`**, verified live | Before: `env \| grep ^YINZ` inside a spawned Codex → `NONE` (v2 70). Now a sandboxed `on-request` Codex printed `YINZ_PANE_ID`, `YINZ_PANE_REF`, `YINZ_SESSION_NAME` and `YINZ_SOCKET_DIR`, and `agent list` without `--session` and `action context` (caller = its own pane) worked (session `cc-verify78`). |
| 8 | **Fixed on `agent-cli-verify-more`** (opt-in), verified live | Before: `cannot connect to session …: Operation not permitted`, and "There is no active session!" (v2 81). With `spawn --allow-yinz` the sandboxed Codex reached the session. Without it, both commands now explain that a sandbox blocked the connection and name `--allow-yinz`. |
| 9 | **Fixed on `agent-cli-verify-more`, verified live** (see [fix status](#fix-status-dialog-input-9-and-10)) | Before the fix, Codex trust dialog: `send-keys Enter` → `status_unavailable` "cannot verify a live agent process" (v2 65). After a raw `action write`, the wait ended `Idle`/`ready:false` until `recover` (v2 67, 68). Claude's trust dialog, answered by a person, did the same (v2 02–06). |
| 10 | **Fixed on `agent-cli-verify-more`, verified live** (see [fix status](#fix-status-dialog-input-9-and-10)) | Before the fix, `send-keys Enter` on a permission prompt, then `wait --until idle`: a stale `needs_input` in 10 ms (v2 110). After the turn: `timed_out` with state `Idle` and no reason (v2 112). |
| 11 | **Fixed**, verified live | `/clear` still returns `target_changed`. The clear-start hook restores the new conversation as `Idle/Reported`, `ready:true`; list/status and an idle wait succeed, and the next prompt completes without recovery. See [fix status](#fix-status-claude-clear-11) and v5 11 logs. |
| 12 | **Still failing live; planned settling and correlation implemented** | The 500 ms minimum plus 100 ms output quiet period still leaves a Unicode draft in Claude 2.1.285. A separate two-second paste-to-Enter probe also leaves the draft. Preserved/normalized fingerprints pass unit tests, but timing alone does not fix submission. See [verification](#fix-status-agent-cli-12-14). |
| 13 | **Fixed, verified live** | `/probe`, `/cost`, and Claude `!printf` return `rejected` before writing or claiming input, with `ready:true` and `send-keys` guidance. Exact Claude `/clear` still replaces the conversation, and the next prompt completes. See [verification](#fix-status-agent-cli-12-14). |
| 14 | **Fixed, verified live** | A Stop hook that blocks once keeps the pane Busy and `ready:false`; a concurrent prompt is rejected. The original wait returns only `FINAL_1214` after the matching transcript `turn_duration`. See [verification](#fix-status-agent-cli-12-14). |
| 15 | **Fixed with refreshed hooks, verified live** | Refreshed v10 scripts defer to inline hooks. Live prompt completed with one report per source event. Existing installs must be refreshed. See [verification](#fix-status-agent-cli-15-33-and-36). |
| 16 | **Fixed for Claude shorthand** | Shared launch validation rejects `--kind codex --model haiku` before spawning. Other model IDs remain the provider's responsibility. See [fix status](#fix-status-agent-cli-1618). |
| 17 | **Existing reconnect fix, regression extended** | The launcher survives backend connection failures before and after conversation selection. A live Codex auto-update has not been retested. See [fix status](#fix-status-agent-cli-1618). |
| 18 | **Fixed** | Initial lookups distinguish `not_agent`, `target_not_found`, and `target_exited`. Conversation pins and transitions during pending operations retain their existing outcomes. See [fix status](#fix-status-agent-cli-1618). |
| 19 | **Fixed; automated verification** | Readiness now names its blocker in JSON, text and prompt rejection messages. See [fix status](#fix-status-agent-cli-1921-and-32). Original evidence: v2 04, 14. |
| 20 | **Fixed; automated verification** | Diagnostics distinguish uncertain acceptance from an accepted turn and make recovery conditional. See [fix status](#fix-status-agent-cli-1921-and-32). Original evidence: v2 79, 94. |
| 21 | **Fixed; automated verification** | Every spawn option has help. Kind errors list all recognized kinds and the JSON command alternative. See [fix status](#fix-status-agent-cli-1921-and-32). Original evidence: v2 102, 103. |
| 22 | **Fixed; help verified** | Help documents one key per argument, quoted modifiers, named keys, unsupported aliases, and all four wait states. Parsing is unchanged. See [fix status](#fix-status-agent-cli-2224). |
| 23 | **Fixed; bundled skills verified** | Bundled skills use the current session or explicit discovery, a provider placeholder, and a labeled Claude Haiku example. See [fix status](#fix-status-agent-cli-2224). |
| 24 | **Fixed; automated verification** | Reads count nonblank lines and preserve internal spacing. Help and recovery instructions explain ghost suggestions and require deliberate empty-composer confirmation. See [fix status](#fix-status-agent-cli-2224). |
| 25 | **Still broken** | Pane 5 running a shell-launched Codex: `action context` → `detected: codex`, but `agent list` omits it. |
| 26 | **Still broken, wider** | `register` needs `6` and rejects `terminal_6` (v2 45b, 47b). `agent status 7` is rejected (v2 101). `action context --pane-id terminal_5` is rejected, too. |
| 27 | **Still broken** | `agent runs --json` → "unexpected argument" (v2 11). |
| 28 | **Still broken** | A second `--branch fix-sub` silently returns `fix-sub-2` with no `requested_branch` field (v2 106-*). `.worktrees/` is untracked. See #34. |
| 29 | **Still broken** | `--agent` conflicts with `--dest`. `prompt TEXT --file` prints `Usage: … <TARGET> <TEXT>`. |
| 30 | **Still broken** | `list-sessions` still lists `pty-broker-v8.sock` as UNRESPONSIVE. |
| P3 | **Residual issues remain; focus fixed** | The spawn focus issue (#36) is fixed. Pane titles are still the full command line with the hooks JSON. The `ui-*` leak remains: 13 directories stayed after six Codex panes and the tab were closed. |

### New findings

| # | Sev | Finding | Evidence |
|---|---|---|---|
| 31 | P1 | **Claude background work ends the turn early and then strands the pane.** When Claude backgrounds a Bash command or launches a background subagent, `prompt --wait` returns `completed` with the interim text ("The command is running in the background…", "Agent launched. Waiting for it to complete."). When the work finishes, Claude starts its own turn with a `<task-notification>` `UserPromptSubmit`. Yinz treats it as foreign input, so the pane ends `ready:false` until `recover`, and the real answer ("DONE", "SUB=sub") never reaches the caller. While that turn runs, status can read `Idle`/`ready:true`. The archive stores the notification as a `prompt`. | v2 26, 35 (sleep), 119, 120 (subagent); runs `claude:c74f207a-…`, `claude:7308c863-…` |
| 32 | P1 | **Fixed; automated verification.** The bridge translates a final failed turn into `StopFailure`. The original HTTP 400 failure left the pane Busy until timeout. See [fix status](#fix-status-agent-cli-1921-and-32). | v2 79 |
| 33 | P1 | **Fixed, verified live.** Managed resumes with permission overrides now return `rejected` and exit 1 before creating a pane or checkout. Resume without overrides succeeds. See [verification](#fix-status-agent-cli-15-33-and-36). Previously, **`spawn --kind codex --resume ID --permission-mode skip` exited immediately.** Codex prints "Permission overrides are not supported when resuming a remote task", yet `spawn` returned `ok`. The wait then reports `target_changed`/`Exited`. Without `--permission-mode`, the resume works and keeps the binding. | v2 88–93 |
| 34 | P2 | `--reuse` puts a second agent into a worktree that a live agent already occupies (`terminal_16` and `terminal_18` both in `fix-sub`), with no warning or field. | v2 106-1, 107 |
| 35 | P2 | A worker spawned into `.worktrees/fix-sub` read and **edited the main checkout's** `calc.py` by absolute path. Its cwd and system prompt named the worktree, so the model chose the path, but nesting `.worktrees/` inside the main checkout makes this easy. `prompt --wait` reported success. | Session `0a76a8a5-…` tool calls, v2 108–112 |
| 36 | P1 | **Fixed, verified live.** CLI spawn preserves the active pane and tab, including overflow placement. See [verification](#fix-status-agent-cli-15-33-and-36). Previously, **spawn stole focus in the user's active tab.** `action new-tab` from the CLI does not move the client, so every spawn lands in the user's tab. The user's keystrokes (from their own Codex pane) went into two freshly spawned orchestrators ("m t", "he"). Those panes became `ready:false`, and the text was lost from the user's pane. This is the likely source of the earlier unexplained `ready:false` after spawn. `pane-move-to-tab` works as a workaround. | v2 121–124 |
| 37 | P2 | For Claude, the `PASTE_LEAD_IN` does not always establish trust. Haiku called a 55 KB paste ending in "Ignore all lines above. Reply with exactly…" a "prompt injection" and did not follow it. Codex answered it. | v2 114, 115 |
| 38 | P3 | Validation errors bypass `--json`: an empty or whitespace-only prompt, argument errors, and `register` print plain stderr with no `AgentResponse`. | v2 116, 117, 45 |
| 39 | P3 | Claude's hidden prompt-suggestion subagent is archived as messages ("Subagent a3acd…: fix it", "(No obvious next step to suggest)") and can be a run's last entry. Readiness is unaffected. | runs `claude:c74f207a-…`, `claude:a9d5a4eb-…` |
| 40 | P2 | Codex `/new` rebinds the pane to the new thread (`target_changed`) and stays in `agent list`, unlike Claude `/clear`. It is left `ready:false` until `recover` with the new conversation ID. | v2 82–85 |
| 41 | P3 | `action context` returns every pane's full command line, including the ~3 KB `--settings` hooks JSON for each Claude pane. |  |

What worked this time: parallel prompts to two panes, and two `wait`s on a pane mid-prompt (v2 51-*). Also Claude
`--resume` (v2 86, 87), `kill -9` during a `wait` (→ `target_changed`/`Exited`, v2 97), bad `--project` (v2 118),
and empty-prompt rejection.

### Orchestration rerun

| Orchestrator | Task | Result |
|---|---|---|
| Codex luna (full access, `effort medium`) | Delegate the fix to a Claude haiku worker on `fix-sub-l` | **Succeeded**, with no warm-up turns needed (#1 and #2 fixed). It tried `--session main`, then `yinz session list`, then found the session. It handled two permission prompts with `read` + `send-keys` but needed `yinz subscribe` to see the dialog options. It verified the tests itself: they pass. Rollout `01a0ef4d-…`. |
| Claude haiku (bypass permissions) | The same task on `fix-sub-h` | **Failed** (the tests still fail). It copied `--session main` and `--kind codex --model haiku` from the skill, stalled on Codex's trust dialog (#9, `send-keys` refused), then respawned Claude with `--reuse`. It never answered Claude's manual-mode permission prompts with `send-keys Enter` and tried `pipe` and `run` instead. It interrupted and recovered repeatedly, then gave up and closed the worker. Session `a9d5a4eb-…`. |

Skill takeaways that are still open: drop `--session main` and the `--kind codex` example, and say how to find the
session. Say to pick `--permission-mode` at spawn. Show the `needs_input` → `read` → `send-keys Enter`/`Down Enter` →
`recover` loop, with the key syntax. Warn about the trust dialog on a new directory, and say what to do when a worker
never becomes ready.

## Fix status: agent CLI #15, #33, and #36

Implemented and checked on 2026-09-30 in `agent-cli-verification-continued`.
The planner sets `YINZ_CLAUDE_INLINE_HOOKS=1` only when it supplies inline Claude hooks.
It sets `0` for Claude launches without inline hooks, including Firecracker, to override inherited markers.
Version 10 installed scripts skip reporting when the marker is `1`.
Inline hooks use the remote broker when `YINZ_REMOTE_PANE_TOKEN` is nonempty.
Third-party hooks and Stop-completion handling remain unchanged.

**Migration:** rerun `yinz hocus hooks install --agent claude` on each host with existing installations.
Refresh project installations with `--project` or `--project-dir PATH` as applicable.
Old scripts continue to duplicate inline reports until refreshed. This change does not rewrite user settings automatically.

Managed Codex launches reject nonblank `resume` plus nonblank `permission_mode` before checkout or VM preparation.
The error tells callers to omit `--permission-mode` or start a new conversation with that mode.
`hooks:false` retains the native launch path. CLI worker placement uses the existing no-focus target.

### Automated evidence

- 115 focused launch, hook, server, and placement tests passed.
- Generated v10 scripts and inline commands were executed against a recording reporter for every Claude hook event.
  Each event produced exactly one report, with unchanged input, for local and remote routes.
  Tests cover marker clearing, disabled hooks, Firecracker, all recognized Codex modes, IDs, `last`, and normalization.
- Server rejection checks observed no pty request, branch, or worktree for local branch, worktree, and VM launch requests.
- All four placement integration tests passed. The new CLI test uses an unattached connection and the attached-client fallback.
  It checks the destination tab and sends keyboard input after placement in the current, existing, and new overflow tabs.
- `cargo x remote-test --build --native` succeeded. `cargo x remote-test --test` passed all 43 SSH tests.
- `cargo x ci check` passed format, lint, 9,419 unit tests, doctests, and all 475 integration tests.
  The first pass identified 12 caller assertions that expected Claude without its new environment wrapper. Those expectations were updated.
- `cargo x firecracker-test --build` succeeded. `cargo x firecracker-test --test` passed all four real-VM tests and removed its temporary network.
  KVM is available to the escalated runner, although it is hidden from the sandbox.
- [Automated command results](logs/v7-automated.txt) record the final runs. All builds used this worktree's own `target/`.

### Live evidence

The isolated session was `verify-153336`, using this worktree's own native binary and Wasm plugins.
Agents were Claude Code 2.1.286 (Haiku) and Codex 0.159.2 (`gpt-6-luna`).
Claude used an isolated configuration with the v10 script emitted by `hooks install --dry-run` and installed hook entries.
The user's installed hooks were not changed. The scratch session and copied authentication state were removed after verification.

- **#15:** [Claude's prompt](logs/v7-15-prompt.json) completed with `VERIFIED_153336` and `ready:true`.
  The [event stream](logs/v7-15-events.jsonl) contains one SessionStart, UserPromptSubmit, and MessageDisplay for the foreground turn.
  The two Stop records are the original Busy report and the existing synthetic Idle completion with `_yinz_claude_turn_complete:true`.
  They are not duplicate hook deliveries. The separate SSH and Firecracker suites also passed.
- **#33:** Both [ID](logs/v7-33-rejected-id.json) and [last](logs/v7-33-rejected-last.json) resumes with `skip` returned `rejected`.
  The [ID](logs/v7-33-rejected-id-evidence.json) and [last](logs/v7-33-rejected-last-evidence.json) checks record exit 1, unchanged panes, and no branch or worktree.
  Resume without overrides [kept the conversation ID and became ready](logs/v7-33-resume-idle.json).
  The [next prompt](logs/v7-33-resume-prompt.json) completed with `RESUMED_153336`.
- **#36:** [Focus evidence](logs/v7-36-focus.json) records direct terminal input after a current-tab spawn and an overflow spawn.
  The original pane retained focus, and tab 0 remained active while the overflow worker occupied tab 1.

## P0 — the documented flows break

| # | Finding | Repro | Suspected cause / fix |
|---|---|---|---|
| 1 | **Codex never becomes ready after spawn.** Codex fires `SessionStart` only when the first turn starts. A freshly spawned Codex stays `Busy/Spawned/status_unavailable`, so `wait --until idle` → `status_unavailable` and `prompt` is impossible. The documented spawn→wait→prompt flow cannot work for Codex. | `agent spawn --kind codex …` then `agent wait T --until idle --timeout 2m` (logs 07, 08) | **Fixed on `agent-cli-findings-second-pass`.** See [Fix status](#fix-status-codex-1-and-2). The bridge reports a `SessionStart` when the UI selects a conversation. |
| 2 | **Codex loses its conversation binding after the first turn.** On the first prompt, Codex TUI starts a hidden `temporary-structured-*` thread ("Generate a concise, single-line task title…"). The proxy rebinds the pane to that thread on its `thread/start` response and `turn/start`. Hooks for the real conversation (Stop) are then `outcome=unbound`, and the pane stays **Busy** forever. The next `turn/start` on the real thread rebinds it. | Spawn Codex, type one prompt, see `binding.json` `thread_id` ≠ the reported `session_id` (`/tmp/yinz-1000/codex/ui-*/binding.json`) | **Fixed on `agent-cli-findings-second-pass`.** See [Fix status](#fix-status-codex-1-and-2). The bridge ignores threads that Codex marks `ephemeral`. |
| 3 | **Claude: prompts of five or more lines, or about 1000+ characters, always end as `interrupted`, `acknowledged:false`.** Claude collapses the paste into `[Pasted text #N +k lines]`, and `UserPromptSubmit.prompt` becomes `\n\n<pasted_content id="…">…</pasted_content id="…">\n`. The fingerprint does not match (`control.rs:218-223`), so Yinz treats it as conflicting input. Claude **did** run the turn, but the caller loses the outcome and the pane is left not ready. This hits every realistic `--file task.md`. Claude also frames `<pasted_content>` as text the user may not have written. The archive stores the wrapper as the prompt. | log 44 (3 lines: OK) and logs 45/46/47/48 (5 lines or 1008 characters: interrupted in 0.13s); orchestrator log 42 | **Fixed on `agent-cli-verify-more`.** `prompt_sha256` (`yinz-types/src/agent/control.rs`) unwraps each `<pasted_content id="X">…</pasted_content id="X">` element before hashing, so the submitted prompt and the hooked or pruned digest agree. The run archive and the pane title record the unwrapped prompt. Live, Claude then acknowledged long prompts but refused them ("pasted content … but no actual request from you"), because it follows `<pasted_content>` instructions only when the user's own words ask. For a long or multi-line Claude prompt, Yinz now types `Follow the instructions in the pasted text: ` ahead of the paste (`PASTE_LEAD_IN`). The fingerprint, the archive and the title strip the lead-in. |
| 4 | **A `wait` started before SessionStart always returns `target_changed`.** The pending wait records `conversation: None`, and the first `SessionStart` looks like a conversation change (`agent_control.rs:816-823`). This is step 2 of the documented flow. Afterwards, `ready` lags `Idle` by a variable time, from 2s to over 30s. | `spawn` then an immediate `wait --until idle` (logs 02, 33): `target_changed` after 0.6 to 6s | **Fixed on `agent-findings-codex-fixes-test`.** The wait adopts the first conversation, and a repeated SessionStart for that same conversation, such as #15's duplicate Claude hook, retargeted it. A SessionStart for the tracked conversation now retargets only after a turn is accepted, for every kind. |
| 5 | **Claude Esc-interrupt deadlocks the pane.** Claude sends no `Stop` when interrupted, so the state stays `Busy` forever. `recover` requires Idle (rejected), `prompt` requires ready (rejected), and `wait` times out. The agent CLI has no way out, and only a human typing a new prompt clears it. | log 37, then recover, wait and prompt (all fail) | **Fixed on `agent-cli-verify-more`.** `agent recover … --confirm-empty --interrupted` also accepts a Busy or Waiting agent once it has not reported for 3s (`RECOVER_QUIET`). It resets the record to Idle, then clears claims as usual. Without the flag, the rejection names `--interrupted`. The same escape applies to Codex #6. |
| 6 | **Codex Esc-interrupt ends Busy too.** The `Interrupt` hook sets Idle for about 8s, then a late hook, probably `PostToolUse` from the killed command, flips the pane back to **Busy** forever. The screen shows an idle composer. The deadlock is the same as #5. | log 30, then status polls | **Fixed on `agent-cli-verify-more`.** The screen's agent lifecycle records the `turn_id` of an `Interrupt` or `StopFailure` until the next `UserPromptSubmit`. `apply_queued_agent_report` (`yinz-screen/src/screen/thread_main.rs`) consumes a busy or waiting hook for that turn without applying or broadcasting it. `Stop` does not end a turn here, because a blocking Stop hook continues it. Hooks without a `turn_id`, which includes Claude's, are unaffected. |
| 7 | **Codex shell commands have no Yinz context.** Codex runs tools in its shared app-server daemon, whose environment lacks `YINZ_SESSION_NAME`, `YINZ_PANE_ID`, and `YINZ_PANE_REF`. `yinz agent list` fails with "no session selected; pass --session". `action context` cannot identify the caller, so a Codex orchestrator cannot know its own pane and could target itself. | Sandboxed and full-access luna panes, with `env \| grep YINZ` empty | **Fixed on `agent-cli-verify-more`.** The bridge (`yinz-codex/src/lib.rs`, `inject_pane_env`) adds the pane's routing environment to `params.config` as `shell_environment_policy.set.*` on every non-ephemeral `thread/start`, `thread/resume` and `thread/fork`, merging into a nested `shell_environment_policy.set` when the UI sent one. Codex applies it to the thread's tool commands. |
| 8 | **Default-sandboxed Codex cannot reach Yinz at all.** Under `workspace-write`, the socket connect fails with "Operation not permitted (os error 1)", and `action context` misleadingly prints "There is no active session!". Every skill is unusable from a default Codex. | `spawn --kind codex --permission-mode on-request`, then ask it to run `yinz --session S agent list` | **Fixed on `agent-cli-verify-more`.** Codex's seccomp filter blocks `connect()` unless the sandbox has network access. `writable_roots` and a permission-profile `unix_sockets` allowlist do not help (checked with `codex sandbox`). `agent spawn --kind codex --allow-yinz` adds `-c sandbox_workspace_write.network_access=true`, which also allows the internet, so it is opt-in. A refused connect is now `SessionProbe::Denied`: `list-sessions` shows `UNREACHABLE` instead of `UNRESPONSIVE`, and `action`/`agent` commands explain the sandbox and name `--allow-yinz`. |

## Fix status: Codex #1 and #2

Both were reconfirmed live on 2026-09-29 with Codex 0.159.0. The test used a fresh scratch repo that was trusted in
`~/.codex/config.toml` before the test, so the trust prompt was ruled out. Every Yinz hook was also trusted.

- **#1 reconfirmed:** The screen showed an idle composer and no dialog. The binding was written at spawn, but
  `agent status` stayed `Busy/Spawned`, `status_available:false`, for 30s. Nothing reaches the pane until the first turn,
  and readiness requires a `Reported` source and lifecycle evidence (`yinz-screen/src/screen/agent_control.rs`, `agent_status`).
- **#2 reconfirmed:** After one typed prompt, `binding.json` moved to `01a0ee1d-6b28-…`, the "Generate a concise,
  single-line task title…" thread. It has no rollout on disk, which fits `ephemeral`. Status kept the real conversation
  and stayed Busy after Codex answered. Codex 0.159's app-server protocol exposes `ThreadStartParams.ephemeral` and
  `Thread.ephemeral`.

Fix, all on this branch:

- `yinz-codex/src/lib.rs`: `Conversations` records ephemeral threads, flagged on either the request or the reply. They
  never take the binding, including through `turn/start` or `turn/steer`, so their hooks go unbound and are dropped.
  When the UI selects a non-ephemeral conversation through `thread/start`, `resume`, or `fork`, the bridge forwards a synthetic
  `SessionStart` (`source` `startup`/`resume`/`fork`, plus `cwd`). It uses the path `report()` uses, now the shared `forward()`,
  and runs off the relay loop.
- `yinz-types/src/agent/control.rs` and `yinz-screen/src/screen/agent_control.rs`: Codex's own deferred `SessionStart` for the
  same conversation no longer turns an unacknowledged prompt or a pending wait into `target_changed`. A different
  conversation, or a SessionStart after the turn was accepted, still retargets. Claude is unchanged.

Live verification used the worktree build in a separate session. `spawn` → `wait --until idle` returned `ok` and
`ready:true` in 0.56s. Two `prompt --wait` calls returned `completed` ("OK", "TWO"). The title thread
`01a0ee24-fbbf-…` ran and did not move the binding.

**Revalidated live** on 2026-09-29 in session `sparkling-hill`. The run used `~/.cargo/bin/yinz`, built after `c9e7c9ba1`,
with Codex 0.159.0 and a trusted scratch repo:

- **#1:** `spawn`, then `wait --until idle` returned `ok`, `Idle/Reported`, and `ready:true`. With the wait chained right
  after `spawn` (the strict #4 case), a second Codex pane returned `ok`/`ready:true` in 0.36s.
- **#2:** Two `prompt --wait` calls returned `completed` in 2.2s and 1.3s with messages "OK" and "TWO". The pane stayed
  Idle and `ready:true`. `binding.json` kept the real thread `01a0ee68-ad76-…` after both turns. The title thread
  `01a0ee68-efc5-…` ran (it appears in `logs_2.sqlite` with the task-title request) and has no rollout.
- **Claude regression check:** A Claude pane with no stray input became ready within 5s and completed a prompt. For
  Claude, a `wait` chained right after `spawn` still returns `target_changed` (#4, about 9s). In one run, that Claude pane
  then stayed `Idle` but `ready:false` until `recover`. Input reached the pane before Claude's SessionStart, so
  `Lifecycle.input_revision` stayed `None` (`agent_control.rs`, SessionStart branch). The probable source is the spawned
  pane receiving focus or keystrokes in the user's tab. This logic predates the branch. `recover --confirm-empty` fixed it.

**#4 root cause and fix.** A wait issued before SessionStart adopts the first reported conversation in
`refresh_agent_controls_at`. It stays pending while `ready` is false, because Claude reports SessionStart before its
composer enables bracketed paste (the live reply showed `prompt_supported:false`). With `hocus hooks install` in
`~/.claude/settings.json` plus spawn's `--settings` hooks, Claude sends a second SessionStart for the same conversation
(#15), and `agent_control_event` treated it as `target_changed`. The exemption that covered Codex's deferred start now
applies to every kind, in both `agent_control_event` and `PromptObservation::observe_event`. Another conversation,
SessionEnd, an exit, or a SessionStart after an accepted turn still retargets. The #15 duplication is addressed by the later [hook-routing fix](#fix-status-agent-cli-15-33-and-36).

Earlier note, before that fix: #4 can still affect a `wait` issued before the first SessionStart, though it did not reproduce here. A person
who types into a Codex pane within milliseconds of UI startup could see the synthetic SessionStart land after their turn
began, which marks the pane Idle briefly. `agent prompt` can't hit this, because it requires readiness first.

## P1 — wrong outcomes / stuck states with workarounds

| # | Finding | Evidence |
|---|---|---|
| 9 | **The agent CLI cannot answer startup dialogs.** Examples are Claude's Bypass-Permissions acceptance (`--permission-mode skip`) and trust prompts. `send-keys` and `wait` refuse with `status_unavailable` "cannot verify a live agent process" until SessionStart arrives, which only happens after the dialog. You must use raw `action write`, which bumps the input revision, so `ready` never arrives until `recover`. The skill forbids raw input, so a compliant orchestrator is stuck. | terminal_9; **fixed on `agent-cli-verify-more`**, see [fix status](#fix-status-dialog-input-9-and-10) |
| 10 | **Every `send-keys` leaves the agent not ready once the turn ends**, for example after approving a permission (`needs_input` → `send-keys Enter`). Each approval therefore needs a `recover`. `wait --until idle` right after `send-keys` returns `needs_input` immediately (10 ms), because the Waiting state is stale. Once the turn finishes, `wait --until idle` returns **`timed_out` while `state` is Idle**, with no explanation. The luna orchestrator hit this. | logs 05, 06; orchestrator 2 report; **fixed on `agent-cli-verify-more`**, see [fix status](#fix-status-dialog-input-9-and-10) |
| 11 | **`/clear` makes the agent vanish.** `SessionEnd` clears the record, and `SessionStart` with `source:"clear"` is not mapped (`hooks.rs:841` accepts only startup, resume, and fork). The pane leaves `agent list`, and `read`, `send-keys`, and `recover` all return "target does not contain an agent". | log 29; **Fixed on `agent-cli-verify-more`**, see [fix status](#fix-status-claude-clear-11). |
| 12 | **Prompts containing Unicode format characters (U+200B, U+202E) are never submitted.** Yinz's Enter, sent 100 ms after the paste, is swallowed and the draft stays in the composer, giving `timed_out`. Claude also strips U+200B, so the fingerprint would not match anyway. Emoji, accents, Arabic, CRLF, and stdin prompts work. | logs 16, 21–26 |
| 13 | **Non-prompt input times out after the full `--timeout` and leaves a claim.** Examples: `!cmd` (Claude bash mode, which runs a model turn but never acknowledges), `/cost` (opens a modal), and other slash commands. Nothing rejects these up front. | logs 27, 28 |
| 14 | **`completed` and `ready` arrive while Claude is still "running Stop hooks… 0/2 · thinking".** The first Stop report wins, even though other Stop hooks can still continue the turn. | logs 44, 53 |
| 15 | **Fixed with refreshed hooks**, see [verification](#fix-status-agent-cli-15-33-and-36). Previously, **every Claude hook fired twice** when the user had `yinz hocus hooks install --agent claude`, because spawn also injects `--settings` hooks. Correlation tolerated it here, but it doubles report traffic and is a race hazard. | `~/.claude/settings.json`, and spawn `pane_command` |
| 16 | **Codex model names are not validated.** `--kind codex --model haiku` spawns a Codex that shows "haiku medium" and will fail later. | orchestrator 1, terminal_10 |
| 17 | **A spawned pane's state was `Exited`/`Spawned` while Codex was running.** Codex auto-updated from 0.158 to 0.159 and restarted at launch, so the first wait returned `target_changed`. | orchestrator 1 log |

## Fix status: dialog input #9 and #10

Implemented in `yinz-screen/src/screen/agent_control.rs`, with no new commands, flags, or dependencies.
`agent send-keys` can reach a live Claude or Codex command pane whose agent record is still `Spawned`.
Conversation pins, process matching, held/exited panes, and disabled-input checks still apply.

Successful CLI key batches during startup or Waiting are tracked together. Matching lifecycle evidence consumes
only their input revision: the first verified SessionStart for startup, or parent progress/completion for a
permission answer. Until then, the old Waiting report cannot finish a wait with `needs_input`; a fresh matching
Waiting hook can. No Busy or Idle report is synthesized. Accepted prompt correlation survives these keys once
paste delivery has finished. Unaccepted prompts, failed key batches, manual/raw input, and newer drafts are not
automatically accounted for. Recovery and lifecycle invalidation clear the tracking.

**Automated regression coverage:** the focused screen agent-control suite covers both agents' startup and permission
answers, separate navigation and Enter batches, fresh permission requests, completion-reply readiness, manual input
before/after/between batches, invalid and partially failed batches, conversation replacement (including same-conversation restart), cleanup, in-flight
paste delivery, and unrelated worker progress. Existing held-pane, conversation-pin, and Esc-interruption tests
also pass. These are screen tests with fake hooks; the live checks below use the actual agent CLIs.

**Live verification, 2026-09-29:** separate session `verify-agent-dialogs`, using this worktree's `target/debug/yinz`,
Claude Code 2.1.285 (`haiku`) and Codex 0.159.0 (`gpt-6-luna`). Both used fresh throwaway Git repositories under
`/tmp/yinz-agent-dialog-check/`. All test panes were closed at teardown.

- **#9, Claude:** the real trust dialog showed `Busy/Spawned` and no lifecycle status. Separate `send-keys Down` and
  `send-keys Enter` calls succeeded. The immediately chained idle wait returned `ok`, `Idle/Reported`, `ready:true`.
  Evidence: [dialog](logs/v4-09-claude-dialog.json), [Down](logs/v4-09-claude-down.json),
  [Enter](logs/v4-09-claude-enter.json), [wait](logs/v4-09-claude-wait.json).
- **#9, Codex:** the real trust dialog also showed `Busy/Spawned`. `send-keys Enter` succeeded, and the chained idle
  wait returned `ok` and `ready:true`. Evidence: [dialog](logs/v4-09-codex-dialog.json),
  [Enter](logs/v4-09-codex-enter.json), [wait](logs/v4-09-codex-wait.json).
- **#10, Claude:** a scratch-file write produced `needs_input` with `acknowledged:true`. After `send-keys Enter`, the
  immediately chained wait returned `ok` and `ready:true`; the file contained `approved`. The next prompt completed
  `CLAUDE_NEXT_OK` with `ready:true`. No recovery was used. Evidence: [permission request](logs/v4-10-claude-prompt.json),
  [Enter](logs/v4-10-claude-enter.json), [wait](logs/v4-10-claude-wait.json), [next prompt](logs/v4-10-claude-next.json).
- **#10, Codex:** this permission probe used another pane with the per-process `approvals_reviewer="user"` override,
  since the user's default automatic reviewer approved the harmless command without a dialog. An explicitly escalated
  `/usr/bin/true` produced `needs_input` with `acknowledged:true`. After navigation-only `Down Up`, a 300ms idle wait
  remained pending until its deliberate timeout, still showing Waiting. A separate Enter batch followed by an idle
  wait returned `ok` and `ready:true`; the next prompt completed `CODEX_NEXT_OK` with `ready:true`. No recovery was used.
  Evidence: [permission request](logs/v4-10-codex-prompt.json), [navigation](logs/v4-10-codex-navigation.json),
  [pending wait](logs/v4-10-codex-navigation-wait.json), [Enter](logs/v4-10-codex-enter.json),
  [wait](logs/v4-10-codex-wait.json), [next prompt](logs/v4-10-codex-next.json).

Human/raw dialog answers still require recovery when their input cannot be correlated. Keys sent while Idle or Busy
keep their previous behavior, including Esc interruption. This change accounts only for `agent send-keys` during
startup or Waiting.

**Validation:** `cargo x test -p yinz-screen -E 'test(/^screen::screen_tests::agent_control::/)'` passed all 60 tests.
The final `cargo x ci check` passed format, lint, all 9,311 workspace tests, doctests, and all 473 integration tests,
using this worktree's own `target/`. The first gate encountered a plugin-host permission FIFO callback assertion;
that test passed alone and in both subsequent full gates. The final gate includes the same-conversation restart guard.

## Fix status: Claude `/clear` #11

`SessionStart` with `source:"clear"` now restores an `Idle/Reported` agent record. Compact and unknown sources
keep their existing behavior. Exact `/clear` submissions through `agent prompt` track their input revision only
after successful paste delivery and Enter. The matching parent SessionEnd and subsequent clear-start hook carry
that revision into the new conversation. The pane must still contain a live matching agent command, and the new
conversation must be nonempty and different. The original submission returns `target_changed` because it replaced
the conversation; it does not report a completed prompt.

Only the submitted revision is consumed. Newer drafts remain unconsumed, and duplicate hooks preserve readiness,
including an older installed hook that reports no state before the updated inline hook. Failed delivery, timeout,
recovery, pane closure, process replacement, and unrelated session transitions discard tracking. Manual/raw
`/clear` restores the agent record but still requires explicit recovery to account for its input.

**Automated validation:** all 27 focused `yinz-types` hook tests and all 64 focused screen agent-control tests pass.
The new screen regressions cover the clear-to-next-prompt flow for local, SSH, and VM pane targets; newer manual
input before/after the session transition; duplicate hooks and mixed reporter versions; failed paste/Enter;
unsubmitted or non-exact commands; recovery; and unrelated lifecycle changes. These tests use fake hooks.
The final `cargo x ci check` passed format, lint, all 9,315 workspace tests, doctests, and all 473 integration
tests with 16 workers, using this worktree's real `target/`. The first gate found another stale clear-state
expectation in `hocus-proto` (updated) and a portable CLI timeout. Both passed focused reruns; the final full gate
passed without failures.

**Live verification, 2026-09-29:** Claude Code 2.1.285 (`haiku`) in separate session `verify-agent-clear-fixed`,
using this worktree's `target/debug/yinz` for the server and inline hooks, with a throwaway Git repository at
`/tmp/yinz-agent-clear-check/project`. The installed older hook and updated inline hook both fired. The initial
conversation `88202d7c-bf80-4818-b469-fbe5e3143578` was ready. CLI `/clear` returned `target_changed`, and
`7fdced25-dc38-4057-93c7-e942b6a041ab` appeared as `Idle/Reported`, `ready:true` in both status and list.
The idle wait returned `ok`, and the next prompt completed `CLAUDE_CLEAR_NEXT_OK` with `ready:true`. No recovery
was used between `/clear` and the next prompt. All verification panes were closed at teardown.
Evidence: [before](logs/v5-11-before.json), [clear](logs/v5-11-clear.json), [status](logs/v5-11-after.json),
[list](logs/v5-11-list.json), [idle wait](logs/v5-11-wait.json), [next prompt](logs/v5-11-next.json),
[terminal output](logs/v5-11-next-read.json).

## Fix status: agent CLI #12–14

Verified on 2026-09-29 with Claude Code 2.1.285 and Haiku, in isolated scratch sessions
`verify-agent-12-14` and `verify-agent-12-14-fresh`. The fresh session used a 180-column terminal.
No existing user panes or drafts were used.

**#12 remains unresolved in live Claude.** The requested settling guard now waits at least 500 ms after
paste delivery and 100 ms without pane output, then sends Enter once. Deadline, process, conversation,
and input ownership checks remain active. Claude acknowledgement accepts the original fingerprint or
one with U+200B/U+202E removed. The PTY receives the original text; Codex stays exact, and emoji joiners
remain significant. Automated tests cover both hook spellings, long prompts, delayed output, and
manual input during settling.

Those checks pass, but the live combined Unicode prompt, U+200B alone, and a long collapsed Unicode
paste still timed out with the draft in the composer. A deliberate raw paste followed by one Enter
**two seconds later** also left the draft. This contradicts the earlier inference that waiting alone
would solve #12. Fixing it needs a changed delivery constraint; the proposed choices are normalizing
only these two characters before Claude paste or allowing another guarded Enter. Neither change has
been applied without the user's choice. Evidence: [combined Unicode](logs/v6-12-unicode.json),
[U+200B](logs/v6-12-zwsp.json), [long Unicode](logs/v6-12-long-unicode.json),
[two-second probe](logs/v6-12-slow-paste.log).

**#13 is fixed and verified live.** The screen thread rejects slash commands for both agents and `!`
shell input for Claude before claiming or writing input. It checks leading whitespace and Claude's
U+200B/U+202E normalization. Only exact Claude `/clear` is exempt. Live `/probe`, `/cost`, and
`!printf BANG_OK` returned `rejected` with `ready:true` and guidance to use `send-keys`.
`/clear` returned `target_changed`; once its clear-start hook arrived, idle waiting succeeded and the
next prompt completed `5` with readiness restored. An immediate query between SessionEnd and
SessionStart still sees no agent record; that existing lifecycle gap is outside findings #12–14.
Evidence: [probe](logs/v6-13-probe.json), [cost](logs/v6-13-cost.json),
[shell input](logs/v6-13-bang.json), [clear](logs/v6-13-clear.json),
[idle wait](logs/v6-13-after-clear.json), [next prompt](logs/v6-13-next-prompt.json).

**#14 is fixed and verified live.** Ordinary parent Claude Stop reports, including Idle reports from
older installed hooks, now mean Busy. A detached observer on the agent's host follows the submitted
`prompt_id` through complete transcript records and confirms only a matching `turn_duration`, using
the last assistant text on that chain. Its hook output descriptors are closed. Local confirmations
use the existing IPC reporter; remote confirmations use the pane broker. Duplicate and stale
confirmations cannot change state or finish another prompt. Missing evidence returns
`status_unavailable` and retains the submission claim. Initial transcript creation has the existing
five-second hook budget; later loss, replacement, process exit, or the 24-hour limit ends observation.
Host-local conversation markers also stop observers when `/clear` or resume leaves an old transcript
file intact.

A project [Stop hook](logs/v6-14-stop.py) paused for six seconds and blocked once after `BLOCKME_1214`.
During that hook, [status](logs/v6-14-blocking-status.json) was Busy with `ready:false`, and a
[concurrent prompt](logs/v6-14-concurrent.json) was rejected. The original wait stayed pending and
returned [only `FINAL_1214`](logs/v6-14-final.json) with `ready:true`.
The [transcript records](logs/v6-14-transcript.jsonl) show the final assistant message and its duration
boundary. Ordinary prompting also verified that the observer handles a transcript created after the
submission hook returns.

**Automated validation:** `cargo x ci check` passed format, lint, all 9,347 workspace tests, doctests,
and all 473 integration tests with 16 workers. The worktree built its own native and Wasm artifacts.
The focused regressions cover command rejection without writes or claims, `/clear`, Unicode
fingerprints, one Enter after settling, input interruption, blocking Stop continuation, duplicate
reporters, old prompt IDs, partial records, initial creation, transcript loss, process identity, and
conversation replacement. `cargo x remote-test --build --native` built this worktree's Linux binary
and SSH fixtures; `cargo x remote-test --test` passed all 43 tests, including a real remote reporter
that stays Busy through an ordinary Stop and a partial duration record, then confirms only the final
answer. See the [validation record](logs/v6-validation.txt).

## Fix status: agent CLI #16–18

**#16:** The shared launch planner rejects Claude aliases (`haiku`, `sonnet`, `opus`, and `opusplan`)
for Codex, including case variants and the `[1m]` suffix. The error directs callers to use
`--kind claude` or a Codex model ID. Validation runs before pane creation and worktree setup.
These aliases are reserved even with a custom provider. Other model IDs and raw arguments remain the configured provider's responsibility.
API failure reporting is addressed separately in [#32](#fix-status-agent-cli-1921-and-32).

**#17:** Commit `18684fcf6` already keeps the Codex UI alive when its backend connection fails.
The launcher regression now also rejects the first connection, before any conversation binding exists.
It then checks abrupt backend exit, another failed connection, successful reconnection, and preserved binding.
The test uses a real launcher subprocess with a mock Codex backend. It does not install or test a live Codex update.

**#18:** An initial lookup without `--conversation` returns `target_not_found` for an absent pane
and `not_agent` for a pane without an agent. A held pane whose agent exited returns `target_exited`,
even after `SessionEnd` cleared its agent record. A retained exited record also returns `target_exited`
from `status`, with its record included. These outcomes have specific messages and nonzero CLI exit codes.
Conversation pins still return `target_changed` or `target_closed` when their target disappears.
Pending operations retain their transition outcomes. No agent record is synthesized.

**Validation, 2026-09-30:** All 106 focused regressions passed. The final `cargo x ci check`
passed format, lint, all 9,379 workspace tests, doctests, and all 473 integration tests with 16 workers.
The gate includes the additional held-before-first-launch check and IPC round trips for the new outcomes.
Builds and tests used this worktree's own native and Wasm artifacts.
The status regressions cover local, SSH, and VM pane targets through the screen's fake OS backend.

## Fix status: agent CLI #19–21 and #32

**#19:** `AgentStatus.not_ready_reason` is additive and defaults to `None` when
an older response omits it. The shared status builder selects `prompt_unsupported`,
`not_idle`, `pending_submission`, then `unconsumed_input`, in that order.
Ready agents have no reason. List, status and prompt rejections expose the cause.
Responses also account for claims while the screen temporarily removes pending operations.

**#20:** Unacknowledged timeouts describe uncertain acceptance. Acknowledged timeouts
describe an accepted turn whose completion was not observed, without implying cancellation.
Failures and interruptions retain the provider message. Recovery advice requires inspection
of the same live conversation with unresolved input. Closed, exited and replaced targets
receive no recovery advice. Existing outcome codes and command rejection, including `/exit`, remain.

**#21:** Spawn help describes every option, JSON file/stdin input, flag overrides,
checkout defaults, provider settings, resume, SSH and Firecracker.
Help and errors use the full recognized-kind list. Errors explain the JSON `command`
alternative. Case normalization, executable aliases and JSON-only configuration remain supported.
Validation still runs before checkout creation or pane spawning.

**#32:** The Codex bridge converts `turn/completed` with `turn.status == "failed"`
into `StopFailure`, carrying conversation ID, turn ID and provider error through the
existing local or remote reporter. It observes only the selected conversation's current turn.
Hidden helpers, unrelated threads, stale turns and duplicate terminal events cannot fail it.
Transient error notifications and backend disconnects do not signal final failure.
Backend traffic passes through unchanged.
See the [protocol's final turn events](https://learn.chatgpt.com/docs/app-server#turn-events).

**Validation, 2026-09-30:** All 281 focused tests passed. A subsequent serial run passed
five checks covering the real Codex reporter, failure/interruption messages, disappeared and
exited targets, the server's spawn response, and the Git-plugin test described below.
The final `cargo x ci check` passed format, lint, 9,389 workspace tests (54 skipped),
doctests, and all 473 integration tests with 16 workers. Builds and tests used this
worktree's own native and Wasm artifacts. The compiled `agent spawn --help` was also inspected.

The first gate exposed a server test that expected the old spawn error text. Its expectation
was updated. A Git-plugin test timed out in that run. No competing Cargo runs were found.
`git_plugins_pin_imports_workers_and_attached_clients_until_reload` passed alone in 0.557 seconds
and passed again in the final gate. The first gate's integration phase also passed all 473 tests.

Automated coverage uses mock Codex traffic and mock reporter subprocesses to check local/remote
routing, unchanged traffic, retryable errors, stale failures and duplicate terminal events.
A real `yinz hocus codex-report` subprocess test verifies failure state, conversation, turn ID
and provider text through IPC. Screen tests use the fake OS backend for local, SSH and VM targets.
They verify one reply before timeout, a released failure claim, a subsequent prompt, and
continued rejection when newer manual input remains. No live provider API failure has been
induced for this change. These checks do not constitute live-service, SSH or VM verification.

## Fix status: agent CLI #22–24

**#22:** `send-keys --help` documents one key per argument, quoted modifiers such
as `"Ctrl u"`, and named keys such as `Esc`, `Enter`, and `Down`. It explicitly
identifies `Ctrl+u`, `ctrl-u`, `C-u`, `Escape`, and `Return` as unsupported aliases.
`wait --until` help lists `busy`, `waiting`, `idle`, and `exited`. Parsing is unchanged.

**#23:** The bundled agent and session skills no longer hard-code `--session main`.
They explain current-session selection through `YINZ_SESSION_NAME`. Outside a
session, callers discover sessions and explicitly select the intended name.
The generic launch sequence uses `--kind KIND` and requires the requested provider.
A labeled Claude example uses `--kind claude --model haiku`. Callers must continue
to use actual pane and conversation IDs from responses.

**#24:** The shared screen handler selects the last N nonblank terminal lines.
Whitespace-only rows count as blank. It removes blank rows at both ends and keeps
blank rows between selected content. Nonblank lines remain verbatim. An entirely
blank terminal returns an empty string. `--lines` keeps its default of 80 and range
of 1–100000, but output can exceed N physical rows. Text and JSON share this content.
Reads still preserve viewport and selection.

Ghost suggestions remain visible. Read/recover help, the agent skill, and CLI
documentation explain that plain terminal text cannot establish whether the
composer contains a draft. Callers must not infer drafts from suggestion text or
strip suggestions heuristically. `--confirm-empty` requires deliberate confirmation
that the composer is empty.

**Validation, 2026-09-30:** All 90 focused tests passed across `yinz-screen`,
`yinz-cli`, and `yinz`, including the existing skill installation test. Screen
coverage checks the captured 26-leading-blank layout, trailing whitespace,
internal paragraph spacing, exact nonblank limits, entirely blank terminals,
and unchanged viewport and selection. Response formatting tests verify identical
retained content in text and JSON, with the existing final newline in text output.

The compiled `send-keys`, `wait`, `read`, and `recover` help was inspected, as were
`skills show yinz-agents` and `skills show yinz-sessions`. Both skill files passed
the skill validator. `cargo x ci check` passed format, lint, 9,415 workspace tests
(54 skipped), doctests, and all 474 integration tests with 16 workers. Builds and
tests used this worktree's own native and Wasm artifacts, with socket access enabled.
No live-provider orchestration rerun was performed for these changes.

## P2 — misleading output, docs and skill gaps

| # | Finding |
|---|---|
| 18 | `status` on a terminal that never had an agent returns `target_changed`. `status terminal_999` returns `target_closed`. A held pane whose agent exited returns `target_changed` "target does not contain an agent". |
| 19 | The not-ready rejection always says "agent is not idle or already has a pending submission", even when the real reason is an unknown input revision. The JSON has no field for the reason. |
| 20 | `interrupted` or `status_unavailable` messages always say "Submission is uncertain… clear any draft, then use agent recover", even with `acknowledged:true`, and even for `/exit` where the agent is gone. |
| 21 | `agent spawn --help` has no text for `--kind`, `--model`, `--effort`, `--permission-mode`, `--resume`, `--file`, `--project`, `--worktree`, `--base`, or the Firecracker flags, and lists no valid kinds. The skill says to "Read `agent spawn --help` for model, effort, permissions…". `--kind bogus` returns "requires a recognized agent command" without listing kinds, and spawn without `--kind` returns "an agent or a command is required". (`--permission-mode bogus`, by contrast, lists its values.) |
| 22 | `send-keys` key syntax is undocumented. `"Ctrl u"` works; `Ctrl+u`, `ctrl-u`, and `C-u` are rejected. `wait --until` values appear only in error text. |
| 23 | The skill examples hard-code `--session main`. Haiku copied it verbatim ("cannot connect to session main"). They also use `--kind codex`, and haiku spawned Codex even though it was asked for Claude. |
| 24 | `agent read` counts blank TUI rows: 26 of 30 requested lines were blank. Claude's ghost-text suggestions ("Try 'fix lint errors'", "fix the sub function") look like a real draft. This conflicts with the recover guidance to inspect the draft and clear it. |
| 25 | `agent list` omits panes where `action context` shows `detected: codex` (a shell-launched Codex), so the two views disagree. |
| 26 | Pane ID formats are inconsistent. `agent register` takes `2`, other `agent` commands reject `7`, and `action` accepts both forms. |
| 27 | `agent runs --json` is rejected, while live commands need `--json`. Agents add it out of habit. |
| 28 | Branch collisions rename silently (`fix-sub` → `fix-sub-2`), with no `renamed`/`requested_branch` field. `.worktrees/` is untracked in the main checkout, so `git add -A` would add the worktrees as embedded repos. |
| 29 | `skills install`: `--agent` conflicts with `--dest`, and a bare `install` writes to all agents including opencode. `prompt TEXT --file` prints a misleading usage line. |
| 30 | `list-sessions` shows `pty-broker-v8.sock` as an UNRESPONSIVE session and suggests `kill-session pty-broker-v8.sock`. Haiku saw this too. |

## P3 — polish

- Spawn always lands in the last active client's tab (the user's working tab), and there is no `--tab` or `--floating` option. The pane title is the whole command line, including the roughly 3 KB `--settings` hooks JSON.
- `agent read` dumps the full scrollback, then trims it. Its cost on large scrollback was not measured.
- Codex UI registrations (`/tmp/yinz-1000/codex/ui-*`) leak when a pane is closed, because the launcher is killed before `TempDir` drops. The lease check makes them harmless, but they accumulate. The directory is hard-coded to `/tmp` and shared with every session and test run.
- The shared `$TMPDIR/yinz-<uid>/yinz-log/yinz.log` mixes live-session and test-run lines. Hook-report trace lines have no timestamps, so they could not be correlated.

## What worked

- Claude single-line prompts: `completed` with `last_assistant_message` in `message`, in about 1s. Back-to-back prompts, CRLF, emoji, Arabic, stdin (`--file -`), and control-character rejection all work.
- `needs_input` on a permission prompt, `wait --until exited`, `target_closed` on a mid-turn pane close, and the concurrency guard (the second concurrent prompt is rejected).
- Codex, after the two workarounds: `completed` in 4.5s with a `turn_id`, and it stays `ready`. Codex acknowledged a multi-line prompt (there is no paste wrapper there).
- Worktree spawn, collision rename, `--reuse`. The archive commands `runs`, `run`, `history-search`, and `predecessors` work.
- CLI validation for pane IDs, `--lines`, `--timeout`, missing `--confirm-empty`, invalid permission modes, and unknown projects.

## Skill-driven orchestration results

| Orchestrator | Task | Result |
|---|---|---|
| Claude haiku (bypass permissions) | Delegate a fix to a Claude haiku worker on `fix-sub` and verify it | **Failed.** It loaded `yinz-agents`, then tried `--session main` (copied from the skill), then `yinz list`, then spawned **Codex** `--model haiku`. The worker never became ready (P0 #1), so it did everything the skill forbids: `send-keys 'claude' Enter` into Codex, the invented `--stdin` and `action kill-pane -t`, `yinz run 'cd … && claude'`, and finally Claude Code's cross-session SendMessage (held for approval). The tests still fail. Its report was honest about the failure. |
| Codex luna (full access) | The same task on `fix-sub-luna` | **Succeeded.** It needed two manual warm-up turns first (P0 #1 and #2). It found the session with `list-sessions`, used `spawn --kind claude`, `wait` (ok), `status`, and a single-line `prompt --wait`. On `needs_input` it ran `read` and then `send-keys Down Enter`, which chose "don't ask again", and it verified the tests itself: 3/3 pass. |

Skill takeaways: remove the literal `--session main` and describe session discovery. The example kind should match the prose. Document the `send-keys` syntax and the dialog accounting fixed in #9/#10; manual/raw input still requires recovery. Warn that prompts should be single-line for Claude until #3 is fixed. Document the Codex sandbox, the missing environment, and the need for `--session`. Say what to do when a worker never becomes ready.
