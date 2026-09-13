# Project-owned Swamp workflows

Install a compatible Swamp build (see Validation below) and build Zellij with
`cargo x build`. Workflow requests use a managed local `swamp serve`.
Show diff also requires `less` version 582 or later.
The local extensions require the official Git model:

```sh
swamp extension pull @swamp/git
swamp workflow validate @hammz/zellij-agent-exit
```

The extensions stay under `@hammz` in this repository. Do not publish them. The manifest
is `extensions/manifest.yaml`. The Git extension adds methods with `export const
extension` and reuses the official commit and non-force worktree removal methods.

Put the binding in the control checkout's `.zellij/swamp-workflows.kdl`:

```kdl
agent_exit {
    workflow "@hammz/zellij-agent-exit"
    ci "cargo" "x" "ci" "check"
    default_branch "main"
    rebase_prompt "Resolve and stage conflicts. Leave rebase continuation to the workflow, then exit."
}
```

The optional `rebase_prompt` must be a nonblank string. An explicit workflow input,
`rebasePrompt`, overrides it. If both are absent, the existing conflict-resolution
instructions apply. The launch captures this value and retains it through all continuations.

For resolution, the prepared prompt becomes the agent's startup argument through the
shared launcher. The bridge registers ownership before launch and waits for process exit.
It sends no second prompt. Fix and review helpers still receive their prompts after launch.
The resolver must stage conflicts and exit. The workflow runs `rebase --continue`.

The control checkout defaults to the primary Git checkout. To use another directory,
set `control_checkout "relative-or-absolute-path"` in the primary checkout's binding.
Relative paths resolve from the primary checkout. Put the binding, extensions, models,
workflow definitions and Swamp state in that control checkout. The working agent must
use a separate linked checkout of the same repository as the target.

Run from the working checkout inside the session:

```sh
zellij workflow run @hammz/zellij-agent-exit
zellij workflow show RUN_ID
zellij workflow show RUN_ID --json
```

`run` prints its launch token before dispatch. `--launch TOKEN` lets a caller retain a
chosen token. Identical requests return the existing claim; changed arguments with the
same token fail. `--no-view` starts an unattended run. It still suspends for questions.
`--input` accepts a JSON object of declared workflow inputs. Tests default to
`cargo x ci check`. The target defaults to local `main`. Both tests and agent review
are optional. No step pushes commits or creates a pull request.

The exit hook supplies the original terminal ID, agent kind, checkout, and VM context.
For a manual launch, pass `sourcePane` to enable the pane exit choices described below.
Without a source pane, the menu offers only Begin review and Leave.
`agentCommand` and `agentArgs` can select an explicit helper command.
VM-backed runs require the original pane identity. Preparation synchronizes that VM
without starting a worker. Local checkout evidence does not require a VM-sync record.

## Managed server lifecycle

Zellij starts one Swamp server per canonical control checkout and OS user. Sessions
share it through a private runtime directory and startup lock. A configured agent
prewarms the service after launch; other workflow requests start it on demand.
Reading project configuration starts no process.

```sh
zellij workflow server status
zellij workflow server restart
zellij workflow server stop
```

These commands use the current checkout's control binding. Stop and restart refuse
while Swamp reports active executions. Suspended runs survive both operations. A
server crash remains visible until an explicit restart; inspect history and reports
before deciding what to do with interrupted or uncertain operations.

The supervisor runs independently of the Zellij server. Closing a view, disconnecting
a client, or restarting Zellij does not stop Swamp. Each Zellij server multiplexes its
requests over one authenticated WebSocket per checkout. Read requests reconnect;
acknowledged executions reattach with their sequence cursor. An unacknowledged
mutation remains unknown until persisted state identifies its outcome. It is never
sent again automatically. Each continuation has its own correlation tag as well as
the shared launch token.

The endpoint binds an ephemeral loopback port, requires token authentication, and has
scheduling disabled. Bootstrap uses Swamp's local-encryption vault and service-token
commands. The machine-local vault definition is excluded through Git's local exclude
file. Endpoint credentials are stored with owner-only permissions under
`/tmp/zellij-swamp-UID/`; they never enter plugin messages, process arguments, or logs.
The supervisor rotates the seven-day credential daily through Swamp. A renewal
failure is reported without terminating active work.

Workflow YAML loads on each invocation. Before validating, launching or resuming
a workflow, Zellij compares extension contents with the running service's startup
fingerprint. Changed local extensions, imported helpers, pulled extensions and
configured extension sources trigger an automatic service refresh.

New launches wait while existing executions finish or suspend. The view shows
“Waiting for active workflows before loading updated extensions,” then “Loading
updated extensions.” History, cancellation and callbacks for existing executions
remain available. Continuations belonging to an active execution can finish on
that service. Suspended runs survive refresh and use the updated extensions when
resumed. Waiting does not consume the request's execution timeout.

Concurrent requests share one refresh per control checkout. Invalid extensions
produce a loading diagnostic before new work is dispatched. Run
`swamp doctor extensions --json` in the control checkout to inspect the problem.
`zellij workflow server status` reports extension freshness as `current`, `stale`
or `unavailable`. A crashed service still requires explicit recovery. Keep the
local extensions unpublished.

## Exit and review choices

1. When the local checkout is unchanged, choose **Restart**, **Fork**, or **Leave**, in that order.
   Unchanged means no staged, unstaged, untracked, or submodule changes, no paused rebase,
   and zero commits in `targetBranch..HEAD`. Ignored files do not count.
   The check uses the configured local target branch (`main` by default) without fetching.
   Clean branches equal to or behind that target receive this menu.
   Otherwise, choose **Restart**, **Fork**, **Begin review**, or **Leave**.
   Inspection errors and VM context retain these existing choices. The exit check does not synchronize VMs.
   Restart reruns the original command in the held pane. Fork opens the existing agent-kind picker in the
   same checkout and VM context. Cancel the picker to return to a fresh exit menu and checkout check.
   Each new exit run checks again. Its saved result determines the answers and both question and approval prompts.
   Leave closes the original exited terminal and its exit view. It preserves the
   checkout and other workflow panes. Manual launches without a source terminal can
   also complete with Leave.
2. After Begin review, choose **Show diff**, **Commit and rebase**, or **Leave**.
   Show diff displays unstaged changes to tracked files and untracked, non-ignored files after VM synchronization.
   Only the agent’s checkout is included, even when inherited Git settings point to another worktree.
   Untracked nested repositories appear as safely quoted directory entries. Their contents are not traversed.
   A persistent `less` viewer temporarily occupies the workflow pane, including for short or empty diffs.
   Empty diffs show “No unstaged or untracked changes.” Staged-only changes are excluded. Press `q` or `Esc` to return to the Begin review preparation menu.
   Arrow keys scroll, and `/` searches. Pager settings are overridden only for this viewer.
   Temporary output and key bindings are removed on exit. Command failures appear in the workflow view.
   Commit and rebase shows each Git command and its live output in a foreground pane.
   It skips empty commits and rebases onto the configured local target. After success
   or failure, press Enter to return to the workflow. A failed commit stops before rebase.
3. If a rebase is paused, choose **Resolve with agent** or **Leave paused**.
   Only approval starts a resolver. The agent stages resolutions. The workflow
   inspects the result and continues the rebase. Further conflicts require another
   decision. Other errors remain visible without automatic retries.
4. After preparation, choose **Run tests**, **Start code review**, **Merge**, or **Leave**.
   Passing tests return to this menu. Failed tests offer **Start fix agent**,
   **Return to menu**, or **Leave**. A fix agent receives the saved failure output.
   Its process exit returns to a fresh initial exit menu.
5. A selected reviewer inspects the checkout. The workflow compares commits and all
   tracked and untracked contents before and after the process exits. Changes return
   to the initial menu. An unchanged checkout proceeds to merge confirmation with
   the saved findings. Missing or malformed findings remain a failed review.
6. Confirm the exact source and target revisions. The prompt states whether tests
   and review passed, failed, or were skipped. You can explicitly accept failed or
   skipped checks and unresolved findings. Decline to preserve everything and end.
   Changed source or target state returns to preparation and requires fresh approval.
   Integration uses local fast-forward only and retains checkout locks, live VM
   verification, clean-state checks, and ancestry checks.
7. After integration, choose **Remove clean worktree** or **Keep**. Removal uses the
   existing guarded, non-force cleanup method. Changed state prevents removal.

On agent exit, the workflow uses the exit pane for status, questions and interactive
agents. The original terminal and its scrollback remain behind that view.
Shell commands use supervised panes; workflow execution needs no runner pane. Active agents share the exit
location in dispatch order. Other agents stay suppressed and continue their work.
For review and repair helpers, process exit restores workflow status. A completed
turn alone does not advance these decisions. Standalone agent-work remains available
to explicit callers and retains its turn-based observation.

Final results and errors stay visible unless the workflow closes the exit view through
Leave. Enter activates the selected button. **Restart agent** reruns the original
command and **Fork agent** opens the picker for a different agent in its place; both are
offered at every step, including while the workflow is still launching. Choosing either while
work is active ends the lifecycle: its running runs are cancelled, a run still being dispatched
is cancelled once it has an ID, later steps are refused, and a run that attaches afterwards is
cancelled too, preserving panes, checkouts and VMs. A cancelled fork picker cancels nothing, and
pressing Fork again while a picker opened by that button is open returns to it instead of opening
a second one; a picker the workflow's own menu opened is never reused. Restart closes a fork
picker that is still open.
Delete performs the same cleanup as Leave: it closes the original terminal and its exit
view. Delete is disabled while work is active.
The workflow view has no Detach button, and Esc does not close it.
Ordinary CLI launches keep their separate view.

The view shows authoritative run and child-run state, findings, revision identities,
test results, pending questions and retained command output. If a pane is closed
externally, the managed Swamp server continues independently. A missing view never
causes a replacement launch or a new visible pane. Use `show` to reopen a view.
Cancellation remains a separate action:

```sh
zellij workflow cancel RUN_ID
```

Cancellation stops active runs in the attached workflow family and preserves panes,
checkouts and VMs. Suspended runs remain persisted and resumable; cancellation does
not apply to them. It does not approve cleanup.

Diffs and test logs in the view use previews of up to 16,384 UTF-16 units.
Each shortened preview includes a command to retrieve the complete saved version.
Snapshots omit duplicate test logs from continuation history inputs. Saved evidence
does not change.

## Questions and answers

Questions are Swamp resources. Each includes a run ID, question ID, kind, prompt,
permitted answers and submission state. Kinds are choice, text, approval, instructions,
conflict, merge and cleanup. The view presents only pending questions. Submit through
the view or CLI:

```sh
zellij workflow answer RUN_ID MODEL_NAME QUESTION_ID ANSWER --submission TOKEN
```

The model validates the run and suspended approval step, then records the answer before
approval or resume. Duplicate, stale, invalid and wrong-run answers fail. The workflow
consumes only resume inputs that match the recorded answer and submission token.
Unattended runs never invent answers. Automatic answers must be declared by a custom
workflow, with an explicit policy appropriate to that workflow.

If delivery is interrupted after recording, inspect the question resource and recover
its existing submission:

```sh
swamp model get MODEL_NAME --json
swamp data query 'modelName == "MODEL_NAME"' --json
swamp model method run MODEL_NAME recover --input '{"run":"RUN_ID","question":"QUESTION_ID"}' --json
```

Recovery reuses the recorded submission and dispatch claim. It does not accept another
answer or start another resume. An uncertain dispatch requires reconciliation against
the persisted run and submission. Resume acknowledges dispatch before execution finishes
so the question method can release the model lock needed by the resumed workflow.

## Evidence and recovery

Review and tests bind to the source commit, source contents, target commit, target
contents, gate argv and VM sync operation. Repairs, commits, rebases and target changes
invalidate earlier evidence. Final verification and fast-forward integration share both
checkout locks. The live VM must have no outstanding changes or sync conflicts.

A stale integration decision starts a fresh preparation run. Each repeated menu and
agent-exit cycle has new run, model, question, and operation identities. Earlier
answers and results cannot satisfy the new decision. Automatic repair rounds are
removed. Apply these definitions to fresh runs. Do not resume a suspended older run
with the new decision sequence.

The bridge stores only ownership, operation claims, event delivery and attachment state
outside Swamp. Ownership names the host, session, server incarnation, agent and work
item. Supervisor and process identities verify a pane before input. One prompt may be
outstanding per agent. Prompts are limited to 256 KiB (262,144 UTF-8 bytes), including generated fix and review prompts.
Oversized prompts fail before a host claim is created. Repair instructions are never truncated.
Bridge requests travel as one JSON line on stdin.
Acceptance compares the expected SHA-256 digest with the host-computed digest when available.
Older hooks or stored requests without a digest use exact prompt text. Session and turn checks
still apply to completion. Prompt observation is registered before bytes are sent. Idle,
closed panes and process exit are not agent success.

Rebuild and restart with matching binary and plugin assets. Mixed versions and
unrelated older workflow definitions are not supported. Existing CLI-backed operation
records remain observable until their executions finish. New dispatches always use the
managed service; connection failure never selects a CLI fallback. A bridge
or server reload does not restore pane placement. Use a separate `show` view to inspect
the saved run.

Use the model's `observe`, `agent_inspect` and `reconcile` methods to inspect saved
operations. Never create a replacement mutation merely because a caller timed out.
An interrupted dispatch without a saved resource identity remains unknown. Resolve the
actual supervisor, checkout and VM before taking another action. Retained scrollback
can omit output older than the terminal's configured history limit; the resource marks
this limitation.

After a server crash, use `zellij pty-host list --json` to identify surviving local
supervisors. Adopt the saved supervisor with `zellij pty-host adopt SUPERVISOR --session
SESSION`, then reconcile the operation. For a remote pane, use its saved `remoteHost`
and `remotePaneId` with `pty-host adopt TOKEN --host HOST --session SESSION`. Adoption
preserves workflow ownership and clears obsolete plugin and client IDs. A prompt with
an event gap remains unknown; reconciliation does not send its bytes again.

The report includes resources saved earlier in the same run, including steps that
completed before suspension. Closing and reopening the view does not discard them.

After a method or workflow failure, read its report before changing or retrying it:

```sh
swamp report get @swamp/method-summary --model MODEL_NAME --json
swamp report get @swamp/workflow-summary --workflow WORKFLOW_NAME --json
swamp report get @hammz/workflow --workflow WORKFLOW_NAME --json
```

## Custom workflows and migration

Create reusable definitions with `swamp workflow create NAME`; keep the generated ID.
Use `@hammz/zellij` methods for host operations and the extended `@swamp/git` methods for
checkout mutations. Use separate model instances for independent work and long
observations. Connect saved outputs with `data.latest(...).attributes` CEL expressions.
Validate the definition before binding it. The supplied agent-work, review/integration
and question definitions are examples.

The old root `.zellij.kdl` is no longer read. Move its settings to the nested binding
file and add `workflow`. Invalid configuration remains readable for correction, but
workflow launch refuses it. Hocus `/project-setup` reads the previous source and writes
with an expected-source check. It cannot overwrite a concurrent edit.

## Hocus completions

The `@hammz/zellij` model's `hocus_complete` method requests one model turn from the
session's running Hocus daemon. It uses the model's `session`, `binary`, and
`timeoutMs` settings (30 seconds by default), plus workflow cancellation.
Configure the provider and credentials in Hocus. The method makes no automatic retries.
Cancellation stops the waiting CLI process. A provider request already sent by the
daemon can continue.

The inputs match `CompletionRequest`:

| Input | Meaning |
| --- | --- |
| `messages` | Required array of up to 200 messages with role `system`, `user`, `assistant`, or `tool` |
| Message fields | `content` defaults to empty, `tool_calls` defaults to `[]`, and tool messages require `tool_call_id` |
| `request_id` | Optional correlation string, empty by default |
| `system` | Optional system prompt prepended to the messages |
| `model` | Optional model override, subject to Hocus's pinned completion model |
| `tools` | Up to 64 provider tool specifications, empty by default |
| `tool_choice` | Optional JSON value passed to Hocus |
| `temperature`, `max_tokens` | Optional sampling and output limits |

Tool calls have `id`, `name`, and `arguments`. Arguments remain the raw JSON string
from the model. A request must contain a message or a system prompt. Its serialized
payload must fit within 64000 UTF-8 bytes.

For a model named `zellij-session`, use this task in a workflow step:

```yaml
task:
  type: model_method
  modelIdOrName: zellij-session
  methodName: hocus_complete
  inputs:
    messages:
      - role: user
        content: Explain this test failure in one sentence.
```

After the step succeeds, reference
`${{ data.latest("zellij-session", "completion").attributes.content }}` in a dependent
step. The same resource includes `session`, `request_id`, `tool_calls`, `model`,
`finish_reason`, `usage`, and an empty `error`. Unreported usage is `null`.
Reported usage contains `prompt_tokens`, `completion_tokens`, and `total_tokens`.
Retention is seven days and ten versions, matching the CLI result resources.

Command errors, malformed replies, mismatched request IDs, cancellation, and nonempty
reply errors fail the method without saving a completion. The workflow executes any
returned tools and sends the next round with the assistant calls and tool results.
Use separate model instances for concurrent completion steps to avoid overwriting the
latest output reference.

## Verification

### Managed server compatibility gate

Managed-server cancellation targets active executions only. Suspended runs remain
persisted and resumable. Cancellation of suspended runs is outside this migration.

Run the installed-binary compatibility probe before adoption:

```sh
deno run --allow-all extensions/tests/serve_compatibility.ts
# To test another build, append its absolute binary path.
```

The probe creates a private disposable repository, a local-encryption vault, and a
ten-minute service token. It starts an authenticated loopback server on an ephemeral
port with scheduling disabled. Credentials stay out of command arguments and output.
The probe stops its server and retains the fixture directory for inspection.
It uses Deno's native WebSocket client and does not load the project's extensions.

The installed `20260909.131252.0-sha.375d7eca` survives disconnects but fails the
inline-type resume check: its resume handler omits the direct-type resolver. It is
**not compatible** with these workflows. The probe passes against Swamp source commit
`7b11196336e5f67c5f3a81f7938dedede26b406c` with
[`swamp-serve-resume.patch`](../extensions/tests/swamp-serve-resume.patch), which reuses
Swamp's existing execution-service factory. Apply that patch to a separate Swamp
checkout, run `deno task compile --output /absolute/path/swamp`, and put the tested
binary on Zellij's `PATH`. The local acceptance build is
`/tmp/zellij-compatible-swamp-bin/swamp`; it does not replace the installed binary.

The installed-binary probe covers token authentication, readiness, detached execution,
stream reattachment, suspension, and inline-type resume. The managed lifecycle check
adds simultaneous startup, checkout isolation, private credentials, active stop/restart
refusal, persisted suspension, and explicit crash recovery:

```sh
deno run -A extensions/tests/managed_server.ts "$PWD/target/debug/zellij"
```

The automatic-refresh regression uses the real service through Zellij's request
layer. Build Zellij first and put a compatible Swamp binary on `PATH`:

```sh
ZELLIJ_TEST_BINARY="$PWD/target/debug/zellij" cargo x test -p zellij-process \
  -E 'test(refreshes_local_extensions_once)' --run-ignored all
```

It changes a local imported helper and its workflow while the service is active.
It checks waiting beyond the dispatch timeout, one execution after concurrent
refresh, resuming a persisted suspension and invalid extension diagnostics. Fixture
repositories and services are separate from the control checkout. Failures leave
the fixture directory available at the path shown in the test output.

### Existing workflow suites

Run:

```sh
deno test --allow-all extensions
python3 extensions/tests/smoke.py
python3 extensions/tests/smoke.py --exit-menu-only
python3 extensions/tests/smoke.py --source-diff-only
python3 extensions/tests/smoke.py --commit-rebase-only
python3 extensions/tests/smoke.py --rebase-only
cargo x ci check
cargo x firecracker-test --build
cargo x firecracker-test --test
```

The smoke runner creates disposable repositories and stub agents. It tests unattended
questions, an interactive workflow view, stale evidence, optional checks, informed merges, explicit conflict resolution,
recorded-answer recovery, cancellation, and server crash/adoption. It prints the
fixture directory containing its command transcript and terminal capture. The
`--exit-menu-only` path checks exact menus and approval prompts, rejected hidden choices,
fresh checks after Fork cancellation, and fallback choices for inspection errors and VM context. The
`--source-diff-only` path checks short, long, untracked, staged-only, and empty diffs in manual, agent-exit,
and unattended views, including dismissal, scrolling, search, checkout isolation, nested repositories, and temporary-file cleanup. The
Firecracker suite also runs the typed bridge against a real guest, including a sync
conflict. Run socket and KVM tests outside a sandbox that blocks those facilities.

### Measured results (2026-09-10)

Run `python3 extensions/tests/smoke.py --question-latency-only` with the compatible
Swamp build on `PATH`. The check records eight samples: one cold, then seven warm.
Exit latency ends when the first question renders. Answer latency starts at the public
`workflow answer` command and ends when the next question renders. Question lookup
is outside that interval. Both debug builds ran on the same Linux host with concurrent
build/test load. The small-sample p95 is descriptive, not a capacity benchmark.
[Raw samples and build identities](SWAMP_SERVER_MEASUREMENTS.json) are retained.

| Seconds | CLI baseline | Managed server |
|---|---:|---:|
| Cold exit → question | 3.051 | 6.128 |
| Warm exit → question, median | 3.341 | 1.135 |
| Warm exit → question, p95 | 3.864 | 1.435 |
| Cold answer → next question | 5.435 | 2.460 |
| Warm answer → next question, median | 5.998 | 2.607 |
| Warm answer → next question, p95 | 6.834 | 2.758 |

Warm median exit latency fell 66%; warm median answer latency fell 57%. Cold startup
includes vault, credential, and server bootstrap. The real workflow, source-diff,
recovery, and VM checks passed. The process audit recorded zero per-operation Swamp
CLI launches from Zellij or Swamp after bootstrap. Bootstrap and credential renewal
still use Swamp commands.

The crash checks kept a held Swamp execution alive across a Zellij server restart,
reattached it, and observed one completed run. Killing Swamp itself left an unavailable
execution that required explicit reconciliation; restarting did not create a duplicate.
Active cancellation targeted the run ID and kept the shared server alive. Suspended
runs remained persisted across stop/restart.

The final `cargo x ci check` passed formatting, lint, 7,884 unit tests, and 383
integration tests. All 11 extension tests and all four Firecracker checks passed.
The compatibility probe and managed lifecycle check also passed with the patched
Swamp build described above.
