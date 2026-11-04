# Yinz automation capabilities

Select a session with `yinz --session NAME`. The CLI and plugin SDK use the host
controller for live agent operations. Timeouts never trigger automatic retries.

| Topic | Canonical interface | Boundary |
|---|---|---|
| Live agents | `agent list/status/spawn/prompt/wait/read/recover/send-keys` | Readiness, conversation guards, acknowledgement, and turn outcomes. Use `--json`. `wait --submission ID` resumes a particular prompt; `ready` allows text submission, including busy Claude steering; `wait --until idle` awaits all tracked activity. Project filtering is available on list. |
| Agent records | `agent register/runs/run/history-search/predecessors` | Registration uses `agent-service`. Archive queries use `project-store` and retain pagination and ownership filters. |
| Sessions and panes | [`attach --remote HOST SESSION`](REMOTE_SESSIONS.md), `list-sessions`, `action list-panes/list-tabs/new-pane/new-tab/dump-screen/pane-wait`, pane and tab actions, `pty-host` | General programs, remote placement, output, exit, and closure waits. |
| Projects | Named project commands under `action` | Preserve domain rules and guarded configuration writes. |
| Plugin data | `data catalog/fetch/count/apply` | Advanced fallback for records without domain commands. |
| Plugins | `plugins start/reload/stop`, `list --running`, authoring and validation | Direct session lifecycle. Use native file tools to inspect source. |
| Coding-agent skills | `skills list/show/install` | Four separate topics. Installation is explicit and does not replace customized skills without `--force`. |

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

## Clipboard history

Yinz appends clipboard submissions to the `clipboard_history` entity in
`shared:yinz`, stored in the data directory's `plugin-data.db`. Selection copies,
explicit copy actions, plugin copies, and terminal OSC 52 writes are included.
Repeated and empty values are retained without automatic pruning.

Records contain `copied_at_ms`, `host_id`, `yinz_session_name`, `destination`
(`system`, `primary`, or `command`), and `content` as a blob of UTF-8 bytes,
including embedded NULs. The existing `data catalog/fetch/count` commands can
inspect the records; no clipboard browser is provided.

Command copies are recorded after their stdin transfer completes, allowing
clipboard owners such as xclip to remain alive. Known spawn and transfer failures
are omitted. OSC 52 copies are recorded after output submission; terminals do not
acknowledge delivery. Persistence errors are logged without failing the copy.

## Tab groups

Create a group with a fresh tab and capture its stable ID:

```sh
group_id=$(yinz action new-tab-group --name work)
yinz action list-tab-groups --json
yinz action go-to-tab-group-by-id "$group_id"
yinz action rename-tab-group review --group-id "$group_id"
yinz action close-tab-group-by-id "$group_id"
```

Creation focuses the new group and prints only its group ID.
IDs are unsigned 32-bit numbers, including zero.
Closing a group closes all its tabs and uses the existing session quit guard.

Use `go-to-tab-group-name NAME --create` to select a group or create it when missing.
Name lookup matches the displayed name exactly.
When multiple groups have the same displayed name, lookup selects the lowest ID.
This command prints an ID only when it creates a group.
Project-derived names can override stored names in the display and in name lookup.

`list-tab-groups` prints a table.
With `--json`, it prints an array with `id`, `name`, `active`, and `tab_ids` for each group.
Groups are ordered by ID. Tab IDs follow the group's tab order.
`current-tab-group-info` prints the current group's name and ID.
With `--json`, it prints one record with the same fields.

`rename-tab-group NAME` and `close-tab-group` act on the current group.
Both accept `--group-id ID`.
The explicit forms are `rename-tab-group-by-id ID NAME` and `close-tab-group-by-id ID`.

Use `go-to-next-tab-group` and `go-to-previous-tab-group` to cycle through groups.
Each group remembers its focused tab.
Use `go-to-tab-in-group INDEX` to select a tab by its one-based position in the current group.
Use `move-pane-to-next-tab-group` or `move-pane-to-previous-tab-group` to move the focused pane and follow it.

## Pane and tab transfers

`action warp-pane` and `action warp-tab` negotiate semantic terminal state with
the destination. Supporting sessions preserve both screen buffers, normal-screen
history, cursors, scroll margins, input modes, hyperlinks, and palette associations.
Hidden companion panes use the same transfer. The terminal is restored at its
source dimensions and then resized to its destination content area before buffered
output and live reads resume. Destination pane IDs and placement remain local.

If the destination lacks support, the terminal has graphics or incomplete state,
or a snapshot exceeds the transfer budget (4 MiB per pane, shared across a tab's
32 MiB allowance), Yinz uses its bounded ANSI text transfer. The command/plugin
result and server log warn which pane was affected and why. This fallback loses
terminal modes and alternate-screen semantics; semantic snapshots are never
truncated. No additional confirmation is required. An invalid incoming semantic
snapshot rejects adoption and invokes source recovery. Failed adoption returns
consumed local buffered output for replay after the source snapshot, while remote
brokers replay from the original transfer offset.

This preserves future transfers. It cannot recover rows already lost from a
damaged pane, and fullscreen application transcripts remain application-owned.
