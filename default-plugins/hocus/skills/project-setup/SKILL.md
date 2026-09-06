---
name: project-setup
description: Bind a project-owned Swamp exit workflow and its local test gate.
---
Read the active project's AGENTS.md, CONTRIBUTING.md, build scripts, and test instructions.
Load the plugin tool family. Call project_config_read before proposing a write.
Choose the full local CI gate required by the repository, not only a focused test.
Verify the selected Swamp workflow exists with swamp workflow get and validate it before binding.
Resolve an existing control_checkout redirect before choosing where to write.
Propose .zellij/.swamp-workflows.kdl in the primary checkout with this shape:

```kdl
agent_exit {
    workflow "@hammz/zellij-agent-exit"
    ci "cargo" "x" "ci" "check"
    // default_branch "main"
    // agent kind="codex" model="..." effort="medium"
}
```

Use the command appropriate for this repository. CI is an argument vector.
For a shell command, use an explicit vector such as ci "sh" "-c" "make lint && make test".
The supplied workflow defaults to local main. Set default_branch only when the user needs another local target.
Use the supplied workflow's default work and separate review agents, or configure agent inputs in a custom workflow.
If the user requests effort, check the selected agent's supported values.
Do not configure permission bypasses. Reading this file must not run a command.
Explain the proposed command and settings. Call project_config_write with the complete source
and the exact previous source as expected, or null if the file was absent.
The tool presents the write for confirmation. If the source changed, read it again and propose
the updated change. Read back the resulting settings. Do not run CI, commit, or merge as part
of project setup unless the user separately requests those actions.
