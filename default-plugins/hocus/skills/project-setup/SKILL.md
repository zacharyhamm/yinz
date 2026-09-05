---
name: project-setup
description: Configure the agent-exit CI gate and helper agent for this project.
---
Read the active project's AGENTS.md, CONTRIBUTING.md, build scripts, and test instructions.
Load the plugin tool family. Call project_config_read before proposing a write.
Choose the full local CI gate required by the repository, not only a focused test.
Propose .zellij.kdl with this shape:

```kdl
agent_exit {
    ci "cargo" "x" "ci" "check"
    // default_branch "main"
    // agent kind="codex" model="..." effort="medium"
}
```

Use the command appropriate for this repository. CI is an argument vector.
For a shell command, use an explicit vector such as ci "sh" "-c" "make lint && make test".
Omit default_branch unless remote HEAD metadata is absent or the user needs an override.
Omit agent settings to use the exiting agent's kind and its CLI defaults.
If the user requests effort, check the selected agent's supported values.
Do not configure permission bypasses. Reading this file must not run a command.
Explain the proposed command and settings. Call project_config_write with the complete source
and the exact previous source as expected, or null if the file was absent.
The tool presents the write for confirmation. If the source changed, read it again and propose
the updated change. Read back the resulting settings. Do not run CI, commit, or merge as part
of project setup unless the user separately requests those actions.
