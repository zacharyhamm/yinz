# Issue 69 S1: positive obligations in the interactive adapter

**R69-1 is narrowed, but remains unresolved for the full plan-v3 scope.** A
normally delivered native launch hook can acquire an obligation before its
launching turn ends. That obligation can survive arbitrary notification-journal
lag. A negative publication boundary is **not necessary** for the demonstrated
root-notification/local-shell-cancellation subset. The prior report's broad
absence-of-closure argument does not disprove this subset.

There is, however, a specific supported producer path with no correlated terminal
input: an interactive worker wake declined by its function/plugin hook. Its
removal journal deliberately omits content, and an ordinary shell notification
has no command UUID to substitute. An acquisition cannot be discharged from that
record. Sidechain answer completion and gated tool-result delivery also need the
additional mappings described below. This report does not declare a lifecycle
finding resolved, approve the plan, or authorize S2–S5.

Investigation: 2026-11-09 UTC; `issue-69` at
`e1dd464c82f9a6ad2633209ca62e3e8d18df10a1`. The user authorized S1 execution with
“approved, implement”; `/tmp/issue-69-approve-v3.json` records that request and
its log says `Resolve high/critical findings first: R69-1`. No lifecycle method
was invoked. No production code was changed. The
[prior report](issue-69-notification-investigation.md) and all three of its
evidence files are preserved byte for byte.

## Evidence and compatibility

This is a pinned implementation mapping for **ordinary local interactive Claude
Code 2.1.319**, executable
`/home/zacharyhamm/.local/share/claude/versions/2.1.319`, SHA-256
`dabafec82ca1319fa43d8be36a10fc61b1d11821cb98ad5df04ad95e199b8eed`.
It uses native synchronous Yinz command hooks and regular transcript files.
It does not establish compatibility with another executable, diskless sessions,
served/remote-shell tool results, disabled hooks, async reporting hooks, mods,
print/SDK mode, or passive REPL-bridge attachment.

Source citations **S1–S23** refer to the preserved
[original index](issue-69-notification-evidence/provider-source-index.json).
**A1–A31** refer to the new
[source index](issue-69-s1-adapter-evidence/provider-source-index.json).
Every entry specifies exact zero-based executable byte offset, length and SHA-256;
all 54 excerpts were checked against that executable. Source text remains private:
`/tmp/yinz-issue-69-investigation-spzepqfl/verified-excerpts/S*.txt` and
`/tmp/issue-69-s1-adapter/A*.txt`. These are executable-source findings, not a
vendor promise about future versions.

**C followed by a number** means the one-based line in the existing
[149-record capture projection](issue-69-notification-evidence/interactive-capture.jsonl).
Its `source` field names the original raw file and line. The new
[answer-boundary projection](issue-69-s1-adapter-evidence/answer-boundaries.json)
adds only 35 structural rows for seven duration-to-prompt ancestry chains. The
original selected projection omitted intermediate attachment edges; the new
artifact closes those joins using the original identity aliases. It includes no
prompt bodies, thinking, output files, credentials or unrelated sessions.
Both capture artifacts are measured evidence from the prior Haiku experiments,
whose installed Yinz build commit remains unknown. This assignment launched no
Claude session, agent, pane, collector, watchdog or child job.

## Acquisition before a clear

| Input and exact fields | Acquisition / ordering | Evidence and limit |
| --- | --- | --- |
| Native `PreToolUse`: `session_id`, `prompt_id`, optional `agent_id`, `tool_name`, `tool_use_id`, `tool_input` | Identifies the launching tool invocation. `run_in_background` is intent, not proof a task exists. An optional provisional tool intent can live until its successful/failed Post; ordinary turn Busy already covers the invocation. | A1–A2. C10/C45/C50/C112. Neither intent nor command text is a task lifecycle key. |
| Native successful Bash `PostToolUse.tool_response.backgroundTaskId` | Acquire `(process incarnation, conversation, owner, launching prompt, launching tool-use ID, task ID)` **before returning the reporter hook**. Do not require a registry sample or `turn.background`. Handle automatic/manual backgrounding by actual result, not only requested backgrounding. | A2–A8/A11; C12, C51, C114. A11's explicit local branch returns the background ID even if the process completed before adoption finished. A foreground result without an ID does not acquire this obligation. |
| Native successful Agent Post: `tool_response.{isAsync:true,status:"async_launched",agentId}` | Same acquisition, including a foreground worker subsequently moved to background. Do not parse an “async launched” prose sentence. | A12–A13; C47, corroborated by saved tool result C54. Measured tool name is `Agent`. A separately successful `Task`-spelled launch was not captured or validated; do not advertise that spelling/version as covered merely by aliasing it in the parser. |
| Native root `Stop.background_tasks[]`: `id`, `type`, `status` | Positive fallback for still-in-flight work. Attach to the already acquired identity when known. Otherwise retain a provisional identified-task obligation, awaiting launch/run attribution. Never erase an obligation because a later Stop omits it. | A9–A10; C14/C57/C116. Filter includes running/pending and excludes explicitly foreground work. Task ID alone cannot distinguish successive resumed passes. |
| Worker hooks: `agent_id`, root `session_id`, `prompt_id`, native `agent_transcript_path` at SubagentStop | Attribute child launches to their owner; preserve the owner's outstanding root obligation. Worker `prompt_id` can reflect the current root prompt and is not a worker-pass completion token. | A1/A9; C61 is under prompt-3, C76's same worker tool result is under prompt-4. C78 lists the finishing worker as still running. Current screen treats SubagentStop as archive-only. |

The provider path is concrete: the tool caller passes `Rs.data` through
`pct -> _dr -> ZM`; `_dr` constructs the Post input; `pct` exhausts that async
generator; configured command hooks await `E1`, which waits for child exit and
stdio in its synchronous path. Explicit async/asyncRewake hooks take another
branch. Stop likewise constructs its snapshot then `yield* ZM`; its callers
exhaust the generator before completing the turn (A2–A9; Stop call sites at
301396701 and 301407980 (A30–A31), corroborated by C17/C18, C59/C60, C119/C120).
This is the existing bounded hook execution, not a new launch barrier.

On the Yinz side,
`src/agent_report.rs:151` sends the report and waits for its matching
`AgentReportAccepted`; `yinz-server/src/client_router.rs:131` requires successful
enqueue onto the screen queue **before acknowledgement**. The hook therefore
cannot normally return successfully before its acquisition report is enqueued.
The later same-turn Stop/confirmation is enqueued after that. Acknowledgement
does **not** mean screen application. The proposed S3 change must process and
retain this identity before applying an Idle-producing confirmation, across
normal deferred pane adoption too. No separate plugin/event subscriber receipt
is an application barrier. `yinz-screen/src/screen/agent_reports.rs:159` parses
reports, and `:382` tracks background before applying state; those are the
eventual integration points, not functionality added by this report.

This establishes acquisition for **successfully observed native launches**:

1. Work finishes before the first registry read: its successful Post still
   identifies the asynchronous task; registry false does not release it.
2. Work finishes during `settle_background`: the Post acquisition predates that
   call, even if its first false sample arrives at 300 ms and the accepted notice
   is visible at 800 ms.
3. A notice or its confirmed answer is observed before the observer reads the
   launch row: retain positive joins by identity until both facts are available.
   Do not let collector arrival ordering substitute for provider ordering.

Today `confirm` samples in `yinz-process/src/claude_turn.rs:569`,
`settle_background` at `:593` can return false, and `run` at `:312` watches only
when `turn.background` is true. `watch_background` at `:617` clears on registry
false. The screen's replaceable marker at
`yinz-screen/src/screen/agent_control.rs:535` also admits newer false replacement.
Those are actual integration gaps to fix later; they do not prevent early native
acquisition. Hook timeout/report loss is a different path: the one-second local
watchdog (`yinz-runtime/src/agent/report.rs:13`) and provider hook budget let
Claude continue. The adapter must expose unavailable/degraded observation under
the accepted policy, not label a lost acquisition or an expired read as settled.
It cannot promise correctness after a dropped report as if the report arrived.

## Proposed adapter transition table

An obligation is acquired for a particular launch/resume, then optionally bound
to a notice run. Prompt IDs, task IDs, notice run IDs, worker IDs and transcript
row UUIDs have different roles. Keep identities/tombstones until dependent
answers finish; an old task/run completion cannot release a new resume.

| Current fact + positive input | Retained transition | Answer boundary and actual input mapping |
| --- | --- | --- |
| No task obligation + acknowledged asynchronous launch | `acquired`, unbound to a notice run | Original submission may complete with LAUNCH text; aggregate remains pending. Native acquisitions above are demonstrated. |
| Acquired + identified queue enqueue | `acquired`, optionally annotate queued task/tool | S5/S8–S13 queue content carries task/tool, ordinarily **not run ID**. Persistence lag of any length leaves it pending. No EOF or timer inference. |
| Acquired + root accepted user row | Bind `(task/tool, origin.runId)` to `promptId` and root row UUID; state `answer_pending` | Require `type:user`, matching `sessionId` when supplied, `isSidechain:false`, `origin.kind:task-notification`, `origin.producer:session-task`, nonempty `origin.runId`, structured envelope `<task-id>`/`<tool-use-id>` and compatible acquired identity. C27 joins task-1/tool-1/run-2 to prompt-2; C71 and C86 similarly join mixed tasks. Native notification UserPromptSubmit starts an ordinary per-prompt observer but need not itself carry all those IDs. |
| Root answer pending + correlated confirmed turn end | `settled` for that obligation only; publish that answer's text before aggregate idle | Exact explicit prompt identity or UUID ancestry to its accepted row; linked `system/turn_duration` or the existing guarded root confirmation path. Raw Stop alone, another prompt's answer and generic end_turn text cannot settle it. C29/C33/C35 establish FINAL_B; C72/C73/C75 and C89/C96/C98 establish mixed answers. Seven measured ancestry chains are in the new projection. The existing root `Turn::record` (`claude_turn.rs:713`) supplies the per-prompt/ancestry machinery. This is no claim that installed CLI wait `message:null` was fixed. |
| Acquired local Bash + successful exact TaskStop | Bind no-notice disposition to the **cancelling** prompt; wait for that prompt's confirmed answer | Native Post `tool_name:TaskStop`, matching request `tool_input.task_id`, resolved `tool_response.task_id`, `task_type:local_bash`, successful non-error result. Scope/owner and unique live launch must match; do not apply to a notice already bound to a different processing turn. S6–S7/A14–A15 kill/stamp notified before later normal claim. C128/C129/C131 identify task-9; C132/C137/C139 correlate CANCELLED_E. Foreign task, failed Post, ambiguous/prefix request without resolved join, remote served shell and agent TaskStop do not satisfy this shortcut. |
| Acquired + `remove/reason:delivered_as_tool_result` | Transfer to identified receiving tool invocation and its turn; delivery itself remains pending | S21 consumes the task's queued notice. A28 establishes the actual caller: gated **GetTask**, input `taskId`, existing snapshot for same owner, non-inner call, non-aborted signal. Its native Pre/Post tool ID and prompt can identify the receiving turn if retained. S21's abort callback **re-enqueues** the notice; an interrupted receiving turn cannot be treated as successful processing. Native Post plus matching task snapshot and non-error tool-result row are needed before successful receiving-turn completion. No fresh capture. A28 says `isEnabled(){return!1}` on this tool surface; A29 additionally gates snapshot registration. This is a source-only, feature-gated candidate, **not proof TaskOutput exposes equivalent inputs**. |
| Acquired + `remove/reason:delivered_to_agent` | Transfer to a specific sidechain accepted user row; remain `answer_pending` | S15–S17/S22–S23 and A19–A20 join the notice through `joinedNoticeRunIds`, aligned `joinedNoticeLengths`/`joinedNoticeOwn`, envelope task/tool IDs, worker `agentId`, file scope and row UUID. Require array alignment and structural content validation, not loose text search. Delivery commit happens before worker dispatch. The 2.1.318 historical fixture at `37994adff:yinz-process/src/unit/fixtures/claude-interactive-observation.json`, `worker_records[5]`, supports the positive join, not its answer end. Fresh 2.1.319 sidechain records are ordinary C-tool work, not redirected notice validation. |
| Sidechain answer pending + worker-pass completion | Must settle only the attributed processing pass, including its answer and any required root handoff | **Mapping not yet established for all supported resumes.** A21 shows stream completion precedes finalization/review and owner notice; a parked worker can receive a duration row while owner notification is deferred. A22 allows inline/no-owner-notice behavior. Current `Turn::record` rejects sidechains. A raw SubagentStop, final assistant stop_reason, registry false, or delivery acknowledgement cannot be substituted. A terminal owner notice plus its root answer could cover a proved pass join, but neither task ID alone nor the historical fixture proves that join across repeated resumes. An internal abstract `worker_answer_end` in the replay is explicitly an assumed input. |
| Queued notice + stopped / failed-resume terminal removal | If genuinely terminal, discharge only a correlated notice with explicit no-answer/no-reroute disposition | S16 produces `agent_stopped` or `resume_failed` after its specific retry/error rules, with content retained by S15. A18 identifies pre-dispatch refusals. A27 shows another runner may requeue `agent_stopped` notices to root: the reason string alone cannot mean terminal globally. In the pinned interactive Mqe branch, unique task/tool plus its acquired resume may permit a narrow join; repeated notices with identical task/tool need run/command attribution absent from that removal content. Source candidates, not measured terminal coverage. |
| Queued notice + hook-declined wake | Needed: correlated `no_answer` disposition; **current record cannot supply it** | Mqe receives `deniedByPlugin` and removes consumed commands with `dropped_by_hook` (S16/A18/A19). S15/A16 deliberately omit content; S5/S8/A17 ordinary shell command construction supplies neither commandUuid nor deliveryId. Exact missing path below. |
| Any + registry idle / empty Stop / generic dequeue / raw Stop / newer sampled false | No settlement transition | These are execution/turn hints. They cannot erase an acquired obligation or substitute for delivery/answer identity. Both registry/notice orders, pre-first-read and settle-time completion follow the same rule. |
| Any + unknown format, truncation/overflow, process/conversation replacement, lost report/read | Explicit unavailable/degradation; never successful settlement | Preserve the accepted nonblocking fallback. A known obligation may wait through arbitrarily delayed publication while the configured observer remains available. Existing operational bounds may end observation as unavailable; elapsed time supplies no completion proof. |

Repeated runs need **positive acquisition for each resume**. A26 gives one
candidate native SendMessage result: `success:true` plus
`resumedAgentId`/`revivedAgentId`. It has inline and other branches; it is not
validated here as an all-resume contract. Root accepted `origin.runId` prevents
an old answer from settling an already acquired newer pass. It cannot, by itself,
protect a preceding interval for a resume whose acquisition was never observed.
Do not reopen/clear every run solely by task ID. Direct Agent async launches are
demonstrated; other resume routes remain explicitly outside that guarantee.

## The decisive smaller missing path

This is a source-supported trace, **not a new live reproduction**:

1. A worker launches an ordinary background shell A. Native Post identifies A;
   S5 constructs its eventual task notification for that owner worker. The root
   launching obligation is retained too. Completion can be arbitrarily short.
2. Shell completion claims/enqueues its notification (S4–S6). `qCn` constructs
   `{value, mode, agentId, taskId, ...}` without `uuid`/`deliveryId`; `ll` simply
   forwards (A17); `Kr` stamps runId but adds no UUID (S8).
3. The interactive host installs `Mqe -> g8` (A19). Its resume function/plugin
   hook returns `kn.deny`; `g8` returns `deniedByPlugin` **before** delivery
   commit/worker execution (A18). Mqe removes that notice as `dropped_by_hook`.
4. `pl` emits only
   `{type:"queue-operation",operation:"remove",reason:"dropped_by_hook",
   timestamp,sessionId}` for this ordinary shell command. It deliberately passes
   undefined content and optional undefined commandUuid/deliveryId (S9/A16).
   No accepted root/sidechain row for A is created by this denied delivery.

Even with every relevant row eventually persisted, the adapter cannot positively
join this terminal record to A. With two eligible notices A and B, either
internal removal projects to the identical record. Enqueue records preserve
their task/tool envelopes but no command/run/recipient join for this removal.
At the same observed prefix, A can have been declined while B remains pending,
or B declined while A remains pending. A later completion of an unrelated turn
does not distinguish the disposition of A. Inferring closure from queue counts,
absence of a later row, or an assumed empty set requires exactly the generic
negative inference the proposed contract excludes. The replay reproduces this
projection collision directly from the allowlisted producer fields.

Keeping the obligation pending is safe, but cannot provide normal eventual
settlement on this terminal path. Treating it as unavailable is a candid
compatibility limit, not proof full plan-v3 acceptance is met. Reporter loss is
not involved in this counterexample. Nor does journal delay cause it: waiting
longer cannot recover an identity the producer omitted.

The smallest provider-side alternative is an identified queue disposition for
each notice, retaining `(taskId, runId, recipient, reason)` even on a hook drop,
plus an attributed worker-pass end for sidechain processing. A command UUID on
**both** enqueue and disposition could supply an equivalent join if run/task
attribution survives transformations. A normal-observation internal hook
transport for those two facts would suffice; no total ledger, epoch, global
watermark, EOF boundary or restoration after report loss is required. No such
transport was installed or claimed available here. A smaller product scope
limited to demonstrated root/local-shell paths is technically plausible, but
requires the controller's acceptance decision; it is not silently adopted here.

## Minimal native data and ownership

The plan's private observer module and minimal native additions are sufficient
for the demonstrated subset **if** normalization runs before generic pruning:

- Retain bounded Stop task `{id,type,status}` and allowlisted launch/result
  identifiers above. Existing `tool_input`/`tool_response`, prompt/tool/session
  fields suffice at ingress; extract typed internal identity facts there.
- Preserve those facts across `HookEvent::pruned_to` and forwarding. Today
  `background_tasks` is not a typed HookEvent field, and `pruned_to` drops
  `tool_response` (`yinz-types/src/agent/hooks.rs:826`). Small raw JSON preserves
  unknown fields; oversized raw reserialization loses them. Archive
  `tool_capture` previews are bounded/redacted prose, not an authoritative ID
  channel (`yinz-runtime/src/agent/hooks.rs:24`). Never recover IDs from a cut
  preview. Overflow/omission must mean incomplete/unavailable, not empty.
- A per-prompt observer owns original submission text and completion; screen
  retains acquired obligations across newer prompts. The launching observer
  must continue while its obligations exist even when registry false made
  `turn.background` false. Notification UserPromptSubmit creates the answering
  observer; its confirmed result names the bound notice/answer to settle.
  Idempotent acquisition/joins handle duplicate installed hooks and delayed rows.
- Scope all facts to process incarnation and conversation, then launch owner,
  prompt/tool/task and notice run. Keep old run tombstones through handoff;
  preserve existing pane generation, foreign/archive and target-replacement
  guards. A Stop-only provisional ID may remain pending until attribution is
  obtained. Do not let current worker-ignore/ended-turn filters discard a
  necessary positive acquisition before aggregation.

These additions preserve observable information. They cannot synthesize the
missing hook-drop identity or a worker-pass completion the native inputs do not
name. Another native field containing only task ID would not fix repeated-run
or sidechain attribution. S1 therefore validates an implementable subset and
locates the remaining gate; S2–S5 remain untouched.

## Checks, artifacts and cleanup

Reproduce from this worktree:

```sh
python3 docs/agent-cli-findings/issue-69-s1-adapter-evidence/replay.py \
  --executable /home/zacharyhamm/.local/share/claude/versions/2.1.319 \
  --output /tmp/issue-69-s1-adapter/check-results.json
python3 /tmp/issue-69-s1-adapter/verify.py
git diff --check
```

The executable diagnostic and [20 schedules](issue-69-s1-adapter-evidence/schedules.json)
exercise both event orders; pre-first-read and settle-time completion; newer
false marker replacement; cancellation without notice; an inert logical delay
of one day; answer-before-accepted-row observation; repeated task/tool and run
identity; duplicate positive delivery after settlement; foreign scope;
tool-result/sidechain transfers; stopped/resume-failed/hook-drop candidate
inputs; anonymous removal; foreground-only completion and
explicit loss. They assert pending through each gap, exact synthetic final text,
and separate unavailable state. Abstract alternate-delivery/end inputs are
**synthetic assumptions** where labeled. Passing them does not establish a
provider input guarantee.

Measured normalization separately verifies four native acquisitions, three root
notice bindings, one local shell cancellation and seven correlated root ends.
It retains FINAL_B, mixed final text and CANCELLED_E through those joins. This
checks the proposed mapping over measured records, not the current Yinz runtime,
screen application, idle-wait message or an opposite-order live reproduction.

Initial diagnostic runs found two harness issues: it assumed every registry row
had `status`, and it tried to traverse four launching/cancelling ancestry chains
through the intentionally selected old projection. The fixes ignore unknown
registry rows as nonproof and use the supplemental measured structural edges.
Both initial failures and their causes are recorded here; neither was a flaky
provider test. The corrected replay and source verification pass. The final
verification wrapper also corrected its treatment of an empty-diagnostic
`git diff --no-index --check` exit 1 (ordinary file difference) and excluded the
matched PID 1 `codex` controller from experiment-process results. Python AST
parsing, all new JSON parsing, structural-projection validation, preserved-file
hash checks and whitespace checks also pass; final machine results and cleanup
are in the private paths below. No Cargo build/test, full suite, installation,
configuration edit, commit, merge, push or external communication was performed.

Alongside this report, durable supporting deliverables are `replay.py`, `schedules.json`,
`answer-boundaries.json`, `provider-source-index.json` and `check-results.json`
under `docs/agent-cli-findings/issue-69-s1-adapter-evidence/`. The result file
contains only sanitized identifiers and source references.

Private/raw paths:

- `/tmp/issue-69-s1-adapter/`: A1–A31 excerpts, diagnostic contexts,
  historical fixture copy, `preserved-sha256.json`, `check-results.json`,
  `verification.json` and `cleanup.json`.
- `/tmp/yinz-issue-69-investigation-spzepqfl/`: original capture, hooks,
  raw root/worker records, identity aliases, pinned S excerpts and previous
  cleanup evidence. No raw/private material was copied into Git.
- `/tmp/issue-69-approved-plan-v3.json`, `/tmp/issue-69-approve-v3.json`,
  `/tmp/issue-69-approve-v3.log`: plan and existing approval/rejection inputs;
  read without mutation.

No live experiment resources were created. Excluding its own process ancestry
and the PID 1 Codex controller, the final cleanup check finds no process whose
argv references either issue-specific diagnostic directory. It also rechecks
the prior experiment provider PIDs from the original cleanup evidence.
No background agent/job was left running by this assignment. The foreground
controller/worker pane is not an experiment pane and remains available.
