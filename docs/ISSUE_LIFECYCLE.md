# Local issue lifecycle

Yinz uses Agenda for issue descriptions, Swamp for durable development state,
and Git worktrees for local implementation and merging. Agent work runs in
visible Yinz panes through the reusable `@yinz/agent` behavior. The workflow
follows Swamp's reviewed-plan and evidence model while replacing its Swamp Club
issue and GitHub PR stages with Agenda and local Git.

```mermaid
flowchart LR
    A[Agenda issue] --> T[Triage]
    T --> P[Plan and review]
    P --> I[Approved implementation]
    I --> R[Code review and targeted checks]
    R --> M[Merge authorization]
    M --> Q[Wait for integration lock]
    Q --> B[Rebase onto local main]
    B --> V[Full CI gate]
    V --> F[Local fast-forward merge]
    F --> S[Outcome and Agenda completion]
    P -->|Revise| P
    R -->|Fix findings| I
    B -->|Conflict| I
    V -->|Failure| I
```

## Ownership

| Record                         | Owner            | Purpose                                                         |
| ------------------------------ | ---------------- | --------------------------------------------------------------- |
| Workspace and item ID          | Agenda           | Stable issue identity.                                          |
| Item title and note            | Agenda           | Problem, requirements and acceptance criteria.                  |
| Plan, review and authorization | Swamp            | Versioned evidence for proceeding.                              |
| Agent task and submission      | Swamp and Yinz   | Visible worker, provider conversation and exact prompt outcome. |
| State and events               | Swamp            | Resume point and history across sessions.                       |
| Source branch and worktree     | Git              | Implementation and its isolated build artifacts.                |
| Gate and merge receipt         | Swamp and Git    | What was checked and which local commit landed.                 |
| Delivered outcome              | Agenda and Swamp | Whether the issue was resolved.                                 |

Use an existing Agenda task or capture a new one with a concise title and a note
describing the problem, acceptance criteria and useful reproduction details.
Search before creating a duplicate. Reuse project categories where useful, using
their returned IDs. Lifecycle automation does not depend on fixed category IDs.

## One control checkout

Run lifecycle commands against one canonical checkout containing `.swamp.yaml`,
the behavior implementation and workflow definitions. Issue worktrees contain
the code being changed; the control checkout holds the authoritative Swamp data.
Using a separate Swamp store in each worktree would split issue history and
per-issue locking.

The launcher [scripts/issue-lifecycle](../scripts/issue-lifecycle) selects the
checkout containing itself by default. Invoke that copy by absolute path when
working elsewhere, or use `--control` to name the control checkout explicitly.
It passes the same repository to every Swamp command. It computes the behavior
identity from the workspace and item ID, hashing long workspace names to stay
within Swamp's 64-character name limit. Use the launcher instead of constructing
behavior names yourself.

The examples use these shell variables:

```sh
LIFECYCLE_REPO=/absolute/path/to/the/control-checkout
AGENDA_WORKSPACE=your-existing-workspace
AGENDA_ITEM_ID=123
```

Replace the example workspace and item ID with values returned by Agenda. Store
the workspace once in the control checkout's Swamp vault:

```sh
printf '%s\n' "$AGENDA_WORKSPACE" | swamp vault put default AGENDA_WORKSPACE \
  --repo-dir "$LIFECYCLE_REPO" --json
```

The checked-in `default` vault definition uses local encryption. If the control
checkout has no vault definition, create it first with `swamp vault create
local_encryption default --repo-dir "$LIFECYCLE_REPO" --json`. Values and the
encryption key remain in ignored local Swamp storage; configure the value again
in a fresh clone. The workspace is an identifier, not a credential: it remains
visible in issue identity, state and events.

The launcher reads `default/AGENDA_WORKSPACE` for every operation. It does not
use Agenda's current selection or fall back when the vault value is missing or
invalid. `--workspace` is optional and only asserts that a supplied name matches
the vault. Lifecycle methods independently verify the setting before proceeding,
including direct workflow execution, and the Git integration helper checks it
before rebase and again before merging. Changing the configured workspace does
not migrate existing issues; restore the original setting to resume those issues.
After setup, the shell `AGENDA_WORKSPACE` variable is only for manual Agenda commands.

Keep `LIFECYCLE_REPO` unchanged when working in another checkout. The installed Swamp
binary loads the local `@yinz/issue-lifecycle` and `@yinz/agent` behaviors.
Python 3.11 or later, Agenda and Git must also be on `PATH`; verification
requires Yinz's normal build prerequisites. Agent tasks require the installed
`yinz` CLI, a running local Yinz session and the selected provider. Specify the
session explicitly; `YINZ_SESSION_NAME` alone does not select it.

Local `main` must be checked out in exactly one clean worktree. The issue branch
must be a different local branch in the same Git repository.

Read the issue before starting:

```sh
agenda --workspace "$AGENDA_WORKSPACE" --json info
agenda --workspace "$AGENDA_WORKSPACE" --json item show "$AGENDA_ITEM_ID"
```

Start and inspect it:

```sh
"$LIFECYCLE_REPO/scripts/issue-lifecycle" \
  --item "$AGENDA_ITEM_ID" start
"$LIFECYCLE_REPO/scripts/issue-lifecycle" \
  --item "$AGENDA_ITEM_ID" status
```

Drive triage, planning and review to the plan-approval handoff with an input file:

```json
{ "agent": { "session": "your-running-session", "kind": "claude" } }
```

Replace the example session and provider with the intended ones. Pass the file
to `drive`:

```sh
"$LIFECYCLE_REPO/scripts/issue-lifecycle" \
  --item "$AGENDA_ITEM_ID" \
  drive --input-file /absolute/path/to/drive-settings.json
```

Keep evidence files issue-scoped so concurrent work cannot overwrite another
issue's inputs. Pass paths and text as arguments; do not interpolate issue
descriptions into shell programs. Run `scripts/issue-lifecycle --help` for the
launcher interface. For direct Swamp operations, inspect
`swamp help behavior
run` or `swamp help workflow run` and always supply
`--repo-dir` with the control checkout.

## Stages and evidence

The repository skill at
[`.agents/skills/issue-lifecycle/SKILL.md`](../.agents/skills/issue-lifecycle/SKILL.md)
drives the lifecycle. Use `drive` for ordinary start/resume flows and after
restart or rollback recovery. Each task runs in a visible Yinz pane and
returns a role-specific JSON object. Swamp validates that result against the
current issue, plan and commit before importing it as evidence. The controller
records human authorization and performs deterministic lifecycle operations; it
does not replace pane workers with headless agents or inline agent work.

`drive` runs in the foreground until it publishes a concrete handoff or blocker.
The controlling agent must wait for that command, collect the handoff and present
it before ending its turn. A yielded command, launched pane or workflow ID is
progress. Waking a controller after its turn or session ends is outside this
change. The launcher prints readable evidence on stderr and one handoff JSON
object on stdout. The behavior also logs the readable result and publishes the
`handoff` resource, including the issue snapshot, plan version or commit,
assignment identity, decision and supporting evidence.

Before approval, the driver fills in missing triage, planning and plan review.
Unresolved high/critical plan findings trigger up to three revision attempts per
invocation, each followed by a fresh review. It then returns `approve_plan` or a
`blocked` result containing the findings. Approval must be recorded explicitly
with `approve`. The `prepare_worktree` handoff requires `implement` to create or
adopt the worktree. After preparation, `drive` runs implementation and code
review and returns `authorize_merge` with the delivered change, review coverage
and actual verification results. Code blockers or failed checks return `blocked`
for repair and fresh review. `ready`, `merge` and `summarize` remain explicit
operator operations. The driver never infers approval, merge authorization or
a CI waiver.

Resume with `drive` and no input: the selected session and provider settings and
current assignment are durable. Pending work resumes its original task and
submission; ordinary acknowledged wait timeouts keep observing it. Dialogs,
uncertain delivery, missing sessions, invalid evidence and provider failures
produce blockers without replacement submissions. Interrupted evidence imports
reconcile before new work is dispatched. Completed handoffs retain their identity
across resume; changed state or assignments invalidate them, including restarts
and rollbacks. A pending rollback must finish through `resume-rollback` first.
Single-role `agent` and `resume-agent` remain available for deliberate low-level
control and diagnostics, using their existing inputs.

| Launcher command                | Evidence and effect                                                                                                   |
| ------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `start`                         | Read the Agenda issue and establish its workspace/item identity and description snapshot through `yinz-issue-start`.  |
| `drive`                         | Run missing workers or resume their original submissions through `yinz-issue-drive`, returning a human handoff or blocker. |
| `status`                        | Inspect the current state before deciding what to do next.                                                            |
| `refresh`                       | Re-read a changed Agenda description and invalidate planning evidence that no longer covers it.                       |
| `agent` with role `triage`      | Investigate the issue in a pane and import classification, acceptance criteria and reproduction evidence.             |
| `agent` with role `plan`        | Generate concrete steps, affected files, risks and development test strategy; revisions need fresh review.            |
| `agent` with role `review_plan` | Review the current plan against the code and import findings.                                                         |
| `approve`                       | Record the user's authorization for the reviewed plan.                                                                |
| `implement`                     | Create an issue worktree or adopt an existing one.                                                                    |
| `agent` with role `implement`   | Implement and commit the approved plan in the existing issue worktree, running focused checks.                        |
| `agent` with role `review_code` | Review the committed implementation and import plan conformance, deviations and observed verification.                |
| `resume-agent`                  | Retrieve the original submission or reconcile already-applied evidence; never resubmit or relaunch.                   |
| `rollback`                      | Explicitly return to any lifecycle step, archive the previous assignment and downstream decisions, and reopen Agenda. |
| `restart-agent`                 | Roll back to a worker step and dispatch a fresh assignment with the requested session and provider. |
| `resume-rollback`               | Finish an interrupted rollback and resume its saved replacement assignment, if any. |
| `discard-agent`                 | Discard unusable completed evidence, a confirmed stopped failure, or a resource-free rejected launch, with an operator reason and confirmation. |
| `ready`                         | Record authorization to integrate the reviewed change.                                                                |
| `merge`                         | Run `yinz-issue-merge`; serialize rebase, full CI and local fast-forward merge, recording the result.                 |
| `repair`                        | Record repair work after an integration failure and return to development and review.                                 |
| `summarize`                     | Record the delivered outcome and complete the Agenda task only when merged and `outcomeMet` is true.                  |

Direct `triage`, `plan`, `review-plan` and `review-code` commands remain
low-level evidence methods; normal agent work uses the pane roles above. The
merge workflow calls the shared `yinz-main-merge` coordinator, which invokes the
issue behavior's `integrate` method. The coordinator and OS lock serve different
purposes: one orders Swamp integration execution; the other protects the shared
Git repository.

Descriptions and plans can evolve. Refresh the issue when requirements change;
revise the plan when the approach changes. A stale review does not cover a newer
plan or a repaired implementation. Record real findings and command results;
adopting already-written code does not excuse its review.

Approval already given in the current session remains valid within its stated
scope. The operator records that authorization rather than asking repeatedly. If
plan or merge authorization is absent, present the concrete work and request the
missing decision. Approval to implement does not itself authorize merging.

## Pane tasks

The launcher captures the invoking control agent's `YINZ_SESSION_NAME` and
`YINZ_PANE_ID` for `drive`, `agent` and `restart-agent`. Each new worker queries
that pane's current tab immediately before spawning and passes the explicit
`--tab` to Yinz. This includes plan revisions, repairs and restarts. Workers
follow the control pane when it moves between tabs; already running workers
stay where they are. Explicit-tab spawning preserves keyboard focus.

Control identity is stored as `control: {session, paneId}` (for example,
`{"session":"dev","paneId":"terminal_0"}`). Direct behavior callers must
supply it for lifecycle launches. `drive` accepts it alongside `agent` so an
older saved driver can acquire identity without replacing provider settings.
Without a pane environment, the launcher preserves saved identity or accepts
explicit `control` input. A missing pane, failed lookup or session mismatch
blocks new launches; there is no automatic-placement fallback. Existing tasks
and submissions resume with their original configuration. A legacy task that
has not launched needs an explicit restart with control identity.

`agent` accepts `role`, `session` and `kind`, plus optional `model`, `effort`,
`permissionMode`, `startupTimeoutSeconds` and `timeoutSeconds`. Supported kinds
are `claude`, `codex`, `muse`, `pi` and `agy`. Defaults are 120 seconds for
startup and 600 seconds per task observation. Omitting model, effort or
permission mode keeps provider defaults; no permission bypass is selected
automatically.

The lifecycle supplies the prompt and working directory from its recorded state.
It uses the existing implementation worktree when present, otherwise the control
checkout. `implement` and `review_code` roles require the approved plan and an
issue worktree first. The operator's `implement` command creates or adopts that
worktree; the `implement` agent role performs the coding. Workers receive their
output schema in the prompt. Reviews must return actual evidence, and
implementation must return the clean worktree's full committed HEAD.

Codex, Muse and AGY require current, fully wired Yinz hooks before launch. The
behavior checks `yinz hooks status --agent KIND` and reports missing or stale
hooks; it does not install them automatically. It waits for guarded-prompt
readiness and pins the provider conversation. Fresh Muse binds its conversation
on its first prompt. Startup dialogs require deliberate interaction with the
visible pane; a timeout does not authorize answering them automatically.

The controller persists the task before dispatch. The reusable behavior saves
launch intent before spawning and a unique submission ID before prompting. An
unresolved task blocks lifecycle mutations, including dispatching a second
worker. Inspect `status`, then retrieve the original task:

```sh
"$LIFECYCLE_REPO/scripts/issue-lifecycle" \
  --item "$AGENDA_ITEM_ID" resume-agent
```

`needs_input`, timeouts, interruptions and target replacement are failures to
continue the workflow. They are not successful evidence and do not trigger a new
prompt. An uncertain spawn is never retried automatically. Unknown or expired
submissions remain unresolved until the operator requests rollback or restart;
inspect the retained pane and state. Background
tasks require a further observation of the same conversation's final idle answer
before success. Panes remain visible after completion or failure.

If dispatch stopped before the child saved its initial state, `resume-agent`
initializes the original task under the child execution lock using the saved
configuration and prompt. Existing child state is validated and resumed.

If a worker completed but its response is unusable, or its launch was durably
classified as `launch_rejected`, `discard-agent` accepts
`{"request": "the operator's actual discard instruction", "confirmedStopped": true}`.
It verifies the original task identity and unchanged lifecycle state. Completed
workers must have no background tasks. A rejected launch must have a saved,
structured `rejected` response with no pane, submission, background work or
other retained resources. For example, an unsupported effort setting can be
corrected in a new `agent` input after explicitly discarding the rejected task.
The old task's configuration remains recorded; neither resume nor discard
changes configuration or launches a replacement.

Malformed responses, transport failures, responses containing resources and
legacy uncertain launches remain blocked for `discard-agent`, but can be
superseded by explicit rollback or restart. `discard-agent` cannot discard an
active or uncertain task, and it does not close panes or undo code changes.
A correlated, acknowledged terminal provider failure (`failed`, including `StopFailure`) can
also be explicitly discarded. Discard rechecks the original submission and
requires a fresh observation of the same pane and conversation, idle with no
background tasks. The failure response remains separate from that observation.
Inspect any changes before dispatching replacement work.

A finalized prompt-preparation failure can also be discarded when the retained
server response explicitly contains `prompt_delivery: "not_sent"`. Confirmation
retrieves the original submission and requires the recorded submission ID, pane,
Yinz session, provider and conversation to match. The response must have a failure
outcome, no acknowledgement, no turn ID and no background work. Fresh status must
show that same worker idle with no background work, clearing writes or submission
writes pending. `clearing_draft` and `pending_submission` block discard until the
writes drain. The operator must supply a reason and `confirmedStopped: true`, and
the lifecycle must still equal the task's original snapshot.

Discard records the reason on the task, preserves the correlated failure separately
from fresh status, and leaves lifecycle state unchanged. An empty composer is not
required: preparation may have left a marker or draft. Inspect the composer before
reuse. Discard never resends, respawns, closes the pane or marks an issue successful;
a replacement task requires a separate explicit request.

`prompt_delivery` is optional. Missing means unknown, including older responses
whose text says the prompt was not sent. Such text cannot authorize unsent recovery.
`started` is recorded before the first actual prompt write (including a typed
prefix) and never downgraded, even if the write fails. Preparation marker and
clearing writes do not count as prompt delivery. Unknown submissions, malformed or
mismatched evidence, uncertain delivery and active workers remain blocked for
discard. This
requires both updated server responses and updated behavior code; older tasks
without structured non-delivery evidence use explicit rollback or restart.

If a state-changing transition was saved but the final task-completion write
failed, `resume-agent` validates the completed result's identity and reconciles
the task without repeating the transition or event write. This works even if
Agenda requirements changed afterward; use `refresh` once reconciliation
finishes. Evidence that still needs applying must pass the Agenda freshness
check. Implementation reports leave lifecycle state unchanged, so their recovery
still checks requirements and retries the evidence write.

## Rollback and restart

An explicit operator request can supersede any prior assignment, including a
running worker, a missing session, an expired submission, an uncertain launch,
or malformed evidence. It does not require a successful resume, a terminal
provider result, or `confirmedStopped`. Record the user's actual request;
ordinary timeouts never authorize an automatic restart.

Use `rollback` with a step and request:

```json
{ "step": "plan", "request": "Restart planning for this issue" }
```

The supported steps are `triage`, `plan`, `review_plan`, `approve`, `implement`,
`review_code`, `ready`, `merge`, and `summarize`. Rollback retains the evidence
preceding that step and invalidates decisions from that step onward. It cannot
skip prerequisites: for example, `review_plan` requires an existing plan.
Plan versions keep increasing. Changed Agenda requirements require rollback
to `triage`. Completed Agenda tasks are reopened and their descriptions preserved.

Rollback archives the original state and assignment, including failures and
partially applied evidence. A new rollback ID prevents old results from being
imported, even when returning to the same phase. It preserves source changes,
worktrees and branches. It does not reset Git, abort a rebase, undo a merge, or
close old panes. Inspect and stop any surviving old worker before allowing a
replacement to edit the same worktree. The request supersedes its workflow
assignment; it does not establish that its process stopped.

Before replacing a merge attempt, rollback consults its durable Git receipt
under the repository merge lock. A merge that already landed stays in the
history. Retrying `merge` or `summarize` reuses that receipt. Returning to an
earlier step begins a new sequence of review and authorization for subsequent
changes. Conflicts or a rebased HEAD still need repair in the retained worktree.

After rollback, continue with the ordinary command for the selected step.
For worker steps, `restart-agent` combines rollback and a new assignment:

```json
{
    "taskId": "the-current-agent-task-id-from-status",
    "role": "plan",
    "session": "your-running-session",
    "kind": "codex",
    "request": "Restart planning for this issue"
}
```

`taskId` guards against replacing a newer assignment. Use the same agent options
as `agent`; omitted optional settings use provider defaults. The replacement
gets a new task, behavior, pane and submission identity. Repeating the identical
request resumes that replacement instead of launching another worker. To restart
the replacement itself, use its new task ID and the new restart request.

Recovery intent is saved before changing Agenda, lifecycle state or assignments.
`status` includes the latest recovery record. If persistence or Agenda fails
midway, `resume-rollback` finishes the saved operation and resumes its exact
replacement, if requested. No additional approval is needed to finish the same
authorized operation. Ordinary mutations wait until recovery completes. A new
explicit rollback can supersede even a pending recovery, preserving its intent
in history; use this when requirements or the desired restart step have changed.
Recovery
records remain queryable under `recovery-<id>` and superseded assignments under
`agent-task-<id>` in the issue's behavior.

## Structured evidence and operator inputs

The input file contains an object for the selected operator command. Pane
workers produce the triage, plan and review objects below automatically. State
returned by `status` supplies the current plan version and recorded commit. Use
full Git commit IDs from the issue worktree, not a branch name or abbreviated
hash.

| Command               | Input fields                                                                                                                                                                       |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Triage result         | `classification`: `bug`, `feature`, `maintenance`, or `security`; `summary`: investigation and acceptance criteria.                                                                |
| Plan result           | `summary`; nonempty `steps`, each with unique `id`, `description`, and `files`; `testingStrategy`; optional `assumptions`. Include risks in the relevant description.              |
| Plan review result    | `planVersion`; `findings`, each with unique `id`, `severity`, `description`, and optional `resolution`.                                                                            |
| `approve`             | `planVersion`; `request`: the user's authorization in its actual scope.                                                                                                            |
| `implement`           | Absolute `worktree` path; `branch`. An existing matching worktree is adopted.                                                                                                      |
| Implementation result | `summary`; full `commit` corresponding to the clean issue worktree.                                                                                                                |
| Code review result    | `planVersion`; `commit`; `steps` covering plan step IDs with `status` of `implemented` or `deviated`; `findings`; optional `preliminaryChecks`. A deviation needs `justification`. |
| `ready`               | `commit`; `mergeRequest`: the user's authorization to merge this change.                                                                                                           |
| `merge`               | Normally no input. An explicitly requested exception uses `skipCiRequest`.                                                                                                         |
| `repair`              | `reason`: what failed and what was repaired.                                                                                                                                       |
| `summarize`           | `outcomeMet: true`; `summary`: original problem, delivered outcome and any relevant limits.                                                                                        |

Finding severities are `low`, `medium`, `high` and `critical`. Unresolved high
or critical findings block advancement. An empty findings array means a review
actually found no findings; it is not a way to skip reviewing. Each preliminary
check has `command`, `result` (`passed`, `failed` or `skipped`), and `summary`
describing the observed result and reason for the test selection.

For example, a plan result can be:

```json
{
    "summary": "Correct the reported session behavior while preserving existing clients.",
    "steps": [
        {
            "id": "session-fix",
            "description": "Repair the reproduced behavior and add a regression test covering the affected caller.",
            "files": ["yinz-server/src/"]
        }
    ],
    "testingStrategy": "Run the focused regression and affected caller tests during development; run the full gate during integration."
}
```

Replace that illustrative plan with source-specific findings. For a genuinely
clean review of its first version, the corresponding plan-review result is:

```json
{ "planVersion": 1, "findings": [] }
```

An `implement` input names the desired local branch and absolute worktree path:

```json
{
    "worktree": "/absolute/path/to/issue-worktree",
    "branch": "issue/example-fix"
}
```

These objects describe evidence and intended actions; they do not establish that
a human approved anything. Record approval inputs only after the authorization
exists in the conversation.

## Reusable Swamp extension

[`extensions/yinz-agents.yaml`](../extensions/yinz-agents.yaml) packages the
`@yinz/agent` behavior and `yinz-agent-task` workflow independently of Agenda or
this issue lifecycle. The behavior methods are `run {prompt}`, `resume {}` and
`status {}`. Global arguments are `session`, `kind`, absolute `cwd`, and the
optional model, effort, permission and timeout settings described above. Omitted
or empty optional model, effort and permission strings preserve provider
defaults. Standalone behavior callers may optionally pass
`control: {session, paneId}` to use the same tab resolution; lifecycle workers
require it. A behavior represents one prompt task; use a new behavior name for a
new task. Repeating `run` is refused, even with the same prompt. After
submission, `resume` observes its saved ID rather than sending again. It can
continue known startup progress before submission, but refuses an uncertain
launch instead of spawning another pane. A structured spawn rejection with no
retained resources is saved as `launch_rejected` with its response. Resume
explains that lifecycle tasks can use `discard-agent`; standalone callers must
use a new behavior identity with corrected inputs.

The [`yinz-agent-task` workflow](../workflows/workflow-yinz-agent-task.yaml)
wraps that behavior. Its input object has this shape:

```json
{
    "agentName": "review-example-1",
    "agent": {
        "session": "your-running-session",
        "kind": "claude",
        "cwd": "/absolute/path/to/existing-worktree"
    },
    "prompt": "Review this worktree's committed changes against local main. Do not edit files. Report findings."
}
```

Validate and run it using the installed Swamp binary:

```sh
swamp workflow validate yinz-agent-task --repo-dir "$LIFECYCLE_REPO" --json
swamp workflow run yinz-agent-task --repo-dir "$LIFECYCLE_REPO" \
  --stdin < /absolute/path/to/agent-task.json
swamp behavior run review-example-1 resume --repo-dir "$LIFECYCLE_REPO" --json
swamp behavior run review-example-1 status --repo-dir "$LIFECYCLE_REPO" --json
```

The behavior publishes `result` only after a correlated completed task returns a
nonempty final message and no background work remains. `state` retains launch,
pane, conversation and submission identity; `observation` records live status
separately. A completed provider turn is evidence of an answer, not proof that
the requested code is correct; lifecycle review and verification still apply.

## Development checks and the merge gate

Use targeted tests during development according to `AGENTS.md`. Record what ran,
its result and why the selection covers the change and affected callers. Fix
failures and flakes in the current branch; a passing rerun alone does not
resolve a failure.

This lifecycle's merge gate runs `cargo x ci check` after rebasing onto local
`main`. It covers format checking, lint, tests and integration tests. Docker
suites remain separate; include relevant remote tests when the changed behavior
requires them. Every worktree must have its own real `target/` directory. Never
set `CARGO_TARGET_DIR`, change `build.target-dir`, share a target directory, or
copy mutable build artifacts between worktrees.

Only an explicit user request permits skipping the full merge gate. Record the
request and its scope in `skipCiRequest`. A prior successful run, a small diff,
an expensive build or a desire to move faster does not supply that request. A
skipped gate is recorded as skipped, never as passed. For an ordinary merge,
omit that input:

```sh
"$LIFECYCLE_REPO/scripts/issue-lifecycle" \
  --item "$AGENDA_ITEM_ID" merge
```

## Parallel development and serialized integration

Multiple issues can be planned, developed and reviewed independently. Swamp
serializes methods for one issue. Integration additionally takes an OS lock
associated with the repository's shared Git directory, so cooperating lifecycle
runs in different worktrees share the same merge boundary.

The lock remains held across rebase, full CI and fast-forward merge. A second
issue waits before entering that sequence, then rebases against `main` as it
exists when the lock is acquired. The wait is not a FIFO queue. Independent Git
commands do not participate in this cooperative lock; avoid moving the `main`
branch manually while a lifecycle merge runs.

There is no remote fetch, push, pull request, remote merge or deployment stage.
The `main` branch advances locally only. Branch and worktree cleanup are
separate from completing the issue; failed worktrees remain available for
diagnosis.

## Recovery

Read `status` and inspect the exact workflow run before retrying. Use Swamp's
workflow history and data commands to inspect persisted evidence, rather than
editing internal `.swamp/` files. Use `swamp help workflow history` and
`swamp help workflow resume` for the installed command schema. The current
behavior state is stored as the `state` resource; events preserve the transition
history.

| Situation                                                                 | Next action                                                                                                                           |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| A session ends during planning or implementation                          | Resume the recorded phase and existing worktree.                                                                                      |
| The user requests restart or rollback                                     | Use `rollback` for any step or `restart-agent` for a fresh worker; the request supersedes the old attempt without terminal-outcome proof. |
| Rollback is interrupted                                                   | Use `resume-rollback`, or supersede the pending recovery with a new explicitly requested rollback.                                     |
| A pane task timed out or needs input                                      | `drive` observes ordinary acknowledged timeouts; address dialogs deliberately, then `drive` resumes the original submission.            |
| Launch was rejected without resources, such as for unsupported effort     | Use `discard-agent` with the operator's reason and confirmation, then dispatch a new task with corrected configuration.               |
| Launch has an uncertain outcome, malformed response or retained resources | Inspect the session and saved response; automatic relaunch and discard remain blocked.                                                |
| A worker's completed response fails schema validation                     | Inspect the response and worktree; discard only that completed evidence with explicit confirmation before assigning replacement work. |
| Lifecycle state advanced but the final task-completion write failed       | Use `resume-agent` to reconcile the saved transition, then `refresh` if Agenda requirements changed.                                  |
| The Agenda description changes                                            | Use `refresh`, assess changed requirements and regenerate affected plan/review evidence.                                              |
| A review finds blockers                                                   | Repair the plan or implementation, then record a fresh review.                                                                        |
| Integration is waiting                                                    | Let the current integration finish; do not remove its lock file.                                                                      |
| Rebase conflicts                                                          | Inspect the retained worktree and resolve the conflict deliberately; record and review the result before retrying integration.        |
| The CI gate fails                                                         | Diagnose and fix the failure in the issue worktree; retain the failure evidence and retry with current review evidence.               |
| The integration process is interrupted                                    | Inspect Git and the persisted run to determine whether rebase, verification or merge completed before resuming.                       |
| The local merge landed but Agenda completion failed                       | Verify the recorded merge and retry close-out without merging again.                                                                  |
| The outcome does not meet the issue                                       | Keep the Agenda task open and record what remains.                                                                                    |

Never reset or discard a worktree to make retry appear clean. A conflicted
rebase, dirty worktree or changed target is evidence that needs inspection. An
empty check list or a cancelled run is not successful verification.

## Validation boundaries

Run the extension checks from `extensions/` with `deno task check`,
`deno task lint`, `deno task fmt` and `deno task test`. From the repository
root, run the local Git regression tests and installed-command smoke test
separately:

```sh
python3 -m unittest discover -s scripts/issue_lifecycle/unit -p 'test_git_ops.py'
python3 -m unittest discover -s scripts/issue_lifecycle/unit -p 'test_workspace.py'
python3 -m unittest discover -s scripts/issue_lifecycle/unit -p 'test_swamp_smoke.py'
```

The latter requires installed Swamp and Agenda commands and uses a disposable
repository and Agenda data directory.

The focused tests exercise state transitions, automatic role dispatch and plan
revisions, human handoffs, timeout observation, interrupted imports, stale
evidence, restart invalidation, Agenda and Yinz adapters, prompt correlation,
local Git locking and recovery. Agent adapter tests use an injected Yinz runner.
The installed Swamp smoke test drives planning through review to a handoff,
resumes an ordinary timeout on its original submission, and proves that resuming
the handoff does not submit duplicate prompts. Integration smoke tests use a
fake `cargo` executable to exercise
gate ordering and failure recovery without running Yinz's full Rust suites.
These checks do not establish that a live provider launched or that the full CI
gate passed for this feature. A separate live smoke check exercised the
installed Swamp binary through a visible Claude pane and returned correlated
JSON. That check does not cover every provider or a complete issue lifecycle.
The real full gate runs when the feature's local merge is authorized.

## Design origin

The source design is Swamp's `@swamp/issue-lifecycle` behavior and its
`.claude/skills/issue-lifecycle/SKILL.md`, with schemas in
`extensions/models/_lib/schemas.ts` and verification DAGs under `verification/`.
Its useful separation is preserved: typed behavior methods own issue state,
workflow DAGs orchestrate repeatable actions, and agent work supplies reviewed
plans and implementation evidence. See Swamp's `design/primitives/workflows.md`
and `design/primitives/behaviors.md` for those primitive boundaries.

Yinz replaces remote issue status and PR attestations with explicit Agenda
identity, local verification and a local merge receipt. It does not synthesize
review evidence for ad-hoc work or complete an issue whose outcome was not met.
