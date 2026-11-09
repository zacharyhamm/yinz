# Claude activity observation and historical completeness investigation

## Current design: registry hints and positive root notification obligations

The registry remains an execution hint. In the validated local Claude Code
2.1.319 root subset, native successful asynchronous Bash/Agent results acquire
identified obligations before launching confirmation. A structural accepted root
notice and its correlated confirmed answer settle the matching obligation;
successful exact local Bash TaskStop waits for the cancelling answer instead.
Registry false, empty snapshots, EOF and elapsed time are not answer evidence.
The screen retains these responsibilities independently of per-prompt observers,
preserves launch results, and publishes final text before aggregate idle waits.

See the [adapter mapping](issue-69-s1-adapter-mapping.md) for pinned source proof,
measured input/ancestry projections and explicitly synthetic schedules. The
[earlier investigation](issue-69-notification-investigation.md) preserves its
original verdict/provenance. This subset supersedes its all-path implementation
gate; it does not resolve anonymous worker removal or prove sidechain answer ends.
Overall issue 69 stays open. Unsupported formats/routes, unproved resumed passes,
worker-owned tasks and gated tool-result delivery retain existing fallback.
Observation loss, capacity and the 24-hour operational budget degrade observation;
none proves completion. Handoff preserves pending facts and the remaining budget.
An explicit unavailable notification observer (including a registry-only turn end
without an answer boundary), or expiry of its attributed obligation, releases that
notification's activity lifetime as unavailable. It does not settle the task or
replace final text. Independent work remains pending, and a later real confirmed
answer may still update text in turn order. Ordinary foreground answer history
cannot exhaust the adapter: only identified dependencies are protected from the
bounded recent-answer cache's pruning.
No complete execution ledger, epochs or replay prerequisite is restored.

The general transcript/sidechain reconstruction described below was removed. Its
old fixtures remain in Git history; it is not the implementation of this subset.

## Superseded acceptance: best-effort activity tracking

The user superseded the complete-ledger prerequisite below. Claude must stay
interactive when reporting fails. Occasional stale status from missing or delayed
reports is accepted; a completeness epoch, blocking launch barrier, transport
journal, or public observer-health API is not required or implemented.

Correlated launches and completions now supersede provisional raw worker activity.
Once all observed executions finish, worker/background activity is removed. An
unidentified newer execution can temporarily be hidden by delayed evidence; its
subsequently observed launch restores Busy. Raw Starts arriving after all observed
executions finish cannot reopen them, even when the observer has no new records
to report or replays completed records. Exact execution tombstones still stop
an older identified completion from finishing a known newer execution. Input waits
continue to require the matching tool release, and draft readiness is independent.

A detached, host-local observer follows the root transcript and worker sidechains
through parent Idle, scoped by provider process incarnation, conversation and
reporting route. A nonblocking file lock deduplicates triggers from parent prompts
and worker starts. Reporting remains bounded on the existing local and remote
routes. Failure exits the observer without clearing activity; a later trigger can
restart it and replay the transcripts. Provider exit or conversation replacement
ends observation. There is no timeout-based task completion.

Launch results recognize `resumedAgentId` and `revivedAgentId`; successful active
worker messages create no execution. Provider run identities deduplicate root and
sidechain notifications, including a single `joinedNoticeRunIds` entry. Notification
XML may be bare or preceded by the exact captured system banner; arbitrary
surrounding prose and multiple notification blocks remain rejected. Unmatched
completions are retained until their launch appears. A completion without a causal
tool ID is associated only with the sole observed initial Agent/Task launch for
that worker. SendMessage resumes and workers with multiple observed launches need
causal attribution, even if only one execution remains unfinished. An unmatched
notice can acquire attribution from a later copy of the same provider run; matched
run tombstones are never reassigned. This is best-effort correlation, not a complete provider execution ledger.

The sanitized captures below remain evidence of observed provider behavior. Their
old completeness verdict is preserved as investigation history, but no longer
blocks the accepted activity fix. Verification and fresh review are recorded in
`AGENT_CLI_FINDINGS.md`; implementation alone is not a claim that merge gates ran.

## Historical contract and rejected completeness prerequisite


**Historical verdict: rejected / incomplete under the former completeness requirement.** This is the
acceptance contract for a future provider adapter, not an implemented protocol
or a claim that the installed provider satisfies it. Investigation started from
`53ac5e388` on `idle-detection`, on 2026-11-06 UTC.

Claude availability takes precedence when observation fails. No synchronous
launch barrier, `onFailure: "block"`, or replacement of interactive Claude with
print mode is authorized by this contract. Existing conservative activity,
completion, waiting, draft and transfer handling remains in place.

## Required provider capability

A supported interface for the **interactive** provider process must expose:

1. A complete initial snapshot at a provider revision, including pending
   launch/resume operations and all root, nested and provider-initiated work.
   It must bind the conversation and process incarnation. Attaching to an
   existing process cannot imply an empty initial snapshot.
2. Execution identities that distinguish successive executions of one worker,
   with caller, causal tool call, target worker, run and attributable outcome.
   A successful message to an active worker must be distinguishable from a
   resumption. Helper termination must not discard a child's execution record.
3. An ordered change stream from that snapshot's boundary, with replay from a
   provider cursor. Disconnect, overflow, unavailable history and process
   replacement must either reconstruct all missing work or explicitly invalidate
   completeness. Quietness, hook counts, local sequence numbers, transcript EOF,
   parent Idle and transport acknowledgement are not provider boundaries.
4. Non-blocking reporting: observer, journal and Yinz failures leave Claude
   operational. Missing evidence invalidates completeness instead of preventing
   a provider operation. Reconnection restores completeness only after validated
   snapshot/replay, not merely after a successful socket connection.

The missing capability is **a supported interactive execution snapshot plus
revisioned replay (or an equivalent completeness boundary), covering nested and
automatic resumptions across attachment, observer loss and provider restart**.
No qualifying interface was found among the inspected surfaces below. This is
not a claim about every private provider transport or future provider release.

## Inspected surfaces

Installed Claude Code: `2.1.318`; binary SHA-256:
`708c77b204db30086ff6f1ed34577bda03e1be74ced80557cbf7b54249ebf0bd`.
The help and installed binary were inspected without modifying them. Official
documentation was checked on 2026-11-06 UTC; current docs may describe features
newer than this pinned binary.

| Surface | Evidence and reason it does not pass |
| --- | --- |
| `claude agents --json [--all]` | Installed help and the [session registry reference](https://code.claude.com/docs/en/agent-view#list-sessions-as-json) describe session status. The fresh capture returns an interactive session with `status: idle`, no per-execution inventory or revision/cursor. |
| `claude logs`, `claude attach` | Installed help describes recent terminal output and terminal attachment. Neither advertises an execution snapshot or replay cursor. |
| Hooks, including Stop `background_tasks` | [Hook documentation](https://code.claude.com/docs/en/hooks#stop-input) defines in-flight task summaries, without execution runs or a revision boundary. It also warns that transcript persistence can lag Stop. The fresh worker Stop still lists the stopping worker as running. |
| Root and sidechain transcripts | Captured identities can reconstruct some finite executions. Nested notification routing and delayed persistence defeat root-only tracking. No inspected interface supplies a persisted-through watermark that closes the gap between files and running work. |
| `--output-format stream-json`, `--replay-user-messages` | Installed help and the [CLI reference](https://code.claude.com/docs/en/cli-reference) restrict structured I/O to print mode. Re-emitting input messages is not task-history replay. Prior print captures validate identities only. |
| Hidden `--sdk-url`, `--session-mirror` | Installed option descriptions identify SDK streaming and SDK-internal transcript mirroring. The WebSocket option explicitly requires print and stream-json. Their presence is not a supported interactive observation contract. No private endpoint was connected. |
| Remote Control | The [documented interface](https://code.claude.com/docs/en/remote-control#what-connected-devices-see) synchronizes connected user surfaces. It does not specify a third-party execution snapshot/replay API. It was not enabled or used for the experiment. |

## Fresh interactive failure capture

Sanitized evidence:
[`claude-interactive-observation.json`](../../yinz-process/src/unit/fixtures/claude-interactive-observation.json).

A new visible Yinz pane ran interactive Haiku 5.5, without `--print`, with a
120-second process bound, isolated hooks, no MCP servers, no repository-write
tools, and two custom worker definitions limited to six turns each. The worker
returned FIRST; a helper then sent that worker a message requesting SECOND.
The temporary reporter privately archived each hook before dropping both nested
SendMessage PreToolUse and PostToolUse reports and exiting 1. Other reports
continued. This simulates failure of those reporter invocations, not a real Yinz
server outage. There was no `onFailure` override and no launch barrier.

Observed outcomes:

- The nested call executed despite both reporter failures. Its successful
  response contains `revivedAgentId`, without `resumedAgentId`.
- The worker emitted another same-ID Start and Stop. Its completed notification
  was queued in the root transcript, then removed with `delivered_to_agent`.
  The helper's sidechain received it with `joinedNoticeRunIds`. The root did
  not receive it as a normal task-notification user turn.
- The helper also emitted another Start/Stop without another Agent/SendMessage
  launch call in the capture. Its final notification has a run identity but no
  causal `tool-use-id`. This is evidence that call-only tracking needs more
  validation; it does not establish the full set of automatic launch paths.
- Sidechain `hook_non_blocking_error` attachments record both hook failures.
  These are useful attributable diagnostics when eventually read, but do not
  certify that all other hooks or transcript records were observed.
- The supported registry returned Idle. The existing root adapter still misses
  the nested launch and retains the helper as pending. The parent answer itself
  said SECOND was unobserved, although the helper's transcript contains its
  completed notification. Parent text cannot serve as execution ground truth.

The capture exited 0 on Ctrl-C after 55.93 seconds including idle inspection;
the process bound did not expire. Its pane was closed and its process was absent.
The incident pane and its draft were not targeted.

Two setup failures preceded the usable capture: interactive `--agents` rejected
a file path (corrected to inline JSON); the initial tool restriction left the
worker with no recognized tools (corrected by allowing Read in the parent and
using ordinary worker answers). Those attempts are not acceptance results.
Interactive mode does not support the print-only dollar-budget flag; this run
was bounded by time, tools, worker turns and the task's two explicit launches,
not a claimed dollar cap.

The fixture retains only allowlisted identity/status fields with consistent
aliases. The sidechain record at source line 21 now retains the complete captured
system banner verbatim. The previous sanitizer mistakenly removed that envelope
while reconstructing the notification XML, hiding a production parse rejection.
`source_keys` records field presence without private values. Capture
sequence numbers, source lines and `reporter_exit`/`delivered` are experiment
metadata, **not provider revisions or loss signals available to Yinz**. Raw
transcripts, paths and private identifiers remain outside Git. Prior print-mode
fixtures remain separate and are not relabeled as interactive evidence.

## Executable evidence and acceptance status

The two new process tests exercise actual fixture records through the existing
adapter:

- `captured_interactive_reporter_loss_does_not_block_nested_resumption` verifies
  non-blocking hook failure, nested success, sidechain completion identity, and
  the remaining root-observation gap.
- `captured_transcript_replay_recovers_identities_but_cannot_certify_missing_launches`
  withholds a captured ordinary resumption result from the existing print
  fixture. The adapter reports no pending work despite missing that launch;
  delivering the result late then leaves it pending because the unmatched
  completion was discarded. Fresh replay of the intact fixture reconstructs
  both executions and deduplicates repeated records. This injection simulates
  loss/delayed visibility at the adapter; it does not delay provider disk writes.

These tests characterize rejection. They do **not** demonstrate successful
completeness recovery or detectable loss for an accepted provider source.

| Required acceptance case | Status |
| --- | --- |
| Ordinary/nested execution identities, active-worker message | Existing print captures demonstrate identities and the non-resuming message shape; fresh interactive capture demonstrates nested resumption. No completeness proof. |
| Missing PreToolUse / PostToolUse | Both deliberately lost together in the interactive capture; provider continued. Independent single-loss permutations remain untested. |
| Delayed persistence / replay | Synthetic result withholding and full replay tested; no live provider persistence watermark or cursor found. |
| Observer disconnection | Reporter-delivery loss simulated; real transport disconnection and provider replay not validated. |
| Helper termination / provider restart / initial attachment | Not validated. No candidate supported boundary was found to test. |
| Complete ordinary raw-plus-correlated resumption becomes Idle | Still failing as a product requirement; existing Busy characterization retained. |

The plan's prerequisite therefore fails. Stop dependent runtime work: no epoch
implementation, normalized production ledger, activity relaxation, status schema
change or recovery protocol is justified by this evidence. Do not enable the
previous blocking design as a fallback. Issue 17 stays open and is not ready to
merge. Reopening the gate requires the missing provider capability and the full
interactive failure/recovery matrix, followed by implementation and fresh review.

Verification results are recorded in
[`AGENT_CLI_FINDINGS.md`](AGENT_CLI_FINDINGS.md). No rebase, full CI, local merge,
push, installation or deployment follows a rejected contract.
