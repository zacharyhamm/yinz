# Upgrade from Hocus

This release removes the built-in Hocus chat agent. External agents, project panels, plugin authoring, and stateless LLM completions remain available.

1. Stop all Yinz servers that share the database.
2. Upgrade Yinz on every machine before you start any server. Mixed-version operation is unsupported.
3. Update custom configuration and plugins with the names below. There are no compatibility aliases.
4. Move endpoint, summary, and completion settings from `hocus` to the `agent-service` configuration block.
5. Move `width`, `height`, and `pad` to `agents`. Defaults remain `60%`, `66%`, and `3`. Remove `split`.
6. Run `yinz hooks install` to replace managed hooks. Existing managed entries are removed before new entries are added.

| Previous name | Current name |
| --- | --- |
| `hocus-agents` | `agents` |
| `hocus-agentd` | `agent-service` |
| `hocus-todos` | Removed |
| `hocus-new-agent` | `new-agent` |
| `hocus-store` | `project-store` |
| `hocus-proto` | `agent-proto` |
| `hocus-data` | `project-data` |
| `hocus-duplicate-agent` | `duplicate-agent` |
| `hocus-fork-agent` | `fork-agent` |
| `yinz hocus hooks` | `yinz hooks` |
| `yinz hocus codex` | `yinz agent codex` |
| `yinz hocus muse` | `yinz agent muse` |
| `yinz.hocus.complete` | `yinz.llm.complete` |
| `hocus_complete` / `hocus_complete_result` | `llm_complete` / `llm_complete_result` |

The store migrates `shared:hocus` to `shared:yinz` in one transaction. It also updates exact builtin owners and private namespaces. Retired chat records and schedules remain under `plugin:yinz:retired-chat`. They do not execute.

The migration preserves entity tables, rows, IDs, revisions, timestamps, indexes, links, import markers, and host/session ownership. Unrelated plugins keep their namespaces. If a destination entity already exists, the migration fails and rolls back. Resolve the collision before you retry. It never overwrites or merges records.

The Agents panel contains Agents. Use `Ctrl e a` to open it and `Ctrl o` to move it clockwise. Tab and Shift+Tab move between the tab bar and the active view. Esc dismisses the panel.

## Notes and todos removal

Notes and todos are no longer built in. Remove `todos` / `yinz:todos` entries from
custom plugin aliases, layouts, and `overlay_tab_plugins`, and remove scripts that
call `note-*` or `todo-*` actions. The project-note palette and notes/todos model
APIs are removed. Project commands and agent archives remain available.

Existing database tables, records, tags, links, import markers, and legacy files
remain on disk. Project renames, merges, and root changes no longer update these
retired records. No data deletion migration runs, and legacy todos are no longer
imported. Existing installed plugin files and external skills are not deleted;
remove obsolete copies yourself. The bundled `yinz-notes-todos` skill is removed;
project guidance is now in `yinz-sessions`.
