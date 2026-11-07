---
name: issue-lifecycle
description: >
    Track Yinz development from an Agenda issue through a reviewed plan,
    local worktree implementation, verification, local merge, and completion
    using Swamp. Use to start or resume an issue, review or revise its plan,
    adopt existing work, or merge an issue through this lifecycle.
---

# Yinz issue lifecycle

Use Agenda for issue descriptions, Swamp for durable lifecycle state and
evidence, and local Git worktrees for development and merging. All agent work
runs in visible Yinz panes through the reusable `@yinz/agent` behavior. The
lifecycle implementation is `@yinz/issue-lifecycle`; its entry workflows are
`yinz-issue-start`, `yinz-issue-drive` and `yinz-issue-merge`. Read
[the operating guide](../../../docs/ISSUE_LIFECYCLE.md) for commands, evidence
inputs, the reusable agent extension and recovery.

## Establish identity and resume

Select one canonical control checkout containing the Swamp definitions. Use its
`scripts/issue-lifecycle` launcher for every lifecycle operation, including from
an issue worktree. Pass `--control` when invoking a different copy of the
launcher. Do not initialize separate lifecycle stores in each worktree.

Run `start` from the intended worker Git worktree, or a subdirectory of it;
create the feature worktree first if needed. The lifecycle claims the
worktree for the issue and saves its canonical root as `workerWorktree`. It
refuses the main checkout (a worktree on `main`) and any worktree another issue
already claimed. A claim becomes permanent once `start` saves state, so each
worktree serves one issue; another issue's `start` reclaims a claim whose
`start` never saved state. Every new worker uses that directory across roles,
revisions, later drive/resume, explicit restarts and rollback recovery. Later
launcher invocations from other checkouts and repeated `start` do not replace
it. Pending tasks retain their saved configuration. Swamp storage remains in
the control checkout. Legacy states without this field retain their existing
implementation-worktree/control-checkout selection; do not infer a new origin.

Use the installed `swamp` and `agenda` commands. Check their help before using
unfamiliar flags. Use JSON for machine reads. The control checkout's Swamp vault
`default` stores the workspace under `AGENDA_WORKSPACE`; the launcher reads it
automatically. Configure a missing value with `swamp vault put default
AGENDA_WORKSPACE --repo-dir <control-checkout>` using the intended existing
workspace. Do not infer it from Agenda's current selection. `--workspace` is an
optional assertion against the vault value, not an override. Workflow execution
and integration also verify the vault value against the issue's recorded
workspace. A mismatch requires inspecting the configuration, not recreating the
issue. The workspace is a public issue identifier and remains in lifecycle state.
Read the Agenda workspace identity and item before changing it; identify the
issue by the configured workspace and returned item ID. Let the launcher compute
the behavior name; it hashes long workspace names to stay within Swamp's name limit.

Inspect persisted state before starting or resuming. Resume its current phase;
do not recreate the behavior, reset its history, or infer success from a
previous conversation. The Agenda title and note are the requirements. If they
changed, use `refresh` and review the revised requirements before proceeding.

## Drive the work

After `start`, use the launcher's `drive` command with JSON input
`{agent: {session, kind, model?, effort?, permissionMode?, startupTimeoutSeconds?, timeoutSeconds?}}`.
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
Use the local Yinz agents skill for deliberate pane inspection and dialogs.

**Wait for the driver, collect its structured handoff, and present its readable
result before ending the controlling turn.** If a shell command yields, keep
waiting on that same command. A launched pane, workflow ID, or progress report
does not complete the request. Present the issue, plan version or commit,
decision required and supporting evidence (plan behavior, steps, tests,
assumptions and review; or delivered change, code review and verification).
For a blocker, explain the recorded cause and the recovery needed. Do not promise
to wake the controller after its turn or session ends; that is outside this flow.

The driver automatically runs initial triage, planning and review, followed by
at most one targeted revision and review when blockers remain. The allowance is
reserved with the exact assignment before launch, once per requirements generation.
Resume, rollback, restart, manual planning and command reinvocation never replenish
it. Only a refresh with genuinely changed requirements starts a new generation.
Legacy first-version plans may receive one automatic revision; later legacy versions
are treated as having consumed it. Existing revision authorizations remain valid.

After that allowance, unresolved high/critical findings stop at
`authorize_plan_revision`, including prior findings, decisions, revision changes
and disagreements. Inspect cited source and adjudicate supported findings with
`adjudicate-plan`: pass the current `planVersion`, `reviewFingerprint` from the
handoff, and `dispositions` with `findingId`, `disposition` (`resolved`, `rejected`
or `downgraded`), `rationale`, and nonempty source `evidence`; a downgrade also
requires the corrected `severity`. Explain the judgment and what code you inspected.
The controller may record supported dispositions without asking the user. Keep the
original review intact. Never call an accepted unresolved high/critical risk
resolved: it remains blocked for user direction. Stale adjudications and mutations
during unresolved assignments are rejected. Then `drive` again to obtain the next
handoff.

If another revision is needed, present the concrete handoff and obtain explicit
authorization for exactly one targeted revision. Record it with `drive` input
`{revisionApproval: {handoffId, request}}`, using the exact handoff ID and user's
actual request. The complete assignment includes that request and is saved before
dispatch. Resume reuses it and reviews the resulting version. Never infer blanket
approval or ask again for an already authorized assignment.

Planners and reviewers must read [planning standards](references/planning.md) and
[review standards](references/review.md). Preserve unaffected steps, accepted
tradeoffs and finding IDs. Reopening a disposed concern requires new evidence
explaining why its previous disposition no longer holds. Historical context never
substitutes for current review evidence.

Revision authorization does not authorize implementation or merge. The driver
stops separately for explicit approval of the reviewed plan and worktree
preparation, then runs implementation and code review up to merge authorization.
It never records approval, merge authorization or a CI waiver on its own.

1. **Triage:** the driver runs role `triage` to investigate the problem, acceptance
   criteria, scope and reproduction evidence against the affected source.
2. **Plan:** the driver runs role `plan` for concrete steps, affected files, risks and
   development test selection, then role `review_plan` for adversarial review.
   The driver allows one automatic targeted revision; further revisions need
   individual authorization. Every revised plan needs a fresh review.
3. **Approve and implement:** record authorization for the reviewed plan, then
   adopt the recorded worker worktree and its existing feature branch. A conflicting
   implementation path is rejected before Git preparation. Existing approval in
   this session is sufficient; do not ask again. If authorization is missing, present the
   concrete reviewed plan and explain the decision needed before proceeding.
4. **Develop:** the driver runs role `implement` in the prepared issue worktree. The
   worker implements and commits the plan and runs targeted tests. Fix defects
   and flakes in this branch through the worker. Keep each worktree's real
   `target/` isolated; never set `CARGO_TARGET_DIR` or share build artifacts.
5. **Review code:** the driver runs role `review_code` against the committed worktree.
   Swamp imports the role-specific structured result only after validating the
   original task, plan and commit. Repairs require fresh review evidence.
6. **Prepare to merge:** show the resulting change and verification evidence.
   Record the user's authorization with `ready`; existing explicit merge
   authorization counts. Do not treat plan approval as merge authorization.
7. **Integrate:** run `merge` from the control checkout. It serializes rebase,
   `cargo x ci check` and local fast-forward merge with a shared coordinator and
   a Git-repository lock. Full CI is the merge gate for this lifecycle. Skip it
   only when the user explicitly requests that exception, recording the request
   and its scope in `skipCiRequest`; time pressure, previous checks and an
   agent's judgment do not supply that request.
8. **Summarize:** compare the delivered outcome with the original requirements.
   Complete the Agenda task only after the local merge and `outcomeMet: true`.
   Leave an unmet issue open and explain the remaining work.

Use the launcher to persist transitions. Keep human issue descriptions in
Agenda; avoid replacing them with generated logs. Swamp state and events retain
plans, review evidence, authorization, verification and merge history.

The direct evidence methods are lower-level recording interfaces, not a shortcut
around pane workers. Operator actions such as approval, worktree preparation,
merge authorization and close-out remain under the controlling workflow.

## Concurrency and recovery

Development and review can proceed independently in multiple worktrees. The
integration lock covers the repository's shared Git directory, so only one
cooperating lifecycle can rebase, run the full gate and update the target at a
time. Waiting for that lock is not a FIFO queue. Per-issue Swamp locking does
not replace the shared integration boundary.

A failed or interrupted step is not evidence of completion. Inspect state, the
exact workflow run and Git before retrying. Keep the worktree intact for
diagnosis. Resolve conflicts and verification failures in that worktree, record
the repair and refresh review evidence before retrying integration. Do not
silently abort a rebase, reset changes, delete branches, force a merge or remove
a lock file held by another process.

An unresolved pane task blocks ordinary lifecycle mutations and replacement
workers. `drive` resumes its original task and submission, including partially
applied results. It continues observing ordinary acknowledged wait timeouts and
returns a blocker for dialogs, uncertain delivery, missing sessions, provider
failures or invalid evidence. Resolve the blocker deliberately, then `drive`
again; use `resume-agent` for lower-level observation. Do not rerun `agent` to
retry the prompt automatically. Completed handoffs persist, so resume does not
repeat workers; rollback and restart invalidate superseded handoffs.
Accepted worker results are saved before their panes close. Pending cleanup is
retried on resume without replaying evidence. Failed or blocked panes stay visible.
Missing or stale Codex, Muse or AGY hooks fail preflight and
are not installed automatically.

When the user asks to restart or roll back, that request authorizes superseding
the previous attempt regardless of its outcome. Do not require successful
resume, proof of non-delivery, `confirmedStopped`, or another permission question.
Use `rollback` with `{step, request}` to return to any lifecycle step, retaining
its prerequisites and invalidating downstream decisions. Supported steps are
`triage`, `plan`, `review_plan`, `approve`, `implement`, `review_code`, `ready`,
`merge` and `summarize`. Use `restart-agent` with the current `taskId`, actual
`request`, and ordinary agent inputs to roll back to a worker step and start a
fresh assignment. Select the intended running session; the old session may be
gone. Repeating identical restart inputs resumes the replacement, while a new
restart must target its new task ID.

Rollback preserves history, Git changes and worktree ownership, reopens completed
Agenda tasks, and keeps plan versions increasing. A landed merge is never undone;
merge and close-out retries reuse its receipt. Changed requirements require
rollback to triage. Old panes remain visible: inspect and stop surviving workers
before replacement work edits the same checkout. Superseding their assignment
does not prove their processes stopped. After an interrupted rollback, use
`resume-rollback` to finish its recorded intent and resume its replacement; do
not ask again for authorization already recorded on that operation. A new explicit
rollback request may also supersede a pending recovery; it preserves the old intent
in history and does not require completing that recovery first.

If a worker completed but returned unusable evidence, `discard-agent` requires
the operator's actual `request` and `confirmedStopped: true`. It accepts a
matching completed worker with no background tasks and unapplied evidence, or a
durable `launch_rejected` task whose structured rejection retained no pane,
submission, background work or other resources. Lifecycle state must still match
the task's original snapshot. After discarding a rejected launch, correct the
configuration in a new `agent` request. No configuration change or relaunch is
automatic. Malformed responses, transport failures and legacy uncertain launches
remain blocked for discard; an explicit rollback or restart can supersede them.
Discard cannot end active work or undo source changes; inspect any changes before
assigning replacement work.

If a state-changing transition was saved but its final task-completion write
failed, `resume-agent` validates the completed result and reconciles the task
without repeating the transition or event write. Reconcile first, then `refresh`
if Agenda requirements changed afterward. Unapplied evidence still requires
fresh requirements, including implementation reports that leave lifecycle state
unchanged.

If the merge landed but Agenda close-out failed, resume close-out from the merge
receipt rather than merging again. Existing ad-hoc work in the recorded worktree
can be adopted, but it still needs a real plan, review and verification; never fabricate evidence to
advance the phase.

This lifecycle performs no PR creation, push, fetch, remote merge, deployment or
notification to other people. Such actions require their own user instruction.
