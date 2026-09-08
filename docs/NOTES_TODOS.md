# Notes and todos from the command line

Coding agents and shell scripts can use `zellij action` to read and update the
same notes and todo trees as Hocus and the Lua notes/todo panes. The selected
session must have `hocus-store` loaded. Data persists in the shared plugin store
and survives plugin reloads.

Use `zellij --session NAME action COMMAND ...` for an explicit session. Normal
session selection applies when `--session` is absent. Commands select one existing
store instance, even with multiple UI clients, and do not launch a plugin.
If no store is running, the command fails immediately. A missing reply fails after 30 seconds.
Mutations are never retried automatically. After a timeout, read the data before
repeating a write because it may have completed.

Success writes JSON to stdout. Failure writes a diagnostic to stderr and returns
a nonzero exit status. Project remember/forget return JSON strings. Other
operations return objects. Existing `zellij pipe --name store_query|store_mutate`
clients retain their raw replies. Set `"structured_reply":true` in a pipe request
to receive `{"ok":true,"data":...}` or `{"ok":false,"error":"..."}`.

## Commands

Arguments in capitals are positional unless prefixed with `--`. Run
`zellij action COMMAND --help` for flag details.

| Command | Arguments and options |
| --- | --- |
| `project-list` | None |
| `project-remember` | `NAME --root ABSOLUTE_PATH` |
| `project-forget` | `NAME` |
| `todo-list` | `--project PROJECT [--include-done]` |
| `todo-search` | `QUERY [--project PROJECT]` |
| `todo-add` | `TEXT --project PROJECT [--parent-id ID]` |
| `todo-edit` | `ID --project PROJECT --text TEXT [--expect-rev REV]` |
| `todo-done` | `ID --project PROJECT [--undo]` |
| `todo-delete` | `ID --project PROJECT [--recursive]` |
| `todo-move` | `ID --project PROJECT (--parent-id ID or --top-level) [--expect-rev REV]` |
| `note-list` | `[--project PROJECT or --global] [--include-archived] [--tag TAG] [--todo-id ID]` |
| `note-search` | `QUERY [--project PROJECT or --global]` |
| `note-get` | `ID` |
| `note-add` | `TITLE (--project PROJECT or --global) (--body TEXT or --body-file PATH) [--tag TAG]... [--todo-id ID]... [--author AUTHOR]` |
| `note-edit` | `ID [--title TITLE] [--body TEXT or --body-file PATH] [--tag TAG]... [--clear-tags] [--expect-rev REV]` |
| `note-archive` | `ID [--undo]` |
| `note-delete` | `ID` |
| `note-link` | `NOTE_ID --todo-id TODO_ID [--unlink]` |

Projects can be selected by remembered name or absolute root. Forgetting a
project preserves its notes and todos. Project-bound todo operations require an
explicit project. Todo search defaults to every project and includes completed
items. Note reads default to all scopes. Note lists hide archived notes unless
`--include-archived` is set. Note search uses the existing search across archived
and active notes.

Repeat `--tag` and `--todo-id` on note creation. Tags on an edit replace the whole
set. Omit tags to preserve them, or use `--clear-tags` to remove them explicitly.
`--clear-tags` and `--tag` cannot be combined. Use `note-link` to change links.
A note edit must change at least one of title, body, or tags.

## Multiline bodies and revisions

`--body-file -` reads all stdin. A file path reads the whole UTF-8 file. This input
is mutually exclusive with `--body`. Store validation and size limits still apply.
Redirecting stdin for another named action does not send extra requests.

```sh
zellij --session main action note-add 'Rollback decision' --project work \
  --tag migration --author claude --body-file - <<'BODY'
# Rollback

Keep the old column until every reader has migrated.
BODY
```

`note-get` and todo listings return each item's `rev`. Read that revision before
composing a replacement. Pass it as `--expect-rev` on note edits, todo text edits,
and moves. The note guard covers tag replacement as well as text. If another
writer changes the item, the batch fails without applying its changes. Read the
current version, reconcile the intended change, and submit its new revision.
Do not blindly retry a conflict.

## Todo deletion and moves

A leaf can be deleted directly. A parent requires `--recursive`. Deletion removes
the subtree and its note links in one batch while preserving the notes themselves.
Read the complete tree with `--include-done` before deleting a subtree.

Moves change the selected subtree's parent within the same project. IDs,
completion states, and note links remain intact. Moving open work under completed
ancestors reopens those ancestors. Removing work does not complete its old parent.
Missing items, parents in another project, cycles, and moves beyond the five-level
depth limit fail. Cross-project moves are not supported.

## Install the coding-agent skill

The binary includes the portable `zellij-notes-todos` skill. Install it explicitly:

```sh
zellij hocus skills install --dry-run
zellij hocus skills install
zellij hocus skills install --agent claude
zellij hocus skills install --agent codex
zellij hocus skills install --agent all --force
```

The default installs for both agents. Claude uses
`~/.claude/skills/zellij-notes-todos/SKILL.md`. Codex uses
`${CODEX_HOME:-~/.codex}/skills/zellij-notes-todos/SKILL.md`. An empty `CODEX_HOME`
uses the default. Identical content is a successful no-op. Differing content
requires `--force`. Other files in the skill directory are preserved.
`--dry-run` checks destinations without writing.

Installation does not change hooks or project-local skill directories. These
coding-agent skills are separate from [Hocus skills](SKILLS.md).
The [bundled skill](../assets/skills/zellij-notes-todos/SKILL.md) includes command
examples, JSON handling, linked subtasks and notes, and conflict recovery.
