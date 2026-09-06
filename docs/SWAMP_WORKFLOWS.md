# Project-owned Swamp workflows

Install Swamp and build Zellij with `cargo x build`. The local extensions require the
official Git model:

```sh
swamp extension pull @swamp/git
swamp workflow validate @hammz/zellij-agent-exit
```

The extensions stay under `@hammz` in this repository. Do not publish them. The manifest
is `extensions/manifest.yaml`. The Git extension adds methods with `export const
extension` and reuses the official commit and non-force worktree removal methods.

Put the binding in the control checkout's `.zellij/.swamp-workflows.kdl`:

```kdl
agent_exit {
    workflow "@hammz/zellij-agent-exit"
    ci "cargo" "x" "ci" "check"
    default_branch "main"
}
```

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
`--input` accepts a JSON object of declared workflow inputs. The supplied defaults use
Codex, a separate review agent, two repair rounds, `cargo x ci check`, and local `main`.
`agentCommand` and `agentArgs` select a custom agent command. `firecracker` and
`firecrackerImage` select a VM for the worker. Its host worktree remains the source for
review and integration.

The view shows authoritative run and child-run state, findings, revision identities,
test results, pending questions and retained command output. Close the view to detach;
reopen it with `show`. The supervised Swamp command continues independently. Cancel is
separate:

```sh
zellij workflow cancel RUN_ID
```

Cancellation stops active runs in the attached workflow family and preserves panes,
checkouts and VMs. It does not approve cleanup.

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

Recovery reuses the command claim. It does not accept another answer or launch a second
resume command. A resume command whose dispatch is unknown requires reconciliation.

## Evidence and recovery

Review and tests bind to the source commit, source contents, target commit, target
contents, gate argv and VM sync operation. Repairs, commits, rebases and target changes
invalidate earlier evidence. Final verification and fast-forward integration share both
checkout locks. The live VM must have no outstanding changes or sync conflicts.

A stale integration decision starts a new independent Swamp review cycle. New model and
operation identities prevent old results from satisfying the new cycle. The new cycle
requires another merge approval. After two unsuccessful repairs, the workflow asks for
instructions; after the instructed work completes, another review cycle begins.

The bridge stores only ownership, operation claims, event delivery and attachment state
outside Swamp. Ownership names the host, session, server incarnation, agent and work
item. Supervisor and process identities verify a pane before input. One prompt may be
outstanding per agent. Prompts are limited to 8000 UTF-8 bytes so the acceptance hook
can carry the complete correlation text. Prompt observation is registered before bytes are sent. Idle,
closed panes and process exit are not agent success.

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

## Verification

After reviewing and acknowledging local extension changes, run:

```sh
deno test --allow-all extensions
python3 extensions/tests/smoke.py
cargo x ci check
cargo x firecracker-test --build
cargo x firecracker-test --test
```

The smoke runner creates disposable repositories and stub agents. It tests unattended
questions, an interactive detachable view, stale evidence, both repair rounds,
recorded-answer recovery, cancellation, and server crash/adoption. It prints the
fixture directory containing its command transcript and terminal capture. The
Firecracker suite also runs the typed bridge against a real guest, including a sync
conflict. Run socket and KVM tests outside a sandbox that blocks those facilities.
