---
name: yinz-workflows
description: Discover, validate, inspect, and control project workflows through Yinz's managed Swamp bridge, including exact-run diagnostics and approvals.
---

Run `yinz --session NAME workflow ...` from the project's Git control checkout,
initialized as a Swamp repository (`swamp repo init --tool none` for a new one).
The background `swamp-bridge` plugin provides the same validated operations used
by background plugins. `yinz action context` reports service availability.
Use `workflow server status` for the managed server. Use native file tools for
ordinary repository work and external service tools for GitHub and web access.

```sh
yinz --session main workflow search
yinz --session main workflow get WORKFLOW
yinz --session main workflow schema WORKFLOW
yinz --session main workflow validate WORKFLOW
yinz --session main workflow evaluate WORKFLOW --file evaluation.json
yinz --session main workflow run WORKFLOW --json --input-file inputs.json
```

Evaluate accepts an arguments object such as `{"inputs":{}}`; run accepts the
input object itself, such as `{}`. `create NAME`
creates a definition to inspect. `edit WORKFLOW --file FILE` requires
`{"expected":COMPLETE_GET_RESULT,"definition":NEW_DEFINITION}`; preserve the ID.
`delete` requires the same `expected` snapshot. A stale definition fails instead
of overwriting someone else's work. Validate before launching.

```sh
yinz --session main workflow active
yinz --session main workflow history --workflow WORKFLOW
yinz --session main workflow run-get RUN_ID
yinz --session main workflow logs RUN_ID
yinz --session main workflow reports
yinz --session main workflow report REPORT_NAME --file selector.json
```

Inspect exact run IDs and the returned report/data selectors. `data --file FILE`
accepts a saved-data query with `predicate` and an optional bounded `limit`.
Discover schemas with `models`, `model`, `model-types`, `model-type`, and
`model-method`. Read the matching command's `--help` for positional selectors.

```sh
yinz --session main workflow approvals
yinz --session main workflow approve RUN_ID --file approval.json
yinz --session main workflow reject RUN_ID --file rejection.json
yinz --session main workflow resume RUN_ID --file resume.json
yinz --session main workflow cancel RUN_ID
yinz --session main workflow cancel-step RUN_ID WORK KEY
```

Approval/rejection arguments include `step` and optional `reason`. Resume
arguments include optional `inputs` and `from`. Answer a question with
`workflow answer RUN_ID MODEL QUESTION ANSWER`. Step cancellation requires the
bridge-owned work/key from the inspected lifecycle; never guess a process ID.
Closing a view does not cancel execution. Only approve, reject, cancel, or resume
when the user's task authorizes that change.

After an interrupted mutation, inspect definitions, exact run history, reports,
and submission state before retrying. Preserve launch and submission tokens for
reconciliation. New commands return one JSON value; failures use stderr and a
nonzero exit status. The existing `run` default also prints its launch token.
Use cron, Swamp workflows, or coding-agent automations for recurring execution.

Use `workflow show RUN_ID` to open a view and `workflow run-get RUN_ID` for structured inspection.
Workflow runs are distinct from agent turns and agent archive runs.
