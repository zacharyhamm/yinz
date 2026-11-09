# Issue 69: notification settlement evidence

**Verdict: usable settlement signal not established. R69-1 remains open.** There
is a limited, version-scoped candidate: retain a positively identified notification
obligation until its delivery disposition and resulting turn completion are
observed. Current interactive captures supply several of those records, but no
inspected contract closes the interval before they become observable. This is
evidence for plan adjudication, not approval or a product implementation.

Investigation: 2026-11-09 UTC (November 8 locally), branch `issue-69`, clean starting
commit `e1dd464c82f9a6ad2633209ca62e3e8d18df10a1`. No lifecycle state was changed.
The CURRENT preambles of [the findings](AGENT_CLI_FINDINGS.md) and
[the observation contract](CLAUDE_OBSERVATION_CONTRACT.md) govern interpretation:
registry tracking replaced transcript execution reconstruction, and the historical
complete-ledger prerequisite was superseded. Issue 69 does **not** require epochs,
complete replay, or perfect observation after reporting failures. It does require
avoiding premature Idle during normally observed completion/notification handling.

**Evidence and provenance.** Raw captures, programs, source excerpts, private
identity mapping and cleanup results are under:

`/tmp/yinz-issue-69-investigation-spzepqfl/`

Durable projections:

- [Interactive records](issue-69-notification-evidence/interactive-capture.jsonl):
  149 selected records, with raw filenames/line references and observation times.
- [Pinned source index](issue-69-notification-evidence/provider-source-index.json):
  S1–S23 identify exact executable byte ranges and excerpt checksums.
- [Synthetic schedules](issue-69-notification-evidence/synthetic-schedules.json):
  six explicitly constructed scheduling/projection cases, not live reproductions.

Claude was **2.1.319**, executable
`/home/zacharyhamm/.local/share/claude/versions/2.1.319`, SHA-256
`dabafec82ca1319fa43d8be36a10fc61b1d11821cb98ad5df04ad95e199b8eed`.
Live control used the **installed** Yinz `0.1.2-alpha.9`, SHA-256
`d652c29941365234e7b39fa040637f309b91c4854f52225e60c59ead46aa91fb`.
Its build commit was not independently established. These runs are not a fresh
reproduction with a binary built from this worktree's HEAD.

The supplied `/tmp/yinz-claude-idle-tracking-report.md` reports 3 ms and 1.644 s
Idle gaps under Sonnet, plus background Bash/nested-agent scenarios. Those are
supplied observations, not measurements repeated here. Historical interactive
2.1.318 evidence at `37994adff:yinz-process/src/unit/fixtures/claude-interactive-observation.json`
shows sidechain delivery and reporter failure. Commit `ef6b09921` removed that
adapter and its fixtures; it did not establish that Claude removed the records.

**Supported surfaces checked.** Official documentation was inspected on
2026-11-09 UTC; `official-docs-snapshot.txt` preserves the principal lookup.

The [SDK reference](https://code.claude.com/docs/en/agent-sdk/typescript#sdkbackgroundtaskschangedmessage)
promises task updates and terminal notifications before a membership snapshot
removes the task. Repeated initialize supplies a current-set snapshot. Run IDs
join task events and saved notification origins. These are useful producer
contracts, but task completion is earlier than the answering turn's completion.
The same reference defines `queued_turn_count` for human messages only. These SDK
facts do not establish an interactive notification queue boundary.

Installed `claude --help` restricts structured input/output to `--print`, matching
the [CLI reference](https://code.claude.com/docs/en/cli-reference). The pinned event
queue admits events in noninteractive mode or with an active REPL bridge (S1–S3).
It therefore has interactive bridge consumers; claiming the signal exists only in
print mode everywhere would be wrong. No supported passive attachment of Yinz's
ordinary interactive process to that event stream was established. No SDK/print
substitution, private connection, or Remote Control session was used.

[Stop hooks](https://code.claude.com/docs/en/hooks#stop-input) expose in-flight
`background_tasks`, including task IDs, but no run/delivery boundary. The reference
warns that transcript persistence may lag Stop. A worker's SubagentStop snapshot
in the fresh mixed run still contains that worker as running.
[Agent view](https://code.claude.com/docs/en/agent-view#list-sessions-as-json)
provides supported external session status, without a documented notification
settlement guarantee.

The additional [interactive mod reference](https://code.claude.com/docs/en/plugins/mods/reference)
documents turn events, subagent listing, conversation messages and pre-storage
`session.append`. These could reduce persistence lag. Its inspected event/API
inventory does not specify a pending-task-notification disposition boundary.
No mod was installed or tested. This is a bounded negative finding about inspected
interfaces, not a claim that every provider transport lacks the needed signal.

**Pinned producer call paths.** These are read-only implementation findings for
the checksum above, not a cross-version provider promise. S references resolve
through the source index; raw excerpts remain outside Git.

1. Normal background Bash completion processes/flushed output, updates the task
   to terminal status, claims notification ownership, then enqueues its notification
   (S4–S6). The interactive presence calculation considers loading/delegated work
   and live Bash status independently of notification queue membership; registry
   writes occur through an asynchronous effect (S18–S20). Neither task removal nor
   registry Idle is a journal-persisted-through acknowledgement.
2. Interactive enqueue stamps the task's run identity, selects a root/worker
   recipient, and may defer admission. Actual queue mutation notifies subscribers
   before the enqueue journal callback (S5, S8–S9). That callback may itself defer
   records and invokes an asynchronous append without awaiting it (S10–S13).
   Observed transcript EOF therefore cannot prove no notification is pending.
3. Ordinary enqueue journals carry content, while ordinary dequeue journals can
   omit both command identity and content (S9, S14). Root accepted notification
   rows do carry `promptId`, task/tool IDs in their content, and `origin.runId`.
   These are positive joins, not evidence that all dispositions were published.
4. Worker routing consumes on delivery commit with `delivered_to_agent`, and has
   stopped, failed-resume and hook-declined alternatives (S15–S17, S22–S23). Delivery commit
   precedes the worker's answer finishing. Sidechain `joinedNoticeRunIds` still
   exists in this build; historical captures suggest its use, but no fresh nested
   redirected-delivery run was performed.
5. There are real no-root-notification paths. Explicit shell stop marks the task
   killed/notified, preventing a later normal notification claim (S6–S7). A queued
   shell notice can also be consumed as `delivered_as_tool_result` (S21). Root-only
   notification counting would mishandle these dispositions.

**Fresh interactive experiments.** Visible panes were launched through Yinz in
`sincere-cowbell`, with explicit `--model haiku`; the sole requested subagent also
used `model: haiku` and a custom Haiku definition. Retained root and requested
sidechain model records name `claude-haiku-5-5`. Settings/fixtures were task-local
under `/tmp`, MCP servers disabled, tools restricted, and original Yinz hook
commands retained alongside a passive recorder. No reporting failure was injected.
Each session had an approximately 120-second bound; controlled commands also had
an 85-second escape deadline. All exited before those limits.

The first setup attempt used a timeout wrapper with the version filename as
Claude's argv[0]. The Yinz ancestor recognizer expects `claude`
(`yinz-process/src/claude_turn.rs:270`); its observer reported `status_unavailable`.
That capture remains at the raw directory root, excluded from the durable live
projection and from successful-wait conclusions. Subsequent launches used ordinary
`claude` directly. The passive recorder never gates provider behavior.

`run2/timeline.jsonl`, `hooks/`, `barriers.jsonl`, `raw/root.jsonl`, the requested
worker transcript, prompt files and CLI results preserve the usable run. Times
below are collector arrival seconds from that run's start:

| Time | Observation |
| --- | --- |
| 24.427 | Root Stop: `LAUNCH_B`, shell task still running. |
| 24.509 | Yinz confirms the launching submission with `background:true`. |
| 24.731 | Controlled B barrier released/exited, approximately 300 ms after Stop. |
| 24.775 / 24.796 | Registry sampled busy; notification UserPromptSubmit observed. |
| 24.847 | Enqueue, dequeue and accepted root notification become visible in the transcript. Their original timestamps remain in the capture. |
| 25.502 / 25.512 | Notification Stop: `FINAL_B`, empty task snapshot; registry sampled idle. |
| 25.584 | Yinz confirms the notification turn and reports Idle. |
| 69.202 | Mixed launching Stop: one shell and one Haiku worker running. |
| 71.708 / 74.705 | D and C barriers exit separately. |
| 74.567 | First notification Stop: `ACK` for D; worker remains running. |
| 77.559 / 77.594 | Final notification Stop includes `FINAL_MIX`; Yinz confirms Idle. |

The B original submission returns `LAUNCH_B` independently. Its idle wait lasts
0.855 s and finishes after the notification completion. However, the installed
CLI returns `message:null` for these state waits, including the mixed case; final
text is established by Stop/transcript records, **not** by successful wait output.
Thus these runs do not demonstrate the final-text acceptance requirement. No
aggregate Idle blip was captured in these usable runs.

An important measured ordering distinction: the B notification hook arrived
about 52 ms **before the enqueue record was visible on disk**. Its provider
enqueue timestamp precedes the accepted-user timestamp. Arrival order is not
producer order. Registry/transcript polling was 10 ms; file persistence and
scheduling add uncertainty, and sub-poll registry transitions could be missed.
No partial JSON records occurred in the usable collector runs. Hook timestamps
are recorder-arrival times; barrier exit is controlled process evidence, not the
provider's task-state update time.

`run3` tests genuine cancellation after Yinz has confirmed background activity:
Stop at 4.461 s lists E running; successful TaskStop at 6.282 s identifies E;
Stop at 6.832 s says `CANCELLED_E` with an empty snapshot; registry is idle at
6.853 s. No queued/root task notification appears through cleanup about three
seconds later. The finite window alone cannot prove eternal absence; S6–S7
explain why this particular disposition need not enqueue one. The existing later
sampled-false confirmation clears background in this case legitimately.

**Candidate semantics and limits.** All candidates must retain process
incarnation/conversation/task/run attribution and keep original submission
completion separate from aggregate settlement.

| Candidate | Pending evidence | Safe clearing / no-notification handling | Unavailable evidence |
| --- | --- | --- | --- |
| Registry plus Stop snapshot | Live status; known tasks at Stop | Neither Idle nor an empty later snapshot closes an unseen notification obligation. Cancellation succeeds here, but the same registry-only rule races on ordinary completion. | Existing hook-based degradation; no new permanent Busy requirement. |
| SDK task stream plus correlated result | Task events and membership; terminal notification creates response obligation | Membership removal settles execution only. A correlated result settles the answer; cancellation/delivery policy must settle obligations that produce no response. | Reinitialize helps restore live membership, without proving an already-ended response obligation. Interactive access remains unestablished. |
| Interactive task/queue/turn correlation | Background launch result or Stop task ID before the first idle sample; then positively observed queued/accepted run identity | Keep pending through the corresponding root answer, or explicit correlated cancellation/tool-result/sidechain disposition and its necessary processing. TaskStop is a concrete no-root example. A generic dequeue or delivery receipt alone is insufficient. | Preserve Claude operation and degrade observation explicitly under the accepted policy. Missing records cannot justify a normal-path correctness claim. |
| Mod turn/append hooks | Accepted prompt/row and identified turn | Turn completion can settle that observed answer; it does not identify a completion whose notification has not reached those hooks. No no-notice closure established. | Nonblocking reporting remains necessary; no mod recovery claim was tested. |

The interactive candidate must acquire pending obligations independently of
`settle_background == true`; otherwise the short-work bypass remains. A newer
sampled-false Stop cannot erase an older unresolved obligation. This is narrow
notification tracking for observed work, not a prerequisite for reconstructing
every execution or introducing a complete ledger.

**Coverage and remaining discrimination.** Fresh tests cover short shell release,
mixed completions, root notification acceptance/completion, and explicit
cancellation. The shell release happened after its background marker was already
confirmed: it did **not** exercise the initial busy-settle bypass. The supplied
opposite event order, initial 300/800 ms bypass, newer false-marker replacement,
and a notification delayed beyond two seconds are synthetic scheduling cases in
the linked artifact. No live provider queue/persistence delay was injected;
running a task longer is not such a delay. Completion before the first status
sample, fresh nested/redirected or coalesced notifications, independent observer
or registry loss, process/conversation replacement, and person/worker waits,
drafts/readiness and stale/foreign/archived guards were not freshly tested.
Historical reporter-loss evidence remains distinct from a current provider loss
experiment. No wider Rust or integration suite was run.

**Smallest next step.** Keep runtime implementation gated. Validate a producer
boundary for the already-observed tasks that says whether their completion has
created a root response obligation, was delivered elsewhere, or was explicitly
settled without one, before exposing normal-path inactivity. The SDK ordering is
a promising model, but needs an applicable interactive interface or a narrowly
reviewed pinned call-path adapter covering these dispositions. Positive root
notification/turn joins are already available; another timer measurement will
not supply the missing negative boundary. R69-1 is narrowed by concrete records
and a real cancellation counterexample, but is not resolved. Requiring global
snapshot/replay recovery before even considering this narrower adapter would
reintroduce the superseded stronger premise.

**Reproduction and preservation.** These commands use the preserved diagnostic
harness and create a new capture directory for each live rerun:

```sh
RAW=/tmp/yinz-issue-69-investigation-spzepqfl
python3 "$RAW/reproduce.py" short
python3 "$RAW/reproduce.py" mixed
python3 "$RAW/reproduce.py" cancel
python3 "$RAW/replay-schedules.py"
python3 "$RAW/extract-provider.py" \
  docs/agent-cli-findings/issue-69-notification-evidence/provider-source-index.json \
  /home/zacharyhamm/.local/share/claude/versions/2.1.319 /tmp/issue-69-source-check
```

The extraction verified all 23 indexed ranges against the pinned executable;
the six synthetic cases and diagnostic Python syntax checks passed. Live reruns
were stopped after sufficient discriminating evidence, rather than used to infer
an arbitrary-delay guarantee.

The durable capture uses allowlisted fields and consistent identity aliases;
ordinary user prompt bodies, account identifiers, provider request blobs, thinking,
unrelated sessions and terminal dumps are omitted. Full captured notification
envelopes are preserved with only fixture path/identity substitutions. Original
timestamps, field-presence lists and raw line references retain the distinction
between producer records and local observations. These aliases and collector
sequences are not provider watermarks. Provider source text remains outside Git.

Panes `terminal_53`, `terminal_55`, and `terminal_56` were closed. `cleanup.json`
confirms all three provider processes absent and no remaining diagnostic
collector/watchdog/barrier Python processes. The Codex investigation pane remains
for its controller. No product code, installed binary, normal configuration,
Agenda/Swamp state, commit, merge or push was changed.
