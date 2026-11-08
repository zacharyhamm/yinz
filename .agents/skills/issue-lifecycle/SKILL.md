---
name: issue-lifecycle
description: >
    Track Yinz development from an Agenda issue through a reviewed plan,
    local worktree implementation, verification, local merge, and completion
    using Swamp. Use to start or resume an issue, review or revise its plan,
    adopt existing work, or merge an issue through this lifecycle.
---

# Yinz issue lifecycle

Use Agenda for requirements, Swamp for durable state/evidence, and local Git
worktrees for development. `@yinz/issue-lifecycle` uses visible Yinz pane workers through
`@yinz/agent`; direct evidence methods never replace workers with headless or
inline work. Operator actions belong to the controlling workflow. PR creation,
fetch/push, remote merge, deployment and notifications need separate instructions.

## Identity and setup

Use one canonical control checkout and its `scripts/issue-lifecycle` launcher for
every operation (`--control` when using another copy); never split stores across
worktrees. Commands below take `--item <returned-id>` and JSON via `--input-file`.
Use installed `swamp`/`agenda`, help for unfamiliar flags, JSON for machine reads,
issue-scoped input files and arguments rather than issue text interpolated in shell.

The control vault `default/AGENDA_WORKSPACE` identifies the existing workspace,
never Agenda's current selection. `--workspace` asserts equality, not an override.
Read workspace identity/item before changing it; let the launcher compute names.
Consult [setup reference](../../../docs/ISSUE_LIFECYCLE.md#one-control-checkout)
for missing configuration, prerequisites, claims and legacy behavior.

Inspect `status` before start/resume; never recreate the behavior or reset history.
Trust persisted evidence, not prior conversation. Agenda title/note are requirements: if changed, `refresh`
and review them. Create a feature worktree if needed, then `start` there (or in a
subdirectory). It claims canonical `workerWorktree`, refusing `main`, detached
HEAD and other issues' claims. All new workers retain that directory across roles,
revisions/recovery and later invocations; repeated start does not replace it.
Pending tasks keep configuration; legacy states retain implementation/control
selection without inferring a new origin.

## Drive and resume

After start, `drive` accepts
`{agent: {session, kind, model?, effort?, permissionMode?, startupTimeoutSeconds?, timeoutSeconds?}}`.
<<<<<<< HEAD
Select the intended running session/provider; omitted options preserve defaults.
Settings persist; ordinary resume is `drive` with no input. New workers resolve the
control pane's current tab and join its separate worker stack; existing workers
keep tabs/submissions. Sessions must match. Outside the pane environment use saved
identity or `control: {session, paneId}` alongside `agent` (role options for
`agent`/`restart-agent`). Older drivers need identity before new launches; no
automatic-tab fallback is allowed.
=======
Use the intended running Yinz session and provider. Omitted options preserve
provider defaults. Settings persist; ordinary resume uses `drive` with no input.
The launcher captures the invoking control pane's session and ID. Every new
worker resolves that pane's current tab and joins its separate controller-owned worker stack there, including
revisions, repairs and restarts. Existing workers stay in their original tabs
and resume their original submissions. The selected worker session must match
the control pane's session; missing panes and lookup failures block launches.
Outside the pane environment, supply `control: {session, paneId}` explicitly
(alongside `agent` for `drive`, or alongside role options for `agent` and
`restart-agent`), or resume with saved identity. Older saved driver settings
must acquire control identity before launching another worker; an unlaunched
legacy task requires an explicit restart. Never bypass placement with automatic
tab selection.
After approval and worktree preparation, use `drive` again. After an explicitly
requested restart or completed rollback recovery, continue through `drive` to
the next handoff. The driver supplies prompts and uses visible pane workers;
never replace them with headless provider execution or inline agent work.
Single-role `agent` and `resume-agent` remain lower-level diagnostic operations.
Use the local Yinz agents skill for deliberate pane inspection and dialogs. New workers require `agent-launch-transaction-v2` and persist an incarnation-qualified launch ID before spawn. Resume observes that exact ID, never a replacement spawn. Definitive terminal receipts can support deliberate discard even when Git paths remain; pending, unknown, or legacy ID-less launches require inspection. A receipt alone never supplies worker evidence or changes the original approval/task/prompt correlation.
>>>>>>> bab9477ac (Make agent launches transactional with durable receipts and placement safeguards)

Wait on the same foreground command through yields until its structured handoff,
then present the readable result before ending the controlling turn: issue,
plan version/commit, decision and evidence (plan behavior, steps, tests,
assumptions/review, or delivered change, review and verification checklist).
Pane/workflow IDs and progress are not completion. Explain blocker cause/recovery;
do not promise a later controller wakeup.

Pending tasks, saved assignments, partial imports and cleanup resume **before
phase dispatch**. `drive` observes ordinary acknowledged timeouts on the original
submission without resending. Pending rollback needs `resume-rollback` or a new
explicit rollback. Completed handoffs persist until superseded.
`agent`/`resume-agent` are diagnostics; use the local Yinz agents skill for pane
inspection/dialogs.

| Phase | Next action after pending work is resolved |
| --- | --- |
| `triaging` | `drive` triages source, problem, scope, acceptance criteria and reproduction evidence. |
| `classified` | `drive` plans steps/files/risks, focused tests and verbatim argv `checks` covering steps, then runs adversarial review. Present checks: plan approval approves running them. |
| `planning` | Missing/current-version-mismatched review: review. Blocking current review: one automatic revision/review if available, otherwise `authorize_plan_revision`. Clean review: `approve_plan`. Missing plan blocks. |
| `approved` | `prepare_worktree`: adopt recorded worktree/existing feature branch; conflicting paths are rejected before Git preparation. |
| `implementing` | `drive` implements/commits with focused tests, then reviews; completed implementation or rollback to `review_code` routes to review. Clean current review runs checks at that commit. Stale evidence, code blockers or failed verification block; passing gate yields `authorize_merge`. |
| `ready` | Active merge failure: `blocked`, read log and repair. Otherwise `integrate`. |
| `merged` | `summarize` compares outcome to original requirements using landed receipt. |
| `done` | `complete`: report recorded outcome without repeating work/merge. |

| Handoff decision | Controller action |
| --- | --- |
| `authorize_plan_revision` | Supported adjudication or authorization for one targeted revision, below. |
| `approve_plan` | Present reviewed plan; `approve {planVersion, request}` records explicit approval. Existing session authorization within scope suffices; ask only if absent. |
| `prepare_worktree` | `implement {worktree, branch}`, then `drive`. |
| `authorize_merge` | Show change, review and verification; `ready {commit, mergeRequest}` records explicit merge authorization, including an existing explicit request. Plan approval does not authorize merge. |
| `integrate` | `merge` from control checkout serializes rebase onto current local `main`, full `cargo x ci check` and local fast-forward merge. |
| `summarize` | `summarize {outcomeMet: true, summary}` only after merge and met outcome; otherwise keep Agenda open and explain remaining work. |
| `complete` | Present completed result. |
| `blocked` | Read matching recovery section below, resolve deliberately, then `drive`. |

## Review and authorization

One automatic targeted revision/review is reserved with its exact assignment before
launch per requirements generation. Resume, rollback, restart, manual planning and
reinvocation never replenish it; only refresh with genuinely changed requirements
does. Legacy first-version plans may receive one; later versions count as consumed.
Saved revision authorizations remain valid.

At `authorize_plan_revision`, inspect cited source and explain judgment.
`adjudicate-plan` takes current `planVersion`, handoff `reviewFingerprint` and
`dispositions`: `findingId`, `disposition` (`resolved`, `rejected`, `downgraded`),
`rationale`, nonempty source `evidence`, plus corrected lower `severity` for downgrade.
Supported dispositions need no extra permission. Preserve original review; stale
inputs/mutations during unresolved assignments are rejected. Accepted unresolved
high/critical risk stays blocked for user direction. Then `drive` again.

For another revision, present the concrete handoff and obtain authorization for
exactly one targeted revision. `drive {revisionApproval: {handoffId, request}}` uses
that ID and actual user request; the complete assignment is saved before dispatch,
reused on resume and freshly reviewed. Never infer blanket approval or ask again
for that assignment. Revision, implementation, merge and CI-waiver authorizations
are separate; the driver grants none itself.

Planners/reviewers read [planning](references/planning.md) and
[review](references/review.md); plan/code reviewers also read applicable
[Yinz review dimensions](references/dimensions.md). Preserve unaffected steps,
accepted tradeoffs and finding IDs. Reopening disposed concerns needs new evidence
explaining why the disposition no longer holds. History never replaces current
review; repairs need fresh review. Ad-hoc code still needs real planning/review/
verification. Never fabricate evidence. Keep human descriptions in Agenda;
launcher transitions retain evidence and history in Swamp.

Keep each worktree's real `target/` isolated: no `CARGO_TARGET_DIR`, changed
`build.target-dir` or shared/copied mutable build artifacts. Fix defects/flakes in
this branch through the worker. After clean review, lifecycle checks run at the
exact commit outside the merge lock, recording exit codes/durations/logs; reviewer
checks are advisory. `ready` needs passing exact-commit verification. Legacy plans
without checks explicitly retain the reviewer-reported gate. Failed verification
is not automatically rerun. Full merge CI can be skipped only for the user's
explicit waiver for that merge, recorded with scope in `skipCiRequest`; previous
checks, time pressure and agent judgment do not suffice. The shared repository
lock covers rebase/checks/merge; per-issue locks do not replace it, nor is it FIFO.

## Blocker routing

Read `references/recovery.md` only when the driver reports `blocked`:

| Blocker family | Recovery section |
| --- | --- |
| Unresolved task, timeout, unknown/expired submission, background work | [Unresolved tasks](references/recovery.md#unresolved-tasks) |
| Dialog, hooks, session/provider/control failure | [Dialogs hooks and sessions](references/recovery.md#dialogs-hooks-and-sessions) |
| Malformed response, transport failure, uncertain delivery | [Uncertain delivery](references/recovery.md#uncertain-delivery) |
| Resource-free structured `launch_rejected` | [Rejected launches](references/recovery.md#rejected-launches) |
| Unusable completed evidence, stopped failure, structured `not_sent` | [Unusable evidence and stopped failures](references/recovery.md#unusable-evidence-and-stopped-failures) |
| Partial import, pane cleanup | [Interrupted imports and cleanup](references/recovery.md#interrupted-imports-and-cleanup) |
| Interrupted rollback/restart | [Rollback and restart](references/recovery.md#rollback-and-restart) |
| Review blocker, stale evidence, failed verification | [Review and verification failures](references/recovery.md#review-and-verification-failures) |
| Conflict, failed CI, interrupted integration | [Integration failures](references/recovery.md#integration-failures) |
| Failed close-out after landing, unmet outcome | [Close-out failures](references/recovery.md#close-out-failures) |
| Other free-text reason, requirements/configuration change | [Inspect the blocker](references/recovery.md#inspect-the-blocker) |

## Explicit rollback or restart

The actual user request authorizes superseding any prior outcome: no successful
resume, non-delivery proof, `confirmedStopped` or renewed approval is required.
`rollback {step, request}` retains prerequisites and invalidates downstream
decisions. Steps: `triage`, `plan`, `review_plan`, `approve`, `implement`,
`review_code`, `ready`, `merge`, `summarize`; changed requirements require `triage`.
`restart-agent` uses current `taskId`, actual `request` and ordinary agent inputs
with the intended running session. Identical inputs resume the replacement;
another restart targets its new task ID. Inspect/stop surviving workers before
replacements edit the same checkout. After restart/completed `resume-rollback`,
`drive` to the next handoff; saved intent needs no renewed approval, and a new
explicit rollback may supersede pending recovery. History, Git changes, ownership
and increasing versions survive; completed Agenda tasks reopen. Landed merge and
close-out retries reuse receipts. [Design reference](../../../docs/ISSUE_LIFECYCLE.md)
holds persistence and input contracts.

<<<<<<< HEAD
A worker is complete only after its role evidence is validated, recorded and
acknowledged through its assigned `@yinz/issue-evidence` behavior. Final prose is
presentation and is never parsed as lifecycle evidence. Workers may invoke only
the supplied submission method and receipt query; approvals, rollback, merge and
Agenda mutations remain controller operations. Validation errors are corrected
within the same worker. Lost acknowledgements are recovered by inspecting the
original task's durable submission; identical replay preserves its receipt.
=======
If a worker completed but returned unusable evidence, `discard-agent` requires
the operator's actual `request` and `confirmedStopped: true`. It accepts a
matching completed worker with no background tasks and unapplied evidence, or a
durable `launch_rejected` task with a matching version-two launch receipt proving
non-execution or confirmed process exit and completed pane/PTY cleanup. Retained
Git paths do not imply a live provider. Legacy resource-free structured rejections
remain readable. No submission or background work may remain, and lifecycle state
must still match the task's original snapshot. After discarding a rejected launch, correct the
configuration in a new `agent` request. No configuration change or relaunch is
automatic. Malformed responses, transport failures and legacy uncertain launches
remain blocked for discard; an explicit rollback or restart can supersede them.
Discard cannot end active work or undo source changes; inspect any changes before
assigning replacement work.
>>>>>>> bab9477ac (Make agent launches transactional with durable receipts and placement safeguards)

If a provider ends without accepted evidence, keep the task unresolved. Use
`continue-agent` with `{taskId, request}` to ask that same retained conversation
to submit its existing findings. The guarded continuation is saved before delivery
and resumes by its original ID. Do not repeat the investigation. Legacy pending
tasks use this supplement without changing their original prompt or configuration;
already-applied state remains historical evidence. Missing original context blocks
supplementation and never authorizes a replacement review.

Explicit `discard-agent` still requires the operator's actual `request` and
`confirmedStopped: true` and verifies terminality or resource-free launch rejection.
It saves discard intent and fences submissions before marking the task discarded.
Rollback and restart likewise fence the old submission before changing parent
state. Old evidence remains historical; no worker can submit for a replacement
assignment. Accepted evidence is persisted before cleanup and import recovery
never reparses chat or repeats the worker.

Submission has its own short Swamp behavior lock and must never acquire the
issue/child lock held by the waiting controller. Direct submission currently
requires a healthy local format-2 filesystem datastore; preflight rejects
command-wide writer-lease backends before launching a worker. Keep canonical
store access within provider permissions and never create a per-worktree store.

