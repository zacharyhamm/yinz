# Lifecycle recovery

Read this reference only when `drive` reports `blocked`; use the matching section
below. Ordinary operation and explicit restart/rollback routing are in the
[skill](../SKILL.md). Persistence and protocol facts are in the
[design reference](../../../../docs/ISSUE_LIFECYCLE.md); states that predate
current contracts are in [Legacy states](../../../../docs/ISSUE_LIFECYCLE.md#legacy-states).

## Inspect the blocker

A failed or interrupted step is not completion. Read `status`, the exact workflow
run and Git before retrying. Use Swamp workflow history and data commands, not
edits to internal `.swamp/` files; check `swamp help workflow history` and
`swamp help workflow resume` for the installed schema. `state` is the current
resource and events retain history. Preserve the worktree for diagnosis.

Blocker reasons are free text. If none of the sections below fits, inspect the
recorded assignment, saved response, child state, requirements, configuration and
exact run before selecting an action. Never manufacture evidence or reset history
to advance. A workspace mismatch requires inspecting the control vault setting;
changing it does not migrate an issue. Restore the issue's original workspace to
resume it, rather than recreating it.

Changed Agenda requirements are refreshed by `drive` itself before any approval.
After plan approval, drive blocks with the handoff's `requirementsChange`: the
changes and the approvals a refresh would invalidate. Present both; with the
user's direction run `refresh` (drive continues and regenerates the affected
plan/review evidence), or restore the previous Agenda text. While a revision
authorization is reserved, refresh refuses: with the user's direction roll back
to `triage`, which records that request as planning feedback. Reconcile
already-applied transitions first as described below.

## Unresolved tasks

Start with `status`.`taskRecovery`: it classifies the current task from durable
records and names the command (see
[completion diagnostics](#completion-and-stale-run-diagnostics)). An unresolved
pane task blocks ordinary mutations and replacement workers. `drive` resumes the
original task and submission, including partially applied results;
`resume-agent` provides lower-level observation. A timeout does not authorize a
restart or a new prompt. Do not rerun `agent` as an automatic retry.

Drive retries these correlated failures itself, within a budget per drive
invocation. Every retry re-observes the original task: it never resends a
prompt, never spawns after a launch intent and never answers a dialog.

| Class | Applies when | Budget |
| --- | --- | --- |
| `long_wait` | An ordinary wait timeout of the original, acknowledged busy target: phase `spawned`, `waiting`, `background`, or `submitting` with the same submission's started delivery; not-ready reason absent, `not_idle`, `unresolved_turn` or `unconsumed_input`. | 48 read-only waits of the task's `timeoutSeconds` |
| `reobserve` | The child completed and its evidence is accepted (completion unreconciled), or a fresh recorded transport error after submission. | 2 attempts, after 1 s and 5 s |
| `cleanup` | Close-completed refused by a transient guard on the same worker, conversation and submission: busy, background work, an unresolved accepted turn, or an in-flight prompt write or control operation. | 6 attempts, after 2 s to 60 s (about 127 s) |
| `preflight` | A launch-preflight transport failure before any launch intent was saved. | 3 attempts, after 5 s, 15 s and 30 s |

All classes share an 8-hour deadline per drive invocation. Budgets are never
persisted: when drive reports a budget exhausted, rerunning drive is an explicit
act that starts a new one. These must stay blocked for a deliberate decision:
uncertain delivery (an unacknowledged interruption, or a timeout without a
correlated busy observation), the submission's own `interrupted` outcome,
`failed` provider or host failures, missing typed evidence, `not_sent`,
`target_changed`/`target_closed`, `needs_input` dialogs, `status_unavailable`,
`hooks_unavailable`, identity mismatches, uncertain or rejected launches, runner
exceptions with no recorded reply, pending discards, an undelivered continuation
and cleanup refusals of identity or correlation.

Pane input during an accepted turn ends only the prompt's own observation as
`interrupted`; the child then keeps observing the same submission, which input
cannot end. Only that submission's own `interrupted` result is terminal. Then
neither `continue-agent` (it needs a completed worker) nor `discard-agent` (it
needs `failed` or `not_sent`) applies: only `restart-agent` or `rollback` with the
user's request does.

`needs_input`, interruptions, target replacement and timeouts are not successful
evidence. Unknown or expired submissions without a validated saved terminal proof
remain unresolved; inspect the retained pane and state before any explicitly
requested rollback or restart. Background
tasks require another observation of the same conversation's final idle answer
before success. A closed Claude pane can still complete through a retained proof
that drive observes automatically; see [design §Qualified completion after
closure](../../../../docs/ISSUE_LIFECYCLE.md#qualified-completion-after-closure).
Completed handoffs persist across resume; changed state or assignments,
including rollback/restart, invalidate superseded handoffs.

If dispatch stopped before the child saved initial state, `resume-agent`
initializes that original task under the child execution lock using its saved
configuration and prompt. Existing child state is validated and resumed. Pending
tasks keep their original directory, placement and provider settings.

## Dialogs hooks and sessions

Address startup dialogs deliberately in the visible pane using the local Yinz
agents skill. A timeout does not authorize answering them automatically. Missing
or stale Codex, Muse or AGY hooks fail preflight; inspect `yinz hooks status
--agent KIND`. Hooks are not installed automatically.

Missing sessions, failed control-pane lookup, provider failures and session
mismatches block new launches. Inspect the session and saved configuration. New
workers require control identity and the matching session, with no automatic-tab
fallback. Existing workers resume their original submissions and tabs. After
deliberately resolving a dialog or other blocker, `drive` again.

## Uncertain delivery

An uncertain spawn is never automatically retried. Inspect the retained session,
pane and saved response. Malformed or mismatched evidence, transport failures,
unknown submissions, uncertain delivery, retained resources and active workers
cannot be discarded as rejected launches. Explicit rollback or restart may
supersede their assignments without proving a terminal outcome. Text alone
cannot establish non-delivery; the structured `prompt_delivery` protocol is in
[design §Pane tasks](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks).

## Rejected launches

`discard-agent --request '<the operator request>'` verifies the original task
identity, lifecycle state still equal to the task's original snapshot, and a
durable `launch_rejected` whose launch receipt proves non-execution or process
exit with completed cleanup (or a saved structured rejection retaining no pane,
submission, background work or other resources). Malformed responses, transport
failures, responses retaining resources and uncertain launches remain blocked
for discard.

After explicitly discarding a resource-free rejection, correct configuration
(for example unsupported effort) in a new `agent` request; nothing relaunches
automatically. The old configuration stays recorded; neither resume nor discard
changes it or launches a replacement. Standalone `@yinz/agent` callers need a new
behavior identity with corrected inputs. Explicit rollback or restart remains
available for uncertain launches.

## Unusable evidence and stopped failures

Inspect the response and worktree before assigning replacement work.
`discard-agent` with the actual operator request checks the matching original
task identity and unchanged lifecycle snapshot, then verifies its basis itself:
it re-waits a terminal failure's original submission and requires fresh idle
status, and freshly observes a completed worker's pane listed and idle. The
[discard bases](../../../../docs/ISSUE_LIFECYCLE.md#discard-bases) table lists
what each basis needs. A refusal names every failed condition. Add
`confirmedStopped: true` only when the behavior says it cannot observe a
completed worker and you have confirmed the stop yourself. A pane absent from
its session is unobservable, because it may have warped to another session or
outlived its server; look for it in every session before attesting.

For prompt preparation, `clearing_draft` and `pending_submission` block discard
until writes drain. An empty composer is not required: preparation may leave a
marker or draft. Inspect the composer before reuse. Missing structured delivery
evidence cannot be replaced by explanatory text.

Discard records the reason on the task and preserves correlated failure separately
from fresh status. Lifecycle state is unchanged. It cannot stop active work,
undo source changes, resend, respawn, close a pane or mark an issue successful.
Inspect changes before replacement work; a replacement requires a separate
explicit request. Failed, blocked and unusable-result panes remain visible.

## Interrupted imports and cleanup

If a state-changing transition was saved but the final task-completion write
failed, `resume-agent` validates the completed result's identity and reconciles
the task without repeating the transition or event write. This works even if
Agenda requirements changed afterward: reconcile first, then `refresh`.
Unapplied evidence still needs fresh requirements. Implementation reports leave
lifecycle state unchanged, so their recovery checks requirements and retries the
evidence write.

Accepted evidence and task completion are durable before the worker is closed
with a conversation- and submission-guarded command. Cleanup is independently
retryable through `drive` or `resume-agent` without replaying accepted evidence.
Cleanup discards a worker's draft or unconsumed input; `unconsumed_input` never
blocks it and needs no recovery or terminal verification. A rejection names the
failed guard (changed conversation, not idle, another latest submission,
background work or an in-flight control operation). `taskRecovery` kind
`retry_cleanup` shows the saved refusal and whether it is retryable: drive
retries transient guards on the same worker within the `cleanup` budget; resolve
any other cause deliberately. A retried `worker is Busy, not idle` can close the
pane after a user typed into a completed worker; the provider conversation
survives. Failed, blocked, superseded and unusable-result panes stay available
for inspection. Closing the last worker releases the region.

## Rollback and restart

An explicit user request authorizes superseding any prior assignment, including
running workers, missing sessions, expired submissions, uncertain launches and
malformed evidence. Record the actual request. Do not demand successful resume,
proof of non-delivery, a terminal provider result, `confirmedStopped` or another
permission question. Ordinary timeouts do not authorize automatic restart.

`rollback` takes `step` and the user's `--request`. Steps are `triage`, `plan`, `review_plan`,
`approve`, `implement`, `review_code`, `review_ux`, `ready`, `merge` and `summarize`. It retains
preceding evidence and invalidates decisions from the selected step onward;
prerequisites still apply (for example, `review_plan` needs a plan). Changed
requirements require rollback to `triage`. Completed Agenda tasks are reopened
with descriptions preserved, and plan versions keep increasing.

Rollback archives original state, assignments, failures and partially applied
evidence. A new rollback ID fences old results even at the same phase. It
preserves Git changes, worktrees, branches and ownership: it does not reset Git,
abort a rebase, undo a landed merge or close old panes. Inspect and stop surviving
workers before a replacement edits the same checkout. Superseding an assignment
does not establish that its process stopped.

Before replacing a merge attempt, rollback consults its durable Git receipt under
the repository merge lock. A landed merge stays in history; `merge` and
`summarize` retries reuse its receipt. Returning earlier starts a new sequence of
review and authorization for subsequent changes. Conflicts and rebased HEADs
still require repair in the retained worktree.

After rollback, run `drive` to the next handoff (or the ordinary command for the
selected step). For worker steps, `restart-agent` combines rollback with a new
assignment using the current `taskId`, role, actual `request` and ordinary agent
options. Select the intended running session; the old one may be gone. Omitted
optional settings use provider defaults. The replacement has new task, behavior,
pane and submission identities. Identical restart inputs resume that replacement;
another restart must target its new task ID with a new request.

Recovery intent is saved before Agenda, state or assignment changes. If
interrupted, `drive` finishes that intent before anything else and resumes its
exact saved replacement, if any, without another approval. `resume-rollback
--no-drive` finishes the intent and stops before `drive`; a plain
`resume-rollback` continues with `drive` like the other chained commands. Either
command still resumes a saved restart replacement in the same checkout, so
inspect or stop surviving workers before running either. A failure reports
`Rollback is incomplete`; rerunning drive retries the saved intent. Ordinary
mutations wait for pending recovery. A new explicit rollback may supersede it without finishing
it first, preserving the old intent in history. This also supports changed
requirements or a different restart step. `status` includes the latest recovery;
`recovery-<id>` and `agent-task-<id>` retain recovery and superseded assignment
records.

## Review and verification failures

Plan blockers use the skill's `authorize_plan_revision` route: supported
adjudication or separately authorized targeted revision, never implied approval.
Technical and UX findings yield `address_review_findings` after both reviews.
Present findings and ask which to repair or ignore; any severity may be accepted
by the user. Use `decide-review --handoff <id> --request '<direction>'` with
`--repair`/`--ignore` role-qualified finding references; the behavior binds the
exact review fingerprint from that handoff. Selected repairs run a
planner and yield `approve_review_fixes`; approval authorizes the implementation
worker. Repairs require fresh technical and UX reviews and verification. The
user may sign off on merge with outstanding findings; preserve them as unresolved.
Resume interrupted repair assignments using their original submissions and saved
approval. Explicit `restart-agent` for either repair role retains selected scope
and any approved repair plan; ordinary rollback invalidates finding decisions. A failed, unaccepted or stale
UX assignment never counts as approval; resume its original saved submission. Existing
ad-hoc code still needs a real plan, review and verification; never fabricate
evidence. Stale review cannot cover a new plan or repaired implementation.

Rollback to `review_ux` retains matching technical review and implementation head,
but clears UX, verification, merge authorization and attempt eligibility. Rollback
to `review_code` or earlier clears both reviews. Ready-only rollback retains both
reviews and same-commit verification. A new technical review clears prior UX.

For a failed verification, read the checklist and each failing check's log.
`repair` needs no input: the recorded failed probe, baseline and plan checks and
their log names become `lastFailure` for the implementation worker, which reads
the logs itself; add a reason only for context the logs lack. A rebase-probe
conflict tells the worker to rebase onto local `main` and resolve each conflict
deliberately. Baseline checks run at the un-rebased commit, so a baseline
failure in code the branch did not change may come from its older base; the
worker then rebases onto local `main` when `main` already fixes it. The fixed commit needs fresh technical review, UX review and
verification. `drive` never automatically reruns failed verification or runs
repair. Only an environment failure at the same reviewed commit permits an
explicit `verify` rerun. A dirty worktree or changed branch/HEAD must be inspected
and repaired or restored to the reviewed commit before same-commit verification.
Every run stays in event history. An empty checklist or cancelled run is not
successful verification. Reviewer-reported checks remain advisory for structured
plans.

Treat flakes and timeouts as defects: record, investigate and fix them in this
branch. A passing diagnostic rerun alone does not resolve failure. When `/tmp`
space is insufficient, managed tests support `YINZ_TEST_TMPDIR` in a short private
directory on a larger filesystem; set `TMPDIR` there for other fixtures. Keep it
outside Git worktrees so fixtures do not inherit repository identity.

## Integration failures

Waiting for the repository lock is not a FIFO queue. Let the current integration
finish; never remove another process's lock or move `main` manually while it
runs. Per-issue locking does not replace the shared integration boundary.

On merge-gate failure, show the attempt summary, failed phases/tests and log path
from launcher output or the blocked handoff. Read
`<git-common-dir>/yinz-lifecycle/merges/<attemptId>/ci-check.log` and investigate
the cause. `repair` attaches the recorded summary and log to `lastFailure` for the
next implementation worker; a reason adds only what the log lacks. A test that
also failed in earlier attempts is named as recurring in the summary; a flake is
a defect. Fix the cause in this branch and obtain current review/verification
before retrying. Earlier attempts remain in `mergeFailures` and the next
receipt's `previousAttempts`; retry must not hide them.

Verification's rebase probe reports conflicts with local `main` before the merge
gate; the gate's own rebase under the lock stays authoritative. For conflicts,
dirty worktrees, changed targets or interrupted integration, inspect Git and the
exact persisted run to determine whether rebase, verification or merge completed.
Resolve conflicts deliberately in the retained worktree, record the repair and
refresh review evidence before integration. Never silently abort rebase,
reset/discard changes, delete branches or force a merge to make a retry appear
clean. Failed worktrees stay available for diagnosis.

The full merge gate still applies after repair. Only the user's explicit waiver
for that merge, recorded with its scope in `skipCiRequest`, permits skipping it;
earlier checks, time pressure or agent judgment do not. Skipped is never passed.

## Close-out failures

If local merge landed but Agenda close-out failed, verify the recorded merge and
resume close-out using its receipt rather than merging again. Compare delivered
outcome to the original requirements. Complete only after merge and
`outcomeMet: true`; keep an unmet issue open and describe the remaining work.
After successful completion, `authorize_closeout` offers optional bundled cleanup.
Present the directory (including build outputs), local branch and original
controller session/pane. Acceptance ends that conversation; record separate
explicit consent with `closeout --handoff <id> --decision accept --request '<consent>'`.
Initial decline retains everything and is terminal for automatic drive. A renewed
offer describes only remaining actions. If Git is complete, decline keeps the
original pane and completed outcome; deleted resources remain deleted. Declining
partial cleanup stops further effects, retaining remaining resources, saved
progress and any unresolved ownership reservation without claiming completion.

On a Git failure the pane stays open. Read the saved `closeout.failure`, repair
its cause, then run `resume-closeout`. The Swamp behavior reconciles its exact
helper receipt and completes worktree removal before branch deletion. It never
forces dirty/locked/moved/reused resources. Do not manually recycle a reserved
ownership entry or infer absence from a parent checkout.

Closeout uses read-only `closeout-receipt` validation without acquiring the
repository merge lock or reconciling receipts. An unreconciled receipt needs
normal integration recovery before cleanup. Standalone deletion of an
already-merged worktree and branch can proceed during another issue's merge/CI;
merge/rebase/CI serialization remains unchanged.

Optional preview and cleanup return promptly when the brief worktree ownership
guard is busy. The diagnostic names the stage and lock, for example
`Optional closeout deferred during cleanup preview: worktree ownership lock is busy`.
Before an offer exists, this creates no consent or cleanup authority. After an
accepted partial operation, `Git cleanup` contention preserves the original UUID,
recorded deletions, uncertain progress and reservation, and leaves the pane open;
it does not mean all resources remain or that cleanup finished. Inspect the
ownership operation, let it finish, then explicitly retry `closeout` or
`resume-closeout`. This brief guard contention is independent of merge CI. Do not
cancel another issue's merge gate, remove a lock file or use manual deletion to
bypass the guard. Retry retains all target checks and expired-fence consent
requirements.

If self-close lost its acknowledgement, recover in another pane with
`resume-closeout`; it observes the frozen original session/pane, never the
recovery caller. Same-incarnation exact absence can be reconciled; a new server,
conversation, input or turn cannot authorize closing a replacement. Unknown
observations remain unconfirmed. An expired fence yields a fresh offer ID: present
it and obtain fresh explicit acceptance or decline for the still-pending original
pane. Decline is terminal for drive and resume, with no repeated offer. Successful
Git deletion and the completed issue remain recorded.

Legacy/missing origins and older servers retain resources. Use the explicit
exact-target `bind` procedure in the [lifecycle guide](../../../../docs/ISSUE_LIFECYCLE.md#optional-resource-close-out)
to create an offer before consent; never choose the latest worker or invoking
pane. Snapshot refusals preserve the server reason, such as `Controller closeout
would affect other panes`; resolve that condition before retrying the offer.
Accepted cleanup facts survive rollback/refresh while current consent is revoked.
After rollback to summarize, record the corrected outcome again. With the same
requirements, merge attempt and targets, recovery keeps the cleanup UUID and
partial/uncertain helper progress rather than previewing the removed directory.
Present its new offer and obtain fresh consent before any remaining effects.
Same-incarnation exact absence can reconcile an unknown pane close; confirmed
closure remains a fact. Changed requirements/merge identity/targets block reuse
without discarding historical effects. Before restarting
implementation after cleanup, deliberately select a new valid worktree.

## Completion and stale-run diagnostics

Read `status` for `workerObservation`, `diagnostics` and `taskRecovery`.
`taskRecovery` is computed from durable records and is advisory: the method it
names re-verifies every condition. It reports `kind`, `taskId`, `role`,
`summary`, `command` (`{method, input}` or null), `requiresUserRequest`,
`alternatives`, `discard` (`eligible`, `basis` and the condition lists for
rejected launch, terminal failure and completed worker), and `lastError`. Request
inputs show the placeholder `<the user's actual request>`; substitute the user's
actual words, never invented ones. Kinds:

| Kind | Meaning |
| --- | --- |
| `none` | No pane task needs recovery; follow the current handoff. |
| `finish_rollback` | A rollback is pending; `drive` finishes its saved intent. The launcher's `resume-rollback --no-drive` finishes it and stops before `drive`, but still resumes a saved restart replacement; inspect surviving workers before either. |
| `finish_discard` | A recorded discard is unfinished; `drive` completes it first. |
| `retry_cleanup` | The result is imported but cleanup was refused; drive retries a transient guard, otherwise inspect. |
| `resume` | A bounded retry class applies, or background work has no recorded failure yet; `drive` re-observes without resending. A recorded background failure other than a correlated timeout or transport error gets the discard, `interrupted`, `dialog` or `inspect` kind instead. |
| `discard` | A rejected launch or terminal failure is eligible; `discard-agent` with the user's request. |
| `continue_or_discard` | A completed worker has no accepted evidence; `continue-agent` (unless cleaned up) or `discard-agent`, each with the user's request. |
| `continuation_unsent` | A saved continuation never reached the worker; rerun `continue-agent` with the same request. |
| `interrupted` | `drive` observes the submission; its own `interrupted` leaves only `restart-agent` or `rollback`. |
| `dialog` | Resolve the dialog deliberately in the pane, then `drive`. |
| `inspect` | No automatic or discard route applies; inspect, then restart or roll back only with the user's request. |

An accepted receipt with `completion_unreconciled` means the child reports
completion: `drive` (or `resume-agent`) validates and imports the original result
without invoking the worker again. `evidence_waiting_for_completion` and
`background_work` retain the completion fence; continue observing the original
submission. Neither transcript EOF nor pane closure proves termination. A
qualified retained submission can supply the proof after closure; an expired or
unavailable record cannot.

`worker_identity_mismatch` or `child_state_missing` requires inspection of the
recorded assignment and original child. Never substitute a replacement
conversation or resend the prompt. A controller interrupted after evidence or
completion persistence resumes the same task; duplicate recovery preserves the
existing import/event and cleanup guards.

`workflow_running_after_issue_done` names the exact historical run whose status
conflicts with lifecycle completion. Inspect that run with the command in its
`action` and inspect controller liveness separately. Do not infer a live or dead
controller from workflow status or elapsed time. No historical run is rewritten
by this diagnostic. `workflow_history_unavailable` means status could not inspect
history; it makes no claim about run liveness.
