---
name: yinz-notes-todos
description: Manage Yinz projects, guarded project configuration, durable notes, and todo trees. Use for task tracking, recorded findings, and linked project knowledge stored in Yinz.
---

Use `yinz action` to access the shared notes and todo store. A session must have
`hocus-store` loaded. Use `yinz --session NAME action ...` to select a session
explicitly. Without that option, normal Yinz session selection applies.

Discover projects with `project-list`. A project selector is a remembered name or
an absolute root path. Todo operations require `--project`, except `todo-search`,
which searches every project by default. Note creation requires `--project` or
`--global`. Note listing and search default to every scope.

Read existing notes and tasks before creating duplicates. Use subtasks for parts
of an existing task. Record durable decisions, findings, and constraints in notes,
then link them to relevant tasks. Mark completed work with `todo-done`.
Do not record credentials or temporary command output as durable notes.

Commands print JSON on success. Errors go to stderr and produce a nonzero exit
status. Check that status before parsing stdout. `project-remember` and
`project-forget` return JSON strings. Other successes return objects.

```sh
yinz action project-list
yinz action project-remember work --root /absolute/path/to/work
yinz action todo-list --project work --include-done
yinz action todo-search 'migration'
yinz action note-list --project work --include-archived
yinz action note-search 'migration'
```

Create a task and a subtask. These examples use `jq` to extract IDs from JSON.

```sh
task=$(yinz action todo-add 'Ship migration' --project work) || exit
id=$(printf '%s' "$task" | jq -r '.id')
yinz action todo-add 'Check rollback' --project work --parent-id "$id"

note=$(yinz --session main action note-add 'Migration decision' \
  --project work --author codex --tag design --tag database --todo-id "$id" \
  --body-file - <<'BODY'
# Decision

Keep the old column until all readers have migrated.
Verify rollback before removing it.
BODY
) || exit
note_id=$(printf '%s' "$note" | jq -r '.id')
yinz action note-get "$note_id"
```

`--body 'text'` and `--body-file PATH` are mutually exclusive. `--body-file -`
reads all stdin, including line breaks. Repeat `--tag` and `--todo-id` on
`note-add` to attach several tags or tasks. Tags on `note-edit` replace the whole
set. Omit them to preserve tags, or use `--clear-tags` to remove all tags.

Read revisions before replacing text, tags, or a todo's parent. Note reads return
`rev`. Todo listings return `rev` on each item, including subtasks.

```sh
current=$(yinz action note-get "$note_id") || exit
rev=$(printf '%s' "$current" | jq -r '.rev')
yinz action note-edit "$note_id" --title 'Migration and rollback' --clear-tags --expect-rev "$rev"

items=$(yinz action todo-list --project work --include-done) || exit
rev=$(printf '%s' "$items" | jq --argjson id "$id" -r \
  '.. | objects | select(.id? == $id) | .rev')
yinz action todo-edit "$id" --project work --text 'Ship verified migration' --expect-rev "$rev"
```

If a revision conflicts, read the current item again, reconcile the changes, and
submit the new text or parent with the new revision. Do not overwrite another
writer by blindly retrying. If the store does not reply within 30 seconds, the
command fails. A mutation may have completed, so read before repeating it.

```sh
yinz action note-link "$note_id" --todo-id "$id"
yinz action note-link "$note_id" --todo-id "$id" --unlink
yinz action note-list --tag design --todo-id "$id"
yinz action todo-done "$id" --project work
yinz action todo-done "$id" --project work --undo
yinz action note-archive "$note_id"
yinz action note-archive "$note_id" --undo
```

Completing a parent completes its subtree. Reopening a child reopens its ancestors.
Move a subtree with `todo-move ID --project work --parent-id PARENT --expect-rev REV`,
or use `--top-level` instead of `--parent-id`. Read a fresh revision first.
Moves preserve IDs, completion states, and note links. Open work reopens completed
ancestors. Moves across projects, cycles, and trees deeper than five levels fail.
Removing work does not complete its former parent.

Delete a leaf with `todo-delete ID --project work`. A parent requires
`--recursive`, which deletes its entire subtree and the affected note links in
one batch. Read the tree with `--include-done` to check its scope first. Notes
remain. `note-delete ID` deletes a note and its tags and links.
`project-forget NAME` removes the project name while preserving its notes and todos.
Use these commands only when deletion is part of the user's task.

Project lifecycle operations also work without chat:

```sh
yinz action project-rename work product
yinz action project-merge duplicate product
yinz action project-assign product --tab-id 2
yinz action project-open product
yinz action project-config-read product
yinz action project-config-write product --file config-change.json
```

Assignment/open/configuration require background `hocus-agentd`. The write file
contains `source` and `expected`: use the exact source returned by the read, or
null for an absent file. Stale writes fail. Merge preserves the target's settings
and moves linked records atomically; inspect both projects before merging.
Use `action context` and `list-tabs --json` to verify live associations.

Use `yinz action COMMAND --help` for the full flags. The skill is installed by
`yinz skills install yinz-notes-todos --agent claude|codex|opencode|all`.
The legacy `yinz hocus skills install` still installs this skill only.
`--dry-run` previews paths. `--force` explicitly replaces differing skill content.
