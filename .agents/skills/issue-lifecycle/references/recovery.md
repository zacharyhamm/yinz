# Lifecycle recovery

Read this reference only when `drive` reports `blocked`; use the matching section
below. Ordinary operation and explicit restart/rollback routing are in the
[skill](../SKILL.md). Persistence and protocol facts are in the
[design reference](../../../../docs/ISSUE_LIFECYCLE.md).

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
resume it, rather than recreating it. Changed Agenda requirements require
`refresh`, assessment of the changes and regenerated affected plan/review evidence;
reconcile already-applied transitions first as described below.

## Unresolved tasks

An unresolved pane task blocks ordinary mutations and replacement workers.
`drive` resumes the original task and submission, including partially applied
results; `resume-agent` provides lower-level observation. Ordinary correlated,
acknowledged wait timeouts keep observing that submission. A timeout does not
authorize a restart or a new prompt. Do not rerun `agent` as an automatic retry.

`needs_input`, interruptions, target replacement and timeouts are not successful
evidence. Unknown or expired submissions remain unresolved until an explicitly
requested rollback or restart; inspect the retained pane and state. Background
tasks require another observation of the same conversation's final idle answer
before success. Completed handoffs persist across resume; changed state or
assignments, including rollback/restart, invalidate superseded handoffs.

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
fallback. An unlaunched legacy task needs an explicit restart with control
identity. Existing workers resume their original submissions and tabs. After
deliberately resolving a dialog or other blocker, `drive` again.

## Uncertain delivery

An uncertain spawn is never automatically retried. Inspect the retained session,
pane and saved response. Malformed or mismatched evidence, transport failures,
unknown submissions, uncertain delivery, retained resources and active workers
cannot be discarded as rejected launches. Explicit rollback or restart may
supersede their assignments without proving a terminal outcome.

`prompt_delivery` is optional: missing means unknown, even if older response text
says the prompt was not sent. Text alone cannot authorize unsent recovery.
`started` is recorded before the first actual prompt write, including a typed
prefix, and is never downgraded even when that write fails. Preparation marker
and clearing writes do not count as prompt delivery. Structured non-delivery
recovery requires updated server responses and behavior code; older tasks without
that evidence use explicit rollback or restart.

## Rejected launches

`discard-agent` requires the operator's actual `request` and
`confirmedStopped: true`, the original task identity, and lifecycle state still
equal to the task's original snapshot. A durable `launch_rejected` is eligible
only with a saved structured `rejected` response retaining no pane, submission,
background work or other resources. Malformed responses, transport failures,
responses retaining resources and legacy uncertain launches remain blocked for
discard.

After explicitly discarding a resource-free rejection, correct configuration
(for example unsupported effort) in a new `agent` request. The old configuration
stays recorded; neither resume nor discard changes it or launches a replacement.
Standalone `@yinz/agent` callers need a new behavior identity with corrected
inputs. Explicit rollback or restart remains available for uncertain launches.

## Unusable evidence and stopped failures

Inspect the response and worktree before assigning replacement work. All discard
cases require the actual operator request, `confirmedStopped: true`, matching
original task identity and unchanged lifecycle snapshot:

| Case | Required evidence |
| --- | --- |
| Completed response fails schema validation or is otherwise unusable | Matching completed worker, no background tasks, evidence not yet applied. |
| Terminal provider failure, including `StopFailure` | Correlated, acknowledged `failed` response; recheck the original submission and freshly observe the same pane and conversation idle with no background tasks. Keep the failure response separate from fresh status. |
| Finalized prompt-preparation failure | Retained server response explicitly says `prompt_delivery: "not_sent"`; retrieve the original submission and match submission ID, pane, Yinz session, provider and conversation. Require failure outcome, no acknowledgement, no turn ID and no background work. Fresh status must show that same worker idle with no background work or pending clearing/submission writes. |

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
Failed, blocked, superseded and unusable-result panes stay available for
inspection. Closing the last worker releases the region. Existing saved tasks
retain legacy placement and cleanup behavior.

## Rollback and restart

An explicit user request authorizes superseding any prior assignment, including
running workers, missing sessions, expired submissions, uncertain launches and
malformed evidence. Record the actual request. Do not demand successful resume,
proof of non-delivery, a terminal provider result, `confirmedStopped` or another
permission question. Ordinary timeouts do not authorize automatic restart.

`rollback` takes `{step, request}`. Steps are `triage`, `plan`, `review_plan`,
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

After rollback, use the ordinary command for the selected step, then `drive` to
the next handoff. For worker steps, `restart-agent` combines rollback with a new
assignment using the current `taskId`, role, actual `request` and ordinary agent
options. Select the intended running session; the old one may be gone. Omitted
optional settings use provider defaults. The replacement has new task, behavior,
pane and submission identities. Identical restart inputs resume that replacement;
another restart must target its new task ID with a new request.

Recovery intent is saved before Agenda, state or assignment changes. If interrupted,
`resume-rollback` finishes that intent and resumes its exact saved replacement, if
any, without another approval. Ordinary mutations wait for pending recovery. A
new explicit rollback may supersede it without finishing it first, preserving the
old intent in history. This also supports changed requirements or a different
restart step. `status` includes the latest recovery; `recovery-<id>` and
`agent-task-<id>` retain recovery and superseded assignment records.

## Review and verification failures

Plan blockers use the skill's `authorize_plan_revision` route: supported
adjudication or separately authorized targeted revision, never implied approval.
Technical and UX findings yield `address_review_findings` after both reviews.
Present findings and ask which to repair or ignore; any severity may be accepted
by the user. Use `decide-review` with the handoff ID, exact review fingerprint,
actual request and role-qualified finding references. Selected repairs run a
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

Old saved assignments, receipts and prompts retain their original contracts.
Legacy unmerged implementing work with technical review proceeds to UX; it is not
synthetic approval. An unmerged ready state missing UX blocks: explicitly
`rollback {step: "review_ux", request: <actual request>}`, then `drive`, checks and
new merge authorization. An unlanded active attempt needs `repair` before fresh
reviews. First consult its durable receipt: an already-landed merge is a fact,
and merge/close-out replay (including rollback to merge/summarize) reuses it
without retroactively inventing UX evidence.

For a failed verification, read the checklist and each failing check's log. Fix
the defect in the current branch, commit and obtain fresh technical review, UX review and verification
of that exact commit. `drive` never automatically reruns failed verification.
Only an environment failure at the same reviewed commit permits an explicit
`verify` rerun. A dirty worktree or changed branch/HEAD must be inspected and
repaired or restored to the reviewed commit before same-commit verification.
Every run stays in event history. An empty checklist or cancelled run is not
successful verification. Reviewer-reported checks remain advisory for structured
plans; legacy plans explicitly retain their earlier gate.

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
`<git-common-dir>/yinz-lifecycle/merges/<attemptId>/ci-check.log`, then pass the
cause to `repair`. `lastFailure` includes the summary and log for the next
implementation worker. Fix the cause, including flakes/timeouts, in this branch
and obtain current review/verification before retrying. Earlier attempts remain
in `mergeFailures` and the next receipt's `previousAttempts`; retry must not hide
them.

For conflicts, dirty worktrees, changed targets or interrupted integration,
inspect Git and the exact persisted run to determine whether rebase, verification
or merge completed. Resolve conflicts deliberately in the retained worktree,
record the repair and refresh review evidence before integration. Never silently
abort rebase, reset/discard changes, delete branches or force a merge to make a
retry appear clean. Failed worktrees stay available for diagnosis.

The full merge gate still applies after repair. Only the user's explicit waiver
for that merge, recorded with its scope in `skipCiRequest`, permits skipping it;
earlier checks, time pressure or agent judgment do not. Skipped is never passed.

## Close-out failures

If local merge landed but Agenda close-out failed, verify the recorded merge and
resume close-out using its receipt rather than merging again. Compare delivered
outcome to the original requirements. Complete only after merge and
`outcomeMet: true`; keep an unmet issue open and describe the remaining work.
Branch/worktree cleanup is separate from completion.
