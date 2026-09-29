---
name: yinz-agents
description: Launch and coordinate coding agents in Yinz panes, inspect observed state, and search run history and predecessor chains through the Yinz CLI.
---

Use `yinz --session NAME action context` and `agent-list` to discover targets.
Agent operations require the background `hocus-agentd` plugin; archive queries
require `hocus-store`. Neither requires Hocus chat. Use explicit pane IDs for
input and controls. Register an existing agent with `agent-register PANE KIND`.

```sh
yinz --session main action agent-spawn --kind codex --project work   --branch feature --model MODEL --prompt-file prompt.md
yinz --session main action agent-send 3 --file follow-up.md --enter
yinz --session main action agent-control 3 interrupt
yinz --session main action agent-wait terminal_3 idle --timeout 120
```

Launch uses the shared worktree planner. Branch names normally create isolated
worktrees; collisions choose a fresh suffix. Inspect the returned branch and
path. `--reuse true` explicitly selects existing work. To run in a primary
checkout, deliberately set `--allow-outside-worktree true`. Model, effort,
permission mode, base, worktree directory, and Firecracker options are shown by
`agent-spawn --help`. `--file PATH` or `--file -` accepts the complete
AgentSpawnSpec JSON; explicit flags override file values. Prompts accept files/stdin.
Keep permission choices within the user's authorization.

State is an observation. `idle` does not prove task success. Read pane output,
run messages, and the resulting files/tests before reporting completion. A send
or control response says `dispatched`; verify the effect. A timeout or disconnect
can leave a mutation's result unknown. Read state before repeating it.

```sh
yinz --session main action agent-runs --project work --limit 20
yinz --session main action agent-run RUN_ID --from-revision 1 --limit 20
yinz --session main action agent-history-search 'rollback' --project work
yinz --session main action agent-predecessors RUN_ID
```

History defaults to the whole shared archive. Narrow with `--host-id` and
`--run-session` together when selecting one server's records. Use exact run IDs,
`next_offset` for run pages, and `next_revision` for timeline pages. `--revision`
reads around a search hit. Honor truncation and predecessor diagnostics; an empty
partial search is not proof that no history exists. Use native Git/GitHub tools
for repository work, and cron, Swamp, or agent automations for recurring execution.

For an SSH agent, verify an existing remote worktree first, then pass
`--remote-host HOST --cwd /absolute/remote/worktree --allow-outside-worktree true`.
The explicit override acknowledges that the local guard cannot inspect SSH
paths. Branch creation and project defaults operate on local checkouts; do not
combine them with SSH. Use existing pane SSH/warp commands to move a running pane.
