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
fetch/push, remote merge, deployment and notifications to others need separate instructions.
Human-intervention handoffs automatically send an in-app Yinz notification;
activating it focuses the saved control pane.

## Identity and setup

Use one canonical control checkout and its `scripts/issue-lifecycle` launcher for
every operation (`--control` when using another copy); never split stores across
worktrees. Commands below take `--item <returned-id>`. Human decisions use the
launcher flags `--handoff`, `--request` (or `--request-file -` for text with
quotes) and the selection flags described below; other inputs are JSON via
`--input-file`. Use installed `swamp`/`agenda`, help for unfamiliar flags, JSON
for machine reads, issue-scoped input files and arguments rather than issue text
interpolated in shell.

The control vault `default/AGENDA_WORKSPACE` identifies the existing workspace,
never Agenda's current selection. `--workspace` asserts equality, not an override.
Read workspace identity/item before changing it; let the launcher compute names.
Consult [setup reference](../../../docs/ISSUE_LIFECYCLE.md#one-control-checkout)
for missing configuration, prerequisites and claims.

Inspect `status` before start/resume; never recreate the behavior or reset history.
Trust persisted evidence, not prior conversation. Agenda title/note are
requirements; `drive` compares them on every pass (below). Run `start` from the
control checkout on `main`, or from anywhere with `--new-worktree`: it creates or
adopts `<primary checkout>/.worktrees/issue-<item>` on branch `issue-<item>` from
local `main`. Run from a feature worktree (or a subdirectory), `start` claims that
worktree instead, refusing a detached HEAD and other issues' claims. The claim is
the canonical `workerWorktree`: all new workers retain that directory across
roles, revisions/recovery and later invocations; repeated start does not replace
it. Pending tasks keep their configuration.

Six open issues predate current contracts. When a handoff or status names a
legacy state, read [Legacy states](../../../docs/ISSUE_LIFECYCLE.md#legacy-states).

## Drive and resume

Optional worker defaults (`agent`) and verification settings (`verification`)
live in the control checkout's `.yinz/issue-lifecycle.json`; `start --input-file`
may override `agent` fields for one issue. Start saves one resolved `workerAgent`
before confirming the claim and does not launch triage. See
[worker defaults](../../../docs/ISSUE_LIFECYCLE.md#worker-defaults-and-issue-overrides).

With a saved choice, `drive` infers the session from the controlling pane and
needs no agent input. Outside a pane, supply
`{agent: {session}, control: {session, paneId}}`; launch options
`permissionMode`, `startupTimeoutSeconds` and `timeoutSeconds` are optional.
Explicit `drive.agent.kind` replaces provider settings: supply model/effort or
omit them for provider defaults. Settings persist; ordinary resume is `drive`
with no input. Pending workers and saved revision assignments keep their original
configuration. New workers resolve the control pane's current tab and join its
separate worker stack; existing workers keep tabs/submissions. Sessions must
match. Outside the pane environment use saved identity or
`control: {session, paneId}` alongside `agent` (role options for
`agent`/`restart-agent`); no automatic-tab fallback is allowed.

Decision commands (`approve`, `adjudicate-plan`, `decide-review`,
`ready`, and legacy `approve-review-fixes`) and `implement`, `repair`, `refresh`,
`resume-rollback` and `merge` continue with `drive` unless `--no-drive`. Only
after `ready` or `drive`, when drive yields an authorized `integrate` handoff
with no existing merge attempt, does the launcher run merge once and drive again;
`--no-merge` stops before it. Every other chained command stops at an
`integrate` handoff, even an authorized one (for example `decide-review` on a
legacy `ready` state, or `resume-rollback` to `merge`). A merge's full CI gate
can hold the shared lock for hours, so run `ready`, `drive` and `merge` in the
background (or with a timeout of several hours) and wait for their exit. Progress goes to stderr; stdout carries exactly
one handoff JSON. A killed launcher leaves an interrupted attempt for
inspection; it is never re-run automatically.

Wait for the structured handoff, then present the readable result before ending
the controlling turn: issue, plan version/commit, decision and evidence (plan
behavior, steps, tests, assumptions/review, or delivered change, review and
verification checklist). Each presentation ends with a `Next:` line naming the
exact command. Pane/workflow IDs and progress are not completion. Explain
blocker cause/recovery; do not promise a later controller wakeup.

Pending tasks, saved assignments, partial imports, cleanup and a pending rollback
resume **before phase dispatch**. `drive` finishes a pending rollback's saved
intent first; a new explicit rollback may supersede it instead. Drive retries
bounded, correlated recoveries itself: it re-observes the original submission
without resending, retries transient cleanup guards and launch-preflight
transport failures, and blocks with the reason when a budget is exhausted or a
failure needs judgment ([retry classes](references/recovery.md#unresolved-tasks)).
Changed Agenda requirements are refreshed automatically before any approval
(logged; planning restarts in a new generation). After approval, or while a
revision authorization is reserved, drive blocks with the changes and the
approvals a refresh would invalidate. Completed handoffs persist until
superseded. `agent`/`resume-agent` are diagnostics; use the local Yinz agents
skill for pane inspection/dialogs.

Every return of `authorize_plan_revision`, `approve_plan`,
`address_review_findings`, `authorize_merge`, `authorize_closeout`, `blocked` or
a saved legacy `approve_review_fixes` sends a notification, including repeated
returns of the same handoff. Delivery failure is logged without changing the
handoff. Notifications request attention; the controller still presents the
evidence and records the actual human decision.

| Phase | Next action after pending work is resolved |
| --- | --- |
| `triaging` | `drive` triages source, problem, scope, acceptance criteria and reproduction evidence. |
| `classified` | `drive` plans steps/files/risks, focused tests and verbatim argv `checks` covering steps, then runs adversarial review. Present checks: plan approval approves running them. |
| `planning` | Missing/current-version-mismatched review: review. Blocking current review: one automatic revision/review if available, otherwise `authorize_plan_revision`. Clean review: `approve_plan`. Missing plan blocks. |
| `approved` | `drive` adopts the recorded worker worktree and its branch (or the implementation a rollback kept), rejecting conflicting paths before Git preparation. `prepare_worktree` appears only after accepted close-out cleanup or for a legacy issue. |
| `implementing` | `drive` implements/commits with focused tests, then runs technical code review. Pending medium-or-higher findings yield `address_review_findings`; selected repairs go straight to a repair worker, then a delta code re-review of the repair diff. Once code findings are settled, one UX review runs, with the same round shape for its findings. Low findings become follow-ups. With findings decided, drive runs verification: failure blocks, a pass yields `authorize_merge`. |
| `ready` | `integrate`, which the launcher merges unless `--no-merge`. An active merge failure blocks: read its log and `repair`. |
| `merged` | `summarize` compares outcome to original requirements using landed receipt. |
| `done` | `authorize_closeout`: present the optional exact bundled deletion/pane-close offer. Declined or confirmed cleanup yields `complete`; pending cleanup yields `resume_closeout`. |

The launcher never fills `--handoff`: pass the id of the handoff you presented,
as its `Next:` line shows. The behavior derives plan versions, fingerprints,
repair IDs and commits from that exact handoff and rejects it once stale.

| Handoff decision | Controller action |
| --- | --- |
| `authorize_plan_revision` | Supported adjudication or authorization for one targeted revision, below. |
| `approve_plan` | Present the reviewed plan and its checks; `approve --handoff <id> --request '<approval>'` records explicit approval and continues. Existing session authorization within scope suffices; ask only if absent. |
| `prepare_worktree` | Only after accepted close-out cleanup or for a legacy issue: `implement --input-file <{worktree, branch}>` with a deliberately selected worktree; drive continues. |
| `address_review_findings` | Present the current reviews' blocking findings (code only before UX review runs) and any follow-ups, and ask which to address. Record the actual direction with `decide-review --handoff <id> --request '<direction>'` plus `--repair`/`--ignore <role/id>` per finding (or `--ignore-remaining`); drive continues and selected repairs go straight to a repair worker. Never select or ignore findings on the user's behalf; only the user's own standing instruction enables `drive --auto-repair`. |
| `approve_review_fixes` | Legacy only (saved before repair planning was retired). Run `drive`, which dispatches the repair worker without the approval; or, with explicit approval, `approve-review-fixes --handoff <id> --request '<approval>'` keeps the saved plan as guidance. |
| `authorize_merge` | Show change, technical review, UX assessment/findings and verification; `ready --handoff <id> --request '<merge authorization>'` records explicit merge authorization, including an existing explicit request and acceptance of remaining findings for this commit. Plan approval does not authorize merge. The launcher then runs merge; add `--skip-ci-request` only for the user's explicit waiver. |
| `integrate` | The launcher runs merge after `ready`/`drive` unless `--no-merge`; other chained commands stop here, so run `merge` (in the background). An existing attempt is never re-run automatically: inspect it, then run `merge` explicitly. Merge serializes rebase onto current local `main`, full `cargo x ci check` and local fast-forward merge in the control checkout. |
| `summarize` | `summarize {outcomeMet: true, summary}` only after merge and met outcome; otherwise keep Agenda open and explain remaining work. It first files each listed follow-up as its own Agenda item (never editing the issue's note); a failed add is retryable. |
| `authorize_closeout` | Present exact remaining actions and original controller session/pane; acceptance ends that conversation. The initial bundle removes the worktree (including build outputs) and local branch first. Obtain a separate explicit accept/decline request; `closeout --handoff <id> --decision accept\|decline --request '<decision>'` records it. Plan/merge approval never grants deletion. Initial decline retains everything. Renewed decline retains the pane and any remaining resources, preserving prior effects and unresolved reservations; it ends automatic offers. Unavailable origin/capability retains remaining resources with its diagnostic; use the guide's exact binding procedure before offering consent. Lock contention: [close-out recovery](references/recovery.md#close-out-failures). |
| `resume_closeout` | Run `resume-closeout` from a surviving pane; inspect saved Git/pane progress and failure. Never substitute the recovery pane. An expired fence needs a fresh offer and explicit accept/decline; completed Git deletion is retained. After summarize rollback, re-complete the outcome before presenting fresh consent for the same saved operation/UUID. Changed requirements or merge targets block reuse. Same-incarnation exact absence reconciles an unknown pane close without another close. |
| `complete` | Present completed result and retained cleanup decision/results. |
| `blocked` | Read `status`'s `taskRecovery` first; it names the deterministic recovery for the current pane task. Then read the matching recovery section below, resolve deliberately, then `drive`. |

## Review and authorization

One automatic targeted revision/review is reserved with its exact assignment before
launch per requirements generation. Resume, rollback, restart, manual planning and
reinvocation never replenish it; only refresh with genuinely changed requirements
does. Saved revision authorizations remain valid.

At `authorize_plan_revision`, inspect cited source and explain judgment.
`adjudicate-plan --handoff <id> --input-file <{dispositions}>` binds the
handoff's plan version and review fingerprint. Each disposition has `findingId`,
`disposition` (`resolved`, `rejected`, `downgraded`), `rationale`, nonempty
source `evidence`, plus corrected lower `severity` for downgrade. Supported
dispositions need no extra permission. Preserve original review; stale
inputs/mutations during unresolved assignments are rejected. Accepted unresolved
high/critical risk stays blocked for user direction. Drive then continues.

For another revision, present the concrete handoff and obtain authorization for
exactly one targeted revision. `drive --handoff <id> --request '<authorization>'`
binds that handoff and the actual user request; the complete assignment is saved
before dispatch, reused on resume and freshly reviewed. Never infer blanket
approval or ask again for that assignment. Revision, implementation, merge and
CI-waiver authorizations are separate; the driver grants none itself.

Planners/reviewers read [planning](references/planning.md) and
[review](references/review.md); plan/code/UX reviewers also read applicable
[Yinz review dimensions](references/dimensions.md). Preserve unaffected steps,
accepted tradeoffs and finding IDs. Reopening disposed concerns needs new evidence
explaining why the disposition no longer holds. History never replaces current
review; repairs need fresh review. Ad-hoc code still needs real planning/review/
verification. Never fabricate evidence. Keep human descriptions in Agenda;
launcher transitions retain evidence and history in Swamp.

Technical review assesses requirements independently of plan compliance; once
its findings are settled, a distinct visible `review_ux` worker reviews the same
full commit and plan version for user impact with an explicit assessment, never
an inferred skip
([design](../../../docs/ISSUE_LIFECYCLE.md#separate-implementation-reviews)).
Present both reviews, assessed UX paths and limitations in the merge handoff.
Direct `review-code`/`review-ux` methods record evidence and never replace the
visible review workers.

At `address_review_findings`, present every pending finding with its role, ID,
severity, evidence and proposed repair, plus any follow-ups. Before UX review runs
only code findings are pending. The user chooses what to repair or ignore; no
severity forces a repair. `decide-review --handoff <id> --request '<direction>'`
takes `--repair <role>/<id>` and `--ignore <role>/<id>` for each finding the user
chose (roles `review_code` and `review_ux`), or `--ignore-remaining` when the user
ignores every other blocking finding. Unselected findings remain pending.
Ignoring a finding preserves it as unresolved. Repeated drive calls never
authorize or start repairs on their own.

Low findings are follow-ups: they start no round, stay unresolved, and are listed
at `address_review_findings`, `authorize_merge` and `summarize`. The user may
still select one with `--repair`. `summarize` files each remaining one as its own
Agenda item.

Selected repairs launch a visible `implement_review_fixes` worker directly; there
is no repair plan or approval step. It edits and commits only the selected
repairs, may add focused checks under the check policy, and reports broader work
as unresolved. The original approved issue plan remains intact. The next reviews
are delta re-reviews: they cover the repair diff from the last reviewed commit,
verify the prior findings and report only regressions in the changed paths
([review rounds](../../../docs/ISSUE_LIFECYCLE.md#review-rounds)). In a delta
re-review, an unchanged finding the user already ignored stays ignored. Use
`rollback` to `review_code` or `review_ux` when the user wants a full re-audit.
Resume uses persisted assignments; every additional repair round needs new
direction unless the user gave `drive --auto-repair --request '<their standing
instruction>'`. That instruction repairs every blocking finding of the current
plan and rollback for up to three rounds, still asks on reopened findings, and is
withdrawn with `drive --no-auto-repair`.

Both reviews must remain current. Verification requires user acceptance of
outstanding findings; explicit merge sign-off after presenting them may accept
all remaining findings. Record that acceptance with `decide-review` before
checks, then reuse the same explicit merge request with `ready` after they pass;
do not ask for the same authorization again. `ready` also records acceptance of
remaining findings. User acceptance never waives failed checks or merge CI.
New technical evidence invalidates UX;
code repairs require fresh (delta) technical and UX review plus verification. UX-only
rollback retains matching technical evidence/head and drops UX and downstream
decisions; ready-only rollback retains the pair and exact-commit verification.
Historical records parse without fabricated UX. Already-landed durable receipts
remain facts and merge/close-out replay does not require retrospective reviews.
The bounded [reviewer evaluation](references/review-evaluation.md) measures seeded
scenarios and documents limitations, not current implementation approval.

Keep each worktree's real `target/` isolated: no `CARGO_TARGET_DIR`, changed
`build.target-dir` or shared/copied mutable build artifacts. Fix defects/flakes in
this branch through the worker; a flake is a defect. Plan check commands are
validated when plan evidence is submitted ([check policy](references/planning.md#check-policy));
persisted plans still verify. After both reviews and user decisions, verification
runs a rebase-conflict probe against local `main`, the configured baseline checks,
then the approved issue checks and completed repairs' checks at the exact commit outside the merge
lock, recording exit codes/durations/logs; reviewer checks are advisory. `ready`
needs passing exact-commit verification. Failed verification is not
automatically rerun. Full merge CI can be skipped only for the user's
explicit waiver for that merge, recorded with scope in `skipCiRequest`; previous
checks, time pressure and agent judgment do not suffice. The shared repository
lock covers rebase/checks/merge; per-issue locks do not replace it, nor is it FIFO.

## Blocker routing

When the driver reports `blocked`, run `status` first. Its `taskRecovery` block
names the deterministic recovery for the current pane task: `kind`, the
`command` to run, whether it needs the user's request, alternatives, and discard
eligibility with every condition. It is advisory; the named method re-verifies.
Then read the matching section of `references/recovery.md`:

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

If `drive` keeps waiting on a pane task or the user says a worker is done, follow
[Stuck drive or reported completion](references/recovery.md#stuck-drive-or-reported-completion)
before answering.

## Explicit rollback or restart

The actual user request authorizes superseding any prior outcome: no successful
resume, non-delivery proof, `confirmedStopped` or renewed approval is required.
`rollback --request '<request>' --input-file <{step}>` retains prerequisites and
invalidates downstream decisions. Steps: `triage`, `plan`, `review_plan`,
`approve`, `implement`, `review_code`, `review_ux`, `ready`, `merge`,
`summarize`; changed requirements require `triage`. `restart-agent` takes the
current `taskId` and ordinary agent inputs with the intended running session,
plus `--request`. Identical inputs resume the replacement; another restart
targets its new task ID. Inspect/stop surviving workers before replacements edit
the same checkout. After restart or rollback, run `drive`: it finishes a pending
rollback's saved intent and resumes its replacement without renewed approval,
and a new explicit rollback may supersede pending recovery. History, Git
changes, ownership and increasing versions survive; completed Agenda tasks
reopen. Landed merge and close-out retries reuse receipts.
[Design reference](../../../docs/ISSUE_LIFECYCLE.md) holds persistence and input
contracts.

A worker is complete only after its role evidence is validated, recorded and
acknowledged through its assigned `@yinz/issue-evidence` behavior. Workers start
from the evidence template in their assignment directory and submit through
`scripts/issue_lifecycle/submit_evidence.py` ([planning](references/planning.md),
[review](references/review.md)). Final prose is never evidence. Workers may invoke
only their own submission and receipt query; approvals, rollback, merge and
Agenda mutations remain controller operations.

If evidence is accepted but the pane keeps reporting background work, `drive`
returns a `blocked` handoff after bounded timeouts and repeats it on later calls
until `resume-agent` observes the pane again.
Only the user's explicit authorization permits `accept-submission` with
`{taskId, request}`. It imports the accepted receipt through the ordinary import
after current requirement and worktree checks, records the request on the task
and never prompts or closes the pane. Otherwise inspect the pane and use
`resume-agent`.

If a provider ends without accepted evidence, keep the task unresolved. Use
`continue-agent --request '<request>' --input-file <{taskId}>` to ask that same
retained conversation to submit its existing findings. The guarded continuation
is saved before delivery and resumes by its original ID. Do not repeat the
investigation. Missing original context blocks supplementation and never
authorizes a replacement review.

`discard-agent --request '<request>'` discards an unresolved task whose work
cannot be used. The behavior verifies one basis itself: a terminal failure (it
re-waits the original submission and requires fresh idle status), a
resource-free rejected launch (its durable launch receipt), or a completed worker
without accepted evidence, which it freshly observes listed and idle. A pane
missing from its session is unobservable, not stopped: it may have warped to
another session or outlived its server. Its refusal names each failed condition.
Add `confirmedStopped: true` through `--input-file` only when the behavior says
it cannot observe a completed worker and you have confirmed the stop. Lifecycle state must still match the task's
dispatch snapshot. Discard saves intent and fences submissions before marking the
task discarded; it never stops work, undoes changes or relaunches. Inspect any
changes before assigning replacement work, and correct a rejected launch's
configuration in a new `agent` request. Malformed responses, transport failures
and uncertain launches remain ineligible; an explicit rollback or restart can
supersede them. Rollback and restart likewise fence the old submission before
changing parent state. Accepted evidence is persisted before cleanup, and import
recovery never reparses chat or repeats the worker.
