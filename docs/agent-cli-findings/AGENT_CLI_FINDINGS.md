# Agent CLI findings

This file records the current state of the live shakedown of `yinz agent …`. The tests
drove Claude Code and Codex worker panes directly from the CLI. Claude and Codex
orchestrators also delegated a fix to a worker by following the bundled `yinz-agents` skill.
The latest live verification was on 2026-10-01 with Claude Code 2.1.286 (`--model haiku`)
and Codex 0.159.3 (`--model gpt-6-luna`).

Severity: **P0** means a documented flow cannot work and the CLI has no escape. **P1** means a
wrong outcome or a stuck state that has a workaround. **P2** means misleading output or a gap
in the docs or skill. **P3** means polish.

In the most recent orchestration run, both orchestrators completed the task without help.
Each one found its session from the environment, then ran spawn, wait, prompt,
`needs_input`, read, `send-keys Enter` and wait. Neither one needed `recover`, raw input or a warm-up turn.

## Open issues

| # | Sev | Issue |
|---|---|---|
| 17 | P1 | **A Codex auto-update during launch has not been retested live.** Codex 0.158 updated itself to 0.159 and restarted at launch, so the first wait returned `target_changed`. The launcher now survives backend connection failures before and after a conversation is selected. A launcher subprocess test against a mock backend covers this. |
| 38 | P3 | **clap parse errors ignore `--json`.** A bad pane ID or an unknown flag still prints plain stderr. Errors found after parsing return a JSON `AgentResponse`. |
| P3 | P3 | **Codex UI registrations leak.** The `ui-*` directories under the hard-coded `/tmp/yinz-<uid>/codex/` are not removed when a pane closes, because the launcher is killed before `TempDir` drops. The lease check makes them harmless, but they accumulate. |
| P3 | P3 | **The shared log is hard to correlate.** `$TMPDIR/yinz-<uid>/yinz-log/yinz.log` mixes lines from live sessions and test runs, and hook-trace lines have no timestamps. |
| P3 | P3 | **Spawn placement has no options.** Spawn keeps the caller's focus but has no `--tab` or `--floating` option. |
| P3 | P3 | **The cost of `agent read` is unmeasured.** It renders the full scrollback and then trims it. Its cost on a large scrollback has not been measured. |
| — | — | **Uninvestigated:** once, a prompt sent right after a fresh Claude spawn's `wait` returned `ok` was refused with `prompt_unsupported`. Status was `ready:true` a moment later. |
| 35 | P2 | **Not reproduced; watch for it.** A worker spawned into `.worktrees/fix-sub` once edited the main checkout's file by absolute path, and `prompt --wait` reported success. Later runs stayed inside their own worktrees. |
| 37 | P2 | **Not reproduced; watch for it.** Haiku once called a 55 KB paste ending in an instruction a "prompt injection" and ignored it, despite `PASTE_LEAD_IN`. A later 56 KB sample worked. |

## Claude background completion, 2026-11-05

Agenda workspace `yinz`, item 17, approved plan version 2 (separate from the
shakedown finding numbered 17 above). The earlier investigation at
`7ea5a7f4ef4dcee5ea491e1a82bc58b31e132582` identified overlapping worker/background
tracking and stale Stop side effects, but stopped before runtime implementation.
The controller subsequently supplied nine bounded Claude Code 2.1.318 captures;
all exited successfully and the controller's 336 fixture checks passed. This
implementation read those captures and the authorized incident transcript without
changing provider configuration or running further live-provider experiments.

The same-prompt capture disproves raw-hook execution correlation: both Starts
are identical and both Stops are byte-identical across two executions of one
worker. The agent ID and prompt ID remain the same. Reporter UUIDs, receipt time,
and a hook-time transcript tail do not repair that ambiguity. In particular,
Stop snapshots precede persisted successful handback results, and resumed Start
precedes the coordinator message in the worker transcript.

There is a usable independent completion path. Stream task_started and
task_notification records join task_id, run_id and the causal tool_use_id. On
SendMessage resumption, both run_id and the causal tool call change. Parent
transcript launch results preserve that causal tool ID; queued_command
attachments carry runId and task-notification XML with task-id/tool-use-id.
Notifications processed as turns instead carry origin.runId. These same two
notification shapes are present in the interactive incident transcript, so this
adapter does not depend on the print/stream-json interface. All three stale async
task IDs from the incident have a completed notification matching their launch
tool call with a nonempty run ID. A successful
SubagentHandback has success=true in the captured tool response and its linked
terminal result, followed by a completed task notification. The denied handback
has is_error=true and no toolEndsTurn; the adapter never interprets an attempted
handback or a raw Stop as successful completion.

The runtime observer now emits typed worker execution reports from these parent
transcript records. It validates the originating Agent/Task or SendMessage call,
its successful launch result, provider notification origin, conversation, task
and causal tool ID, and completed status with a nonempty run ID. It continues
following background work after confirming the parent's turn. Existing local,
pane-reference and broker routes carry the metadata through bounded broadcasts.
Screen tracking retains active executions and completed execution tombstones
separately from public task IDs. Completion retires only its own execution;
late starts, parent confirmations and notification confirmations cannot resurrect
it or retire another execution of the same task. Ordinary prompts and pane
transfer preserve this evidence. Conversation replacement resets it. Explicit
recovery records abandonment separately from successful completion.

Raw SubagentStop is excluded from activity and wait reconciliation. It remains
available as an archive-only event, and its receipt still invalidates an Esc
recovery quiet-period proof; it cannot publish Idle or clear a worker wait. An
uncorrelated Start for an unknown task conservatively records work. A raw Start for a known task now also
retains provisional activity: a nested resumption must not disappear when an
older observed execution finishes. Background Bash tasks and notification
processing retain their independent lifetimes. Submission results
remain historical and keep their original launch lists. Completion never consumes
an unsubmitted draft or changes its input revision. A recognized nonempty Claude
composer now retains unconsumed_input readiness when the last worker finishes;
support for guarded clearing is not proof that the draft has been consumed.

**Conservative protocol fallbacks:** raw worker input hooks do not identify the
execution. A wait can arrive before the resumed launch is persisted, so execution
completion alone cannot release it. Worker input and release hooks do carry
`tool_use_id` when available. Screen tracking now retains pending and released
tool identities per worker. Matching PostToolUse, PostToolUseFailure,
PermissionDenied, or ElicitationResult releases that tool's wait even after
completion has marked the worker stopped. Older or unrelated releases cannot
clear a newer wait; multiple outstanding tools remain waiting until each releases.
Released identities suppress reordered waiting hooks and survive ordinary prompts
and pane transfer. Missing tool identity, including a legacy handoff lacking that
metadata, conservatively retains Waiting. An unrelated later tool is not proof
that the missing-identity wait ended. Conversation replacement and explicit
recovery clear these pending waits.

Root transcript observation still cannot establish nested same-ID executions.
Raw Starts for already-known tasks therefore retain provisional worker activity,
including when a start might be a duplicate or the adapter later observes another
execution. Neither a parent confirmation nor completion of any observed execution
can retire this ambiguous activity. It remains Busy (or Waiting for input) until
explicit recovery or conversation replacement. This prevents premature Idle for
unobserved resumptions; it does not claim automatic completion for ambiguous raw
starts. Automatically retiring them still requires validated nested launch
observation or a complete provider task stream. Arrival order and synthetic
generations cannot supply that evidence.

Minimal sanitized projections are committed in
`yinz-process/src/unit/fixtures/claude-task-executions.json`. They include
same-prompt resumption, processed notifications, successful and denied handback,
a resumed input request, and blocked plain Stop. XML prose is removed and the
identity/status tags are reconstructed from the controller sanitizer's extracted
fields; source line numbers and provider version preserve attribution. Private
raw captures, paths and alias maps are not committed.

Revision validation includes normal screen report-path regressions using the
captured same-prompt task/tool/run identities, completion-before-release and
release-before-completion, stale and missing-identity releases, reordered waits,
raw starts for known tasks, PaneUpdate and pending idle waits, and transfer of
provisional activity and input correlation. Broker and bounded-broadcast checks
preserve tool identities; IPC roundtrip and legacy handoff coverage include the
new fields. The first focused run passed 354/356 tests; the two failures were
older fixtures expecting identity-free releases to clear waits. Those fixtures now
supply matching tool IDs, while separate regressions verify the conservative
missing-identity behavior. The final affected selection passed all 357 tests
across process observation, screen control and transfer, hook decoding and pruning,
and IPC/broker transport. Two strengthened recovery/publication assertions also
passed their focused rerun. `cargo x lint`, `cargo x format --check`, and
`git diff --check` passed. No full suite, full CI gate, merge, rebase, push or
remote Git operation is part of this task. No live installed binary was replaced.

The controller's subsequent merge gate exposed an outdated IPC steering fixture:
`claude_steering_delivers_without_interrupting_and_publishes_background_activity`
sent only raw SubagentStart/Stop hooks, so the worker correctly remained active
and its idle wait timed out. A focused run reproduced the failure in 32.7 seconds.
The fixture now also sends the observer's correlated launch and completion reports,
using task/tool/run identities from the same captured provider fixture validated
by the process tests. Additional assertions retain work after the raw Stop, remove
the worker after correlated completion, suppress duplicate completion and late
parent confirmation, and retain Busy for the independent Bash notification until
its turn finishes. The repaired integration test passes in 2.7 seconds without
changing runtime acceptance rules or weakening the steering/publication assertions.
The 12 focused process/screen checks also passed, covering captured provider
evidence, stale completion after resumption, worker waits, independent draft
readiness, aggregate activity, and pane transfer. Lint and formatting checks
passed. The controller retains ownership of the next full merge gate.

### Resumption acceptance blocker, 2026-11-06

**Issue 17 remains unresolved and is not ready to merge.** The conservative raw
Start handling above also prevents ordinary, fully completed resumption from
ever becoming Idle. It is a safety fallback, not an acceptable resolution of the
reported bug. The repair at `b5f32045a` remains intact. This investigation makes
no runtime transition changes because the approved plan requires validated
correlation before relaxing completion acceptance.

Two additional bounded Claude Code 2.1.318 runs used `claude-haiku-5-5` in visible
Yinz panes, with isolated temporary hooks, no repository-write tools, no MCP
servers, a $5 limit per invocation and a 240-second process timeout. Both exited
0 without timing out (7.85 and 14.72 seconds); every started worker execution had
a successful provider completion, and both panes were closed. The sanitized
projection is `yinz-process/src/unit/fixtures/claude-resumption-observation.json`.
Only identity, status and success fields are retained; private raw transcripts,
paths and alias maps are not committed.

The first invocation repeats an ordinary launch, successful handback, same-ID
SendMessage resumption and successful handback in one parent prompt. Both raw
Starts are identical and both raw Stops are identical, including the fields
omitted from the sanitized projection. Each worker Stop's `background_tasks`
snapshot still lists that worker as `running`. The final parent Stop lists no
background tasks, but neither snapshot carries an execution revision. Inspecting
this previously unused field therefore did not establish a safe ordering or
completion contract for delayed hooks.

The second invocation proves that nested resumption is real, not merely an
adversarial synthetic possibility. After the first worker completed, a second
worker used SendMessage to resume it. The stream contains three distinct
executions and three completed notifications. The root transcript contains only
the two Agent launches; the resuming SendMessage call occurs in the second
worker's sidechain. The root receives the resumed run's completed notification,
but the current adapter cannot join it to a launch and emits no execution report
for that run. Its pending set is empty despite missing an entire execution.
Feeding the sidechain into the current adapter also produces no events: it
explicitly rejects sidechains, and the nested SendMessage result lacks the
top-level `toolUseResult.resumedAgentId` used by the root adapter.

The new screen characterization replays the captured raw hooks together with
the observer identities through ordinary report acceptance. With only observer
reports, both executions complete and the pane reaches Idle. Adding the actual
raw hooks leaves both execution tombstones completed but the worker provisional,
and the background ID outstanding. An initial assertion
of the required Idle outcome failed with `left: Busy, right: Idle`; this is the
unresolved defect, not a flake. The committed characterization explicitly asserts
the current Busy outcome to preserve the evidence without claiming acceptance.
Early test drafts also mixed an unrelated readiness assertion into this activity
replay: an idle wait had no reply, and a guarded-submission variant returned
TargetChanged. The final characterization isolates report acceptance and activity;
it makes no claim to validate input readiness or idle-wait delivery. Existing
draft and control regressions remain in the focused selection.

The ambiguous histories are: (1) the second raw Start describes g2, whose
correlated completion arrives later; (2) g2 already ended, a nested g3 starts,
and its identical raw Start arrives before delayed g2 observer reports. Both
produce the same available screen reports, but only the first may become Idle.
The second history is a constructed delivery ordering using the demonstrated
payload ambiguity and nested launch gap; it is not claimed as a live transport
reordering experiment. Clearing provisional activity on any observed start or
completion, counting raw starts, waiting for a quiet interval, or inspecting
only transcript EOF cannot distinguish these histories. Parent registry Idle
and parent completion likewise do not establish that all workers stopped.

Concrete design alternatives for the controller:

- Obtain an ordered provider task stream covering root and nested executions,
  with an initial complete snapshot and a reconnect boundary. The captured print
  stream supplies task/tool/run identities for the nested case. Yinz's current
  interactive launcher does not consume that stream; the installed CLI exposes
  stream-json output in print mode. An interactive transport or a deliberate
  provider bridge design needs validation before replacing raw-hook activity.
- Obtain provider execution identity or a comparable event revision on native
  worker hooks and task snapshots. Then raw starts can be joined to the same
  executions as notifications without assigning identity from delivery order.
- Extend transcript observation to every relevant sidechain, but first validate
  nested launch/result shapes and a completeness boundary that accounts for
  records not yet persisted at hook time. Recursive scanning alone is not such
  a boundary and does not safely discharge provisional activity.

Validation: 75 selected process/screen tests passed, followed by all 18 focused
tracking-handoff tests. The selection includes the existing stale completion,
worker wait, draft independence and transfer safety regressions. `cargo x lint`,
`cargo x format --check`, and `git diff --check` passed. These results validate
the characterization and unchanged safety behavior, not the unmet automatic-Idle
acceptance criterion.

Explicit recovery remains a workaround only. No full CI gate, merge, rebase,
push, remote Git operation, lifecycle mutation or Agenda mutation was performed.

## Known limitations and caller guidance

The following behavior is intentional or not worth fixing. Callers and the skill should account for it.

- **Uncorrelated input needs `recover`.** Yinz cannot correlate human or raw input. Examples include a dialog answered by hand and `/clear` or `/new` typed through `send-keys`. After such input, the pane is not ready until `recover`. Instead, use `agent prompt PANE /clear` (Claude) or `agent prompt PANE /new` (Codex or Pi), and answer dialogs with `agent send-keys`.
- **Ghost suggestions can look like drafts.** Plain terminal output cannot confirm an empty composer. Inspect the composer and clear any draft before using `--confirm-empty`.
- **Text-only orchestrators cannot verify recovery's empty-composer precondition.** When terminal UI inspection is unavailable, ask the user to verify the stopped agent and empty composer. The skill no longer presents `Esc` followed immediately by `recover` as a complete recovery procedure.
- **A startup dialog is not reported as such.** A wait during a startup dialog reports `status_unavailable` with `not_ready_reason: prompt_unsupported`. If a fresh spawn stays `Spawned`, `read` the pane.
- **`--allow-yinz` allows all network access.** It lets a sandboxed Codex reach the Yinz socket by enabling the sandbox's network access, which also allows the internet. For this reason it is opt-in.
- **Hosts need current hooks and the current build on `PATH`.** Rerun `yinz hooks install --agent claude`, or use `--project` / `--project-dir`, on each host with an existing install. Older scripts send duplicate reports. Inline hooks run `yinz` from `PATH`, so an older build there breaks background-task tracking (#31).
- **`--reuse` warns but does not refuse.** Reusing an occupied worktree reports the other agents in `occupied_by` and proceeds.
- **Background tasks hold the idle wait.** `wait --until idle` stays pending until every background notification turn finishes. A long-running background server therefore holds the wait until its timeout. The run archive records `<task-notification>` turns as `prompt` entries.
- **Submission lookup is temporary.** `wait --submission ID` retains the original turn's outcome across timeout and `needs_input`, but only the last 32 submissions per pane/conversation are kept in memory. Target replacement or server restart discards them. An unknown result does not establish that the task never ran.
- **A worker's Git baseline must be chosen explicitly when it differs from `main`.** New worktrees do not copy the caller's uncommitted changes. The skill shows `--base` and tells orchestrators to select the intended starting commit or branch.
- **Most Codex model IDs are not validated.** Only Claude aliases (`haiku`, `sonnet`, `opus`, `opusplan`, case variants and the `[1m]` suffix) are rejected for Codex. The provider must validate other IDs.
- **One Esc case is covered only by a screen test.** Claude 2.1.286 sometimes backgrounds a command when Esc interrupts its Bash call (#44). An unknown task notification leaves the pane ready. In later live runs, Esc killed the command instead.

## Resolved findings

| # | Was | Now |
|---|---|---|
| 1 | P0: A spawned Codex never became ready, because `SessionStart` arrived only at the first turn. | The bridge sends a synthetic `SessionStart` when the UI selects a conversation. Spawn followed by wait is ready in under 0.5s. |
| 2 | P0: Codex's hidden title-generation thread took over the pane binding. | `yinz-codex` records ephemeral threads, and they never take the binding. |
| 3 | P0: Claude prompts of five or more lines, or about 1,000+ characters, ended `interrupted`. | `prompt_sha256` unwraps `<pasted_content>` before hashing. Long Claude prompts get the `PASTE_LEAD_IN` prefix. The archive and the title store the unwrapped prompt. |
| 4 | P0: A wait started before `SessionStart` returned `target_changed`. | The wait adopts the first conversation. A repeated start for the same conversation retargets only after a turn is accepted. |
| 5 | P0: Esc during a Claude turn deadlocked the pane at Busy. | `recover --confirm-empty --interrupted` accepts a Busy or Waiting agent after 3s with no reports. A plain `recover` names the flag. |
| 6 | P0: Esc during a Codex turn went back to Busy after a late hook. | Busy and Waiting hooks for an interrupted `turn_id` are dropped until the next `UserPromptSubmit`. A plain `recover` works. |
| 7 | P0: Codex tool commands had no `YINZ_*` environment. | The bridge injects `shell_environment_policy.set.*` on every non-ephemeral thread start, resume or fork. |
| 8 | P0: A sandboxed Codex could not reach Yinz. | Use `spawn --allow-yinz`. A refused connection explains the sandbox and names the flag, and `list-sessions` shows it as `UNREACHABLE`. |
| 9 | P1: Startup dialogs could not be answered from the CLI. | `send-keys` reaches a `Spawned` agent. The first verified `SessionStart` consumes those keys. This works for Claude, and for Codex once the trust prompt below is answered. |
| 10 | P1: The pane was not ready after `send-keys` answered a permission prompt. | Parent progress consumes the key batch. The next wait returns `ok`/`ready:true` with the turn's final text (#45). |
| 11 | P1: The agent disappeared after Claude `/clear`. | A `clear` start restores an `Idle/Reported` record. `agent prompt /clear` returns `target_changed`, and the next prompt needs no recover. |
| 12 | P1: Prompts containing U+200B or U+202E were never submitted. | These two characters are removed before Claude delivery. Codex text and emoji joiners are unchanged. |
| 13 | P1: Slash and `!` input timed out and kept its claim. | Slash commands (both agents) and `!` (Claude) return `rejected` before any write. Exact Claude `/clear` and Codex `/new` are exempt. |
| 14 | P1: A turn reported `completed` while Stop hooks could still continue it. | Claude Stop means Busy. A transcript observer confirms only the matching `turn_duration` and returns the final text. |
| 15 | P1: Claude hooks fired twice when hooks were installed. | Inline hooks set `YINZ_CLAUDE_INLINE_HOOKS=1`, and v10+ installed scripts defer to them. Existing installs need a refresh; see the limitations above. |
| 16 | P1: `--kind codex --model haiku` was accepted. | Claude aliases are rejected for Codex before spawning. |
| 18 | P2: Lookup outcomes were wrong for non-agent and missing panes. | Lookups return `not_agent`, `target_not_found` and `target_exited` correctly. |
| 19 | P2: The not-ready rejection gave no reason. | `not_ready_reason` reports `prompt_unsupported`, `not_idle`, `pending_submission`, `unconsumed_input` or `unmanaged`. |
| 20 | P2: Recovery advice was wrong for acknowledged and `/exit` cases. | Messages distinguish uncertain acceptance from an accepted turn. Closed, exited and replaced targets get no recovery advice. |
| 21 | P2: `spawn --help` and the kind errors were undocumented. | Every option is documented. Kind errors list every kind and mention the JSON `command` alternative. |
| 22 | P2: `send-keys` syntax and `--until` values were undocumented. | The help documents them, including the unsupported aliases (`Ctrl+u`, `Return`, …). |
| 23 | P2: The skill hard-coded `--session main` and `--kind codex`. | The skill uses inherited pane context or explicit discovery, and a labeled Claude example. Explicit selection uses the root option `yinz --session NAME agent ...`; `YINZ_SESSION_NAME` alone is not a selector. |
| 24 | P3: `agent read --lines N` counted only nonblank rows and could return more than N rows. | Internal blank rows count toward the limit. Blank padding at both ends is removed, and trailing blank rows do not consume the limit. |
| 25 | P2: A shell-launched Codex was missing from `agent list`. | It is listed with source `detected` and `not_ready_reason: unmanaged`. Targeting it returns `not_agent` with a hint to relaunch it. |
| 26 | P2: Pane ID formats were inconsistent. | Every pane argument accepts `terminal_7` or `7`. `plugin_N` is refused with a reason. |
| 27 | P2: `agent runs --json` was rejected. | Archive commands accept `--json`. |
| 28 | P2: Branch collisions were renamed silently. | The response includes `requested_branch` and a message. `/.worktrees/` is added to `info/exclude`. |
| 29 | P2: `skills install --agent` could not be combined with `--dest`. | The flags can be combined. `--dest` selects the exact skills directory. Help explains that a bare install writes to every supported agent. |
| 30 | P2: `list-sessions` offered to kill the pty broker. | Sweeps skip `pty-broker-v<N>.sock`, and `kill-session` and `delete-session` refuse it. |
| 31 | P1: Claude background work ended the turn early and left the pane stranded. | The prompt reply lists `background_tasks`. `wait --until idle` returns the notification turn's final text. Notification turns never consume input. |
| 32 | P1: A failed Codex turn left the pane Busy. | The bridge maps a failed `turn/completed` to `StopFailure`. The prompt returns `failed` with the provider's message and `ready:true`. |
| 33 | P1: A Codex resume with `--permission-mode` exited at once. | The command returns `rejected` with exit 1, before any pane or checkout is created. |
| 34 | P2: `--reuse` silently shared a live worktree. | It reports `reused` and `occupied_by` (a warning, not a refusal). |
| 36 | P1: Spawn took focus in the user's tab. | CLI spawns keep the active pane and tab, including overflow placement. |
| 39 | P3: Prompt-suggestion subagents were archived. | A `SubagentStop` with an empty `agent_type` is not archived. |
| 40 | P2: Codex `/new` left the pane not ready. | `agent prompt PANE /new` is tracked like Claude `/clear`. A wait issued during the gap stays pending for the new conversation. |
| 41 | P3: Pane titles and `action context` carried about 4 KB of hooks JSON per pane. | Titles read `KIND (MODEL)`. `launch::display_command` removes the inline hook settings and markers from the displayed command. |
| 42 | P1: A Codex spawned into a worktree of an untrusted repo never became ready. | The launcher shows `Trust this worktree? [y/N]` and saves trust for that worktree only. `send-keys y Enter` followed by wait returns `ok`. |
| 43 | P2: Panes adopted across a server restart kept the old session's environment. | `yinz` asks the supervisor named by `YINZ_PANE_REF` for the current session and pane. An explicit `--session` still wins. |
| 44 | P2: Esc made Claude background a command, and the pane was left stranded. | An unknown task notification leaves the pane ready (screen test only). |
| 45 | P2: No command returned the final answer after `needs_input`. | A successful idle wait returns the last completed turn's text. |
| 46 | P3: An interrupted, acknowledged Codex turn reported `pending_submission`. | Accepted claims report `unresolved_turn` until completion or recovery. Unacknowledged claims still report `pending_submission`. |
| 47 | P3: Closed stdout caused action output to panic and agent output to print an error. | Action and agent output stop quietly when the reader closes the pipe. Other output errors still fail. |
| 48 | P3: `agent runs` omitted session identity across sessions and servers. | Run replies include `session` and `host_id`. Older archive headers recover these fields from their stored row columns. |
| 49 | P2: The skill had no teardown or project steps. | The skill covers `action close-pane --pane-id`, worktree cleanup, `--file -` heredocs, `action project-list`, the new spawn fields and JSON errors. |

## Implemented after the 2026-10-02 review

The following updates have automated controller, launch, archive, and CLI process
regression coverage. Both skills passed format validation and an independent
scenario review. No fresh live-provider verification was performed; the
2026-10-01 live results above remain separate from these automated checks.

| # | Was | Implemented change |
|---|---|---|
| 50 | P2: Skill provider support and first-run prerequisites drifted from the implementation. | The skill includes Pi and AGY, per-host hook setup, AGY completion API requirements, and Pi `/new`. It keeps unsupported or unverified prompting from silently falling back to raw input. |
| 51 | P2: The skill prescribed redundant status/read calls in the successful flow. | Spawn, idle wait, and prompt use their returned checkout, readiness, identity, and answer fields. Extra reads are reserved for diagnostics; parallel workers can be submitted before waiting. |
| 52 | P1: A later idle wait could hide the failure of the task an orchestrator originally submitted. | Prompt responses expose a submission ID. `wait --submission ID` resumes observation of that exact operation after timeout or dialog handling, including its failure outcome. State waits retain their separate readiness meaning. |
| 53 | P2: Skill recovery examples could encourage asserting an empty composer after Esc alone. | The incomplete example is removed. The skill and guide require actual terminal inspection and direct text-only orchestrators to ask for human verification. |
| 54 | P2: Archive continuation was described by tool-call-shaped strings. | Timeline replies use boolean `more` and numeric `next_revision`. The skill explains numeric list and timeline continuation and distinguishes server selection from archive ownership filters. |
| 55 | P1: Explicit launch kinds replaced custom executable paths, and command-only launches missed provider behavior. | Provider identity is inferred separately from the executable. Custom paths are preserved, aliases share provider behavior, conflicting kinds are rejected, and unsupported interpreter/resume combinations explain how to use an executable shim. |
| 56 | P1: A failed spawn could leave a checkout or pane without returning its identity. | Failures return known `spawned` or attempted `launch_resources`, and Pi reporting setup precedes Git changes. |
| 57 | P2: Registration rejected the common `--json` option. | Registration accepts `--json` for structured errors and documents its existing service acknowledgement format. |
| 58 | P1: Caller text resembling a Claude task notification could lose prompt correlation. | A matching submitted prompt takes precedence over notification classification. |

The full validation run also exposed two shell fixture assumptions and a restart
fixture that could accept the previous exit panel's render. Those fixtures now
preserve their shell process and wait for fresh rendered content, respectively.
The Git-plugin fixture now waits for the intended client's subscriptions before
sending a reload probe and reports full startup errors instead of a render timeout.
A recording-compaction property test found an unrelated search checkpoint moving
backward when compacted input had a higher event ID than remaining output; search
now retains the highest ID across both sources, with a deterministic regression.

Final validation: `cargo x ci check` passed with 16 test workers, including
formatting, lint, doctests, all 9,665 workspace tests and all 478 whole-app
integration tests. The Git-plugin fixture also passed ten consecutive isolated
runs after its readiness fix. Both bundled skills passed `quick_validate.py`.


## Guarded draft clearing, 2026-10-04

`agent clear-draft` requires a conversation, the exact inspected input revision, and explicit discard confirmation.
The screen thread owns the composer across marker delivery, one Ctrl+C, and stable-empty verification.
Manual recovery and verified clearing share submission finalization. Clearing never consumes newer input.
Unsupported layouts fail closed. Partial redraws pause verification without sending keys or counting empty-frame stability.

Disposable live panes validated these installed versions with default editing bindings:

| Provider | Layout requirement | Result |
|---|---|---|
| Claude Code 2.1.289 | Visible cursor and distinguishable dim placeholder styling | Clear preserved the conversation and restored readiness. The guarded new prompt completed with `CLEAR_DRAFT_NEW_ONLY`. Multiline and wrapped drafts also cleared with the cursor at the start of the last row. |
| Codex CLI 0.160.0 | Visible cursor and the captured fixed, dim placeholder | Clear preserved the conversation and restored readiness. The guarded new prompt completed with `CLEAR_DRAFT_NEW_ONLY`. Multiline clearing passed after verification was made tolerant of transient hidden cursors during redraw. |
| Pi 1.0.0 | Existing `PI_HARDWARE_CURSOR=1` setting and matching default cyan editor borders | Clear returned `ok`, preserved the conversation, and restored readiness. The provider acknowledged exactly the new prompt. Its model request failed with HTTP 401, so model completion was not verified. |

Claude's unstyled suggestion text remains ambiguous and does not count as empty.
The colored Claude check used `TERM=xterm-256color`, `FORCE_COLOR=1`, and empty `NO_COLOR`.
Pi's default hidden cursor is rejected. Its border check prevents rules typed into a draft from hiding earlier input.
Muse, AGY, and custom providers/editors are explicitly unsupported.

Captured empty and multiline frames, including dimensions, cursor coordinates, and provider versions, are in
`yinz-screen/src/unit/fixtures/agent-composers/`. Tests distinguish typed placeholder lookalikes from dim UI hints.
Controller tests cover exact revisions, confirmation, quiet time, interrupted recovery and abandoned submissions,
concurrent commands, lifecycle changes, disconnection, write failure, deadlines, partial redraws, and stable emptiness.
Disposable validation sessions were removed after testing.

Final validation: `cargo x ci check` passed formatting, lint, doctests, all 9,781 workspace tests,
and all 492 whole-app integration tests. The bundled agent skill passed `quick_validate.py`.
Two failures in an earlier full run (Git-plugin CLI reload and the TypeScript agent harness) passed in isolation
and in the final full run.
