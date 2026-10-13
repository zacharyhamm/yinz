# Yinz automation capabilities

Select a session with `yinz --session NAME`. The CLI and plugin SDK use the host
controller for live agent operations. Timeouts never trigger automatic retries.

| Topic | Canonical interface | Boundary |
|---|---|---|
| Live agents | `agent list/status/spawn/prompt/wait/read/recover/send-keys` | Readiness, conversation guards, acknowledgement, and turn outcomes. Use `--json`. `wait --submission ID` resumes a particular prompt; `wait --until idle` observes readiness. Project filtering is available on list. |
| Agent records | `agent register/runs/run/history-search/predecessors` | Registration uses `agent-service`. Archive queries use `project-store` and retain pagination and ownership filters. |
| Sessions and panes | `list-sessions`, `action list-panes/list-tabs/new-pane/new-tab/dump-screen/pane-wait`, pane and tab actions, `pty-host` | General programs, remote placement, output, exit, and closure waits. |
| Projects, notes, todos | Named project, note, and todo commands under `action` | Preserve domain rules and revisions from the original read. |
| Plugin data | `data catalog/fetch/count/apply` | Advanced fallback for records without domain commands. |
| Plugins | `plugins start/reload/stop`, `list --running`, authoring and validation | Direct session lifecycle. Use native file tools to inspect source. |
| Coding-agent skills | `skills list/show/install` | Six separate topics. Installation is explicit and does not replace customized skills without `--force`. |

The Agents panel’s summaries and inferred attention indicators are UI observations.
They do not establish readiness.
Use spawn, wait, then prompt. Automation spawn inputs reject `prompt` and `reuse_prompt`.
UI and SDK launch prompts remain supported. Guarded prompts support Claude Code, Codex, Muse,
Pi, and Antigravity (agy), subject to their reporting and completion prerequisites.
Retain prompt `submission_id` values to retrieve exact turn outcomes after dialogs or timeouts.
Submission records are bounded and kept in memory; they are not durable idempotency keys.
Use general pane commands for arbitrary programs, deliberate raw input, close, or restart.

Basic live operations do not require `agent-service` or `project-store`. Registration and archive access do.
`action agent-report` remains hook ingestion. The former `action agent-*` control commands,
`hocus skills install`, and `workflow show --json` are removed without aliases.

Native file, Git, web, and external-service tools remain appropriate for work outside
these Yinz interfaces.
