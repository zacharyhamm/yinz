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
`yinz-issue-start` and `yinz-issue-merge`. Read
[the operating guide](../../../docs/ISSUE_LIFECYCLE.md) for commands, evidence
inputs, the reusable agent extension and recovery.

## Establish identity and resume

Select one canonical control checkout containing the Swamp definitions. Use its
`scripts/issue-lifecycle` launcher for every lifecycle operation, including from
an issue worktree. Pass `--control` when invoking a different copy of the
launcher. Do not initialize separate lifecycle stores in each worktree.

Use the installed `swamp` and `agenda` commands. Check their help before using
unfamiliar flags. Use JSON for machine reads. Read the Agenda workspace identity
and item before changing it; identify the issue by the explicit workspace and
returned item ID. Let the launcher compute the behavior name; it hashes long
workspace names to stay within Swamp's name limit.

Inspect persisted state before starting or resuming. Resume its current phase;
do not recreate the behavior, reset its history, or infer success from a
previous conversation. The Agenda title and note are the requirements. If they
changed, use `refresh` and review the revised requirements before proceeding.

## Drive the work

Dispatch worker tasks with the launcher's `agent` command and JSON input
`{role, session, kind, model?, effort?, permissionMode?, startupTimeoutSeconds?, timeoutSeconds?}`.
Use the intended running Yinz session and provider. Omitting model, effort and
permission mode preserves provider defaults. The workflow supplies the prompt
and existing worktree. Never replace this with headless provider execution or
inline agent work. Use the local Yinz agents skill for deliberate pane
inspection and dialog handling when needed.

1. **Triage:** dispatch role `triage` to investigate the problem, acceptance
   criteria, scope and reproduction evidence against the affected source.
2. **Plan:** dispatch role `plan` for concrete steps, affected files, risks and
   development test selection, then role `review_plan` for adversarial review.
   Resolve blockers through revised worker results. A revised plan needs a fresh
   review.
3. **Approve and implement:** record authorization for the reviewed plan, then
   create or adopt a local issue worktree. Existing approval in this session is
   sufficient; do not ask again. If authorization is missing, present the
   concrete reviewed plan and explain the decision needed before proceeding.
4. **Develop:** dispatch role `implement` in the prepared issue worktree. The
   worker implements and commits the plan and runs targeted tests. Fix defects
   and flakes in this branch through the worker. Keep each worktree's real
   `target/` isolated; never set `CARGO_TARGET_DIR` or share build artifacts.
5. **Review code:** dispatch role `review_code` against the committed worktree.
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

An unresolved pane task blocks lifecycle mutations and replacement workers. Use
`resume-agent` to observe its original submission after a timeout, dialog or
lost response. Do not rerun `agent` to retry the prompt. Unknown submissions and
uncertain launches require inspection; no automatic respawn or resubmission is
safe. Panes stay visible. Missing or stale Codex, Muse or AGY hooks fail
preflight and are not installed automatically.

If a worker completed but returned unusable evidence, `discard-agent` requires
the operator's actual `request` and `confirmedStopped: true`. It accepts a
matching completed worker with no background tasks and unapplied evidence, or a
durable `launch_rejected` task whose structured rejection retained no pane,
submission, background work or other resources. Lifecycle state must still match
the task's original snapshot. After discarding a rejected launch, correct the
configuration in a new `agent` request. No configuration change or relaunch is
automatic. Malformed responses, transport failures and legacy uncertain launches
remain blocked. Discard cannot end active work or undo source changes; inspect
any changes before assigning replacement work.

If a state-changing transition was saved but its final task-completion write
failed, `resume-agent` validates the completed result and reconciles the task
without repeating the transition or event write. Reconcile first, then `refresh`
if Agenda requirements changed afterward. Unapplied evidence still requires
fresh requirements, including implementation reports that leave lifecycle state
unchanged.

If the merge landed but Agenda close-out failed, resume close-out from the merge
receipt rather than merging again. Existing ad-hoc work can be adopted, but it
still needs a real plan, review and verification; never fabricate evidence to
advance the phase.

This lifecycle performs no PR creation, push, fetch, remote merge, deployment or
notification to other people. Such actions require their own user instruction.
