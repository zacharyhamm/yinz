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

Optional defaults live in the canonical control checkout's
`.yinz/issue-lifecycle.json`, for example
`{"agent": {"kind": "codex", "model": "your-model", "effort": "your-effort"}}`.
Omit model/effort for provider defaults. `start --input-file` accepts an optional
`agent`: `{"agent": {"effort": "your-effort"}}` overrides one field;
`{"agent": {"kind": "claude"}}` changes kind and clears inherited model/effort;
`{"agent": {"model": null, "effort": null}}` resets those fields to provider
defaults. The launcher reads only `--control` configuration. Start saves one
resolved `workerAgent` for every issue role before confirming the worktree claim;
repeated/interrupted starts preserve it. Changed defaults affect new issues.
Inspect the saved choice in status and readable handoffs. Start does not launch
triage.

With a saved choice, `drive` infers the session from the controlling pane and
needs no agent input. Outside a pane, supply
`{agent: {session}, control: {session, paneId}}`; launch options
`permissionMode`, `startupTimeoutSeconds` and `timeoutSeconds` are optional.
Explicit `drive.agent.kind` replaces provider settings: supply model/effort or
omit them for provider defaults. Legacy issues or absent defaults retain the
explicit `{agent: {session, kind, model?, effort?, ...}}` flow.
Settings persist; ordinary resume is `drive` with no input. Pending workers and
saved revision assignments keep their original configuration. New workers resolve the
control pane's current tab and join its separate worker stack; existing workers
keep tabs/submissions. Sessions must match. Outside the pane environment use saved
identity or `control: {session, paneId}` alongside `agent` (role options for
`agent`/`restart-agent`). Older drivers need identity before new launches; no
automatic-tab fallback is allowed.

New workers require `agent-launch-transaction-v2` and persist an incarnation-qualified launch ID before spawn. Resume observes that exact ID, never a replacement spawn. Definitive terminal receipts can support deliberate discard even when Git paths remain; pending, unknown, or legacy ID-less launches require inspection. A receipt alone never supplies worker evidence or changes the original approval/task/prompt correlation.

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
| `implementing` | `drive` implements/commits with focused tests, then runs technical code review followed by separate UX review; completed implementation or rollback to `review_code` routes to review. Both current reviews yield `address_review_findings` when findings remain, regardless of severity. Ask which findings to repair or ignore. Selected repairs run a planner, yield `approve_review_fixes`, then run an implementation worker only after approval. Fresh reviews follow repairs. Accepted findings permit checks at the same full commit and plan version. UX-only rollback retains technical review and repeats UX. Stale evidence or failed verification blocks; passing gate yields `authorize_merge`. |
| `ready` | Active merge failure: `blocked`, read log and repair. Otherwise `integrate`. |
| `merged` | `summarize` compares outcome to original requirements using landed receipt. |
| `done` | `authorize_closeout`: present the optional exact bundled deletion/pane-close offer. Declined or confirmed cleanup yields `complete`; pending cleanup yields `resume_closeout`. |

| Handoff decision | Controller action |
| --- | --- |
| `authorize_plan_revision` | Supported adjudication or authorization for one targeted revision, below. |
| `approve_plan` | Present reviewed plan; `approve {planVersion, request}` records explicit approval. Existing session authorization within scope suffices; ask only if absent. |
| `prepare_worktree` | `implement {worktree, branch}`, then `drive`. |
| `address_review_findings` | Present both reviews and ask which findings to address. Record actual direction with `decide-review`, then `drive`; never select or ignore findings on the user's behalf. |
| `approve_review_fixes` | Present the focused repair plan and checks. Obtain explicit approval, record `approve-review-fixes`, then `drive`. Selection authorizes planning only. |
| `authorize_merge` | Show change, technical review, UX assessment/findings and verification; `ready {commit, mergeRequest}` records explicit merge authorization, including an existing explicit request and acceptance of remaining findings for this commit. Plan approval does not authorize merge. |
| `integrate` | `merge` from control checkout serializes rebase onto current local `main`, full `cargo x ci check` and local fast-forward merge. |
| `summarize` | `summarize {outcomeMet: true, summary}` only after merge and met outcome; otherwise keep Agenda open and explain remaining work. |
| `authorize_closeout` | Present exact remaining actions and original controller session/pane; acceptance ends that conversation. The initial bundle removes the worktree (including build outputs) and local branch first. Obtain a separate explicit accept/decline request; `closeout {handoffId, decision, request}` records it. Plan/merge approval never grants deletion. Initial decline retains everything. Renewed decline retains the pane and any remaining resources, preserving prior effects and unresolved reservations; it ends automatic offers. Unavailable origin/capability retains remaining resources with its diagnostic; use the guide's exact binding procedure before offering consent. |
| `resume_closeout` | Run `resume-closeout` from a surviving pane; inspect saved Git/pane progress and failure. Never substitute the recovery pane. An expired fence needs a fresh offer and explicit accept/decline; completed Git deletion is retained. After summarize rollback, re-complete the outcome before presenting fresh consent for the same saved operation/UUID. Changed requirements or merge targets block reuse. Same-incarnation exact absence reconciles an unknown pane close without another close. |
| `complete` | Present completed result and retained cleanup decision/results. |
| `blocked` | Read matching recovery section below, resolve deliberately, then `drive`. |

For `authorize_closeout` or `resume_closeout`, read-only `closeout-receipt`
validation does not acquire the repository merge lock or reconcile receipts.
Unreconciled receipts need normal integration recovery first. Preview and deletion
use only the brief worktree ownership guard and Git's native locks; standalone
deletion of an already-merged worktree and branch can proceed during another
issue's merge/CI. Merge/rebase/CI serialization remains unchanged.

Ownership-guard contention returns promptly with the blocked stage, lock and retry
action. Before an offer exists, no consent is created; after acceptance, saved
progress, prior deletions and any reservation remain recorded and the pane stays
open. Inspect the ownership operation and let it finish, then explicitly retry
`closeout`/`resume-closeout`. Do not cancel another issue's merge/CI gate, remove
lock files or delete resources manually to bypass the guard. Retry still requires
target validation and fresh consent for an expired pane fence. See
[close-out recovery](references/recovery.md#close-out-failures).

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
[review](references/review.md); plan/code/UX reviewers also read applicable
[Yinz review dimensions](references/dimensions.md). Preserve unaffected steps,
accepted tradeoffs and finding IDs. Reopening disposed concerns needs new evidence
explaining why the disposition no longer holds. History never replaces current
review; repairs need fresh review. Ad-hoc code still needs real planning/review/
verification. Never fabricate evidence. Keep human descriptions in Agenda;
launcher transitions retain evidence and history in Swamp.

Technical review assesses requirements independently of plan compliance, including
wrong design assumptions and weak tests. A distinct visible `review_ux` worker
then traces affected users/tasks, interactions and indirect usability effects over
the same full commit and plan version. `uxReview` stores its assessment, findings
and advisory checks separately; no-impact requires an explicit rationale and
assessed paths, never an inferred skip. Present both reviews, assessed UX paths
and limitations in the merge handoff. Direct `review-code`/`review-ux` methods
record evidence and never replace the visible review workers.

After both reviews, present every outstanding finding with its role, ID, severity,
evidence and proposed repair. The user chooses what to repair or ignore; no
severity forces a repair. `decide-review` takes handoff `id`,
`reviewFingerprint` (the handoff's `codeReviewFingerprint`), actual `request`,
and `repair`/`ignore` arrays of `{role: "review_code"|"review_ux", findingId}`.
Unselected findings remain pending. Ignoring a finding preserves it as unresolved.
Repeated drive calls never authorize or start repairs on their own.

Selected repairs launch a visible `plan_review_fixes` worker. Present its plan,
risks, assumptions and focused checks. `approve-review-fixes {id, handoffId,
request}` uses the selection's repair ID, current approval handoff ID and actual
user approval. Only then can `implement_review_fixes` edit and commit selected
repairs. The original approved issue plan remains intact. Resume uses persisted
assignments and approvals; every additional repair round needs new direction.

Both reviews must remain current. Verification requires user acceptance of
outstanding findings; explicit merge sign-off after presenting them may accept
all remaining findings. Record that acceptance with `decide-review` before
checks, then reuse the same explicit merge request with `ready` after they pass;
do not ask for the same authorization again. `ready` also records acceptance of
remaining findings. User acceptance never waives failed checks or merge CI.
New technical evidence invalidates UX;
code repairs require fresh technical and UX review plus verification. UX-only
rollback retains matching technical evidence/head and drops UX and downstream
decisions; ready-only rollback retains the pair and exact-commit verification.
Historical records parse without fabricated UX. Legacy implementing work with
technical review proceeds to UX. A legacy unmerged ready state without UX needs explicit
rollback to `review_ux`, drive/checks, and new merge authorization; legacy ready
findings with both reviews return to `address_review_findings`; an unlanded
active attempt requires repair and fresh reviews. Already-landed durable receipts
remain facts and merge/close-out replay does not require retrospective reviews.
The bounded [reviewer evaluation](references/review-evaluation.md) measures seeded
scenarios and documents limitations, not current implementation approval.

Keep each worktree's real `target/` isolated: no `CARGO_TARGET_DIR`, changed
`build.target-dir` or shared/copied mutable build artifacts. Fix defects/flakes in
this branch through the worker. After both reviews and user decisions, lifecycle
checks run approved issue and repair checks at the exact commit outside the merge lock, recording exit codes/durations/logs; reviewer
checks are advisory. `ready` needs passing exact-commit verification. Legacy plans
without checks explicitly retain the reviewer-reported gate, including failed
checks from either role, and still require a separate current UX assessment. Failed verification
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
`review_code`, `review_ux`, `ready`, `merge`, `summarize`; changed requirements require `triage`.
`restart-agent` uses current `taskId`, actual `request` and ordinary agent inputs
with the intended running session. Identical inputs resume the replacement;
another restart targets its new task ID. Inspect/stop surviving workers before
replacements edit the same checkout. After restart/completed `resume-rollback`,
`drive` to the next handoff; saved intent needs no renewed approval, and a new
explicit rollback may supersede pending recovery. History, Git changes, ownership
and increasing versions survive; completed Agenda tasks reopen. Landed merge and
close-out retries reuse receipts. [Design reference](../../../docs/ISSUE_LIFECYCLE.md)
holds persistence and input contracts.

A worker is complete only after its role evidence is validated, recorded and
acknowledged through its assigned `@yinz/issue-evidence` behavior. Final prose is
presentation and is never parsed as lifecycle evidence. Workers may invoke only
the supplied submission method and receipt query; approvals, rollback, merge and
Agenda mutations remain controller operations. Validation errors are corrected
within the same worker. Lost acknowledgements are recovered by inspecting the
original task's durable submission; identical replay preserves its receipt.

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



### Qualified completion after closure

Ordinary drive/resume observes the saved submission without repeating implementation.
Claude observation retries initial transcript absence and exact completion delivery
under an absolute deadline and the original process/conversation identity. A
linked answer plus positive registry idle can qualify a screen-thread closure
snapshot, provided no input or activity intervened. Accepted evidence remains
insufficient until execution is positively terminal and quiescent.

The same server retains at most 1024 closed proofs for 24 hours. The child checks
launch incarnation, session, provider, cwd, pane, conversation, submission and
saved turn identity, then saves the proof before publishing its result. Interrupted
publication resumes from that durable proof; normal parent import guards and
stable events still apply. Retained completion does not imply live availability.

Missing/legacy proof, expiry, eviction, restart, replacement, interruption and
unknown/background work remain blocked with their actual diagnostic. Inspect
saved identity and execution facts; do not infer success from receipt acceptance,
elapsed time, absence, raw Stop, end_turn or EOF. Historical #78 facts already
deleted cannot be reconstructed by this path. Restart/rollback and cleanup still
require their existing controller authority; do not repeat finished work merely
to rebuild bookkeeping. See docs/AGENT_CLI.md, “Closed submission reconciliation”,
for the response fields and direct wait syntax.
