# Hocus capability coverage

The CLI uses the selected session. Hocus chat remains available. File, repository,
web, and external-service work belongs to the coding agent's native tools.
New structured operations print one JSON value; errors go to stderr. A timeout or
disconnect leaves mutation results uncertain and never causes an automatic retry.

| Hocus tool | Coverage | CLI or replacement |
|---|---|---|
| `adopt_orphaned_pane` | Existing command | pty-host adopt |
| `agent_control` | New command | action agent-control |
| `agent_history_search` | New command | action agent-history-search |
| `agent_list` | New command | action agent-list |
| `agent_register` | New command | action agent-register |
| `agent_run` | New command | action agent-run |
| `agent_runs` | New command | action agent-runs |
| `agent_send` | New command | action agent-send |
| `agent_spawn` | New command | action agent-spawn |
| `clear_pane` | Existing command | action clear |
| `close_pane` | Existing command | action close-pane |
| `close_tab` | Existing command | action close-tab-by-id |
| `find_files` | Native replacement | Native file discovery |
| `focus` | Existing command | action focus-pane / go-to-tab-by-id |
| `git_log` | Native replacement | Native Git tools |
| `git_show` | Native replacement | Native Git tools |
| `github_read` | Native replacement | Native GitHub tools |
| `list_orphaned_panes` | Existing command | pty-host list |
| `list_other_sessions` | Existing command | list-sessions; select a session and list-panes |
| `list_session` | Existing command | action list-panes --all --json; action list-tabs --json |
| `move_pane` | Existing command | action move-pane |
| `move_panes_to_tab` | New command | action pane-move-to-tab |
| `new_pane` | Existing command | action new-pane |
| `new_remote_pane` | Existing command | action new-pane --remote-host |
| `new_tab` | Existing command | action new-tab |
| `note_add` | Existing command | action note-add |
| `note_archive` | Existing command | action note-archive |
| `note_delete` | Existing command | action note-delete |
| `note_edit` | Existing command | action note-edit |
| `note_get` | Existing command | action note-get |
| `note_link` | Existing command | action note-link |
| `note_list` | Existing command | action note-list |
| `note_search` | Existing command | action note-search |
| `openrouter_decisions` | Retained chat feature | Hocus model/tool-loop facility |
| `plugin_check` | Existing command | plugins check |
| `plugin_docs` | New command | plugins docs |
| `plugin_edit` | New command | plugins edit |
| `plugin_format` | Existing command | plugins format |
| `plugin_list` | New command | plugins list |
| `plugin_log` | Existing command | plugins log |
| `plugin_outline` | Native replacement | Native source inspection; plugins check --json |
| `plugin_read` | Native replacement | Native file reading; plugins dir |
| `plugin_reload` | Existing command, expanded | plugins reload; --headless for background operation |
| `plugin_search` | Native replacement | Native file search; plugins dir |
| `plugin_test` | Existing command | plugins test |
| `plugin_write` | Existing command | plugins write |
| `project_assign` | New command | action project-assign |
| `project_config_read` | New command | action project-config-read |
| `project_config_write` | New command | action project-config-write |
| `project_forget` | Existing command | action project-forget |
| `project_list` | Existing command | action project-list |
| `project_merge` | New command | action project-merge |
| `project_open` | New command | action project-open |
| `project_rename` | New command | action project-rename |
| `read_file` | Native replacement | Native file reading |
| `read_pane` | Existing command | action dump-screen --pane-id |
| `rename` | Existing command | action rename-pane / rename-tab |
| `rerun_pane` | New command | action pane-rerun |
| `resize_pane` | Existing command | action resize |
| `schedule` | Retained chat feature | Existing chat jobs remain; use cron, Swamp, or agent automations for new recurring tasks |
| `scroll_pane` | Existing command | action scroll-up / scroll-down / scroll-to-top / scroll-to-bottom |
| `search_files` | Native replacement | Native file search |
| `send_input` | Existing command | action write / write-chars |
| `set_floating_pane_geometry` | Existing command | action change-floating-pane-coordinates |
| `set_pane_pinned` | New command | action pane-pin / pane-unpin |
| `skill_disable` | Retained chat feature | Hocus-local skill prompts remain separate from portable CLI skills |
| `skill_enable` | Retained chat feature | Hocus-local skill prompts remain separate from portable CLI skills |
| `skill_list` | Retained chat feature | Hocus-local skill prompts remain separate from portable CLI skills |
| `skill_read` | Retained chat feature | Hocus-local skill prompts remain separate from portable CLI skills |
| `skill_write` | Retained chat feature | Hocus-local skill prompts remain separate from portable CLI skills |
| `slack_backfill` | Native replacement | Native service tools; existing chat tool remains available |
| `slack_post` | Native replacement | Native service tools; existing chat tool remains available |
| `slack_read` | Native replacement | Native service tools; existing chat tool remains available |
| `slack_search` | Native replacement | Native service tools; existing chat tool remains available |
| `slack_status` | Native replacement | Native service tools; existing chat tool remains available |
| `stack_panes` | Existing command | action stack-panes |
| `swamp_issue_comment` | Native replacement | Native service tools; existing chat tool remains available |
| `swamp_issue_create` | Native replacement | Native service tools; existing chat tool remains available |
| `swamp_issue_get` | Native replacement | Native service tools; existing chat tool remains available |
| `swamp_issue_search` | Native replacement | Native service tools; existing chat tool remains available |
| `swap_panes` | New command | action pane-swap |
| `todo_add` | Existing command | action todo-add |
| `todo_done` | Existing command | action todo-done |
| `todo_edit` | Existing command | action todo-edit |
| `todo_list` | Existing command | action todo-list |
| `todo_search` | Existing command | action todo-search |
| `toggle_pane` | Existing command | action toggle-fullscreen / toggle-pane-embed-or-floating |
| `tool_result_read` | Retained chat feature | Hocus model/tool-loop facility |
| `tools_load` | Retained chat feature | Hocus model/tool-loop facility |
| `unschedule` | Retained chat feature | Existing chat jobs remain; use cron, Swamp, or agent automations for new recurring tasks |
| `vm_changes` | Existing command | firecracker changes |
| `vm_close` | Existing command | firecracker close |
| `vm_list` | Existing command | firecracker list |
| `vm_sync` | Existing command | firecracker sync |
| `warp_pane_here` | Existing command | action warp-pane |
| `warp_tab_here` | Existing command | action warp-tab |
| `web_search` | Native replacement | Native web search |
| `workflow_active` | New command | workflow active |
| `workflow_answer` | Existing command | workflow answer |
| `workflow_approvals` | New command | workflow approvals |
| `workflow_approve` | New command | workflow approve |
| `workflow_cancel` | Existing command | workflow cancel |
| `workflow_cancel_step` | New command | workflow cancel-step |
| `workflow_create` | New command | workflow create |
| `workflow_data` | New command | workflow data |
| `workflow_delete` | New command | workflow delete |
| `workflow_edit` | New command | workflow edit |
| `workflow_evaluate` | New command | workflow evaluate |
| `workflow_get` | New command | workflow get |
| `workflow_history` | New command | workflow history |
| `workflow_launch` | Existing command, expanded | workflow run --json --input-file |
| `workflow_logs` | New command | workflow logs |
| `workflow_model` | New command | workflow model |
| `workflow_model_method` | New command | workflow model-method |
| `workflow_model_type` | New command | workflow model-type |
| `workflow_model_types` | New command | workflow model-types |
| `workflow_models` | New command | workflow models |
| `workflow_reject` | New command | workflow reject |
| `workflow_report` | New command | workflow report |
| `workflow_reports` | New command | workflow reports |
| `workflow_resume` | New command | workflow resume |
| `workflow_run_get` | New command | workflow run-get |
| `workflow_schema` | New command | workflow schema |
| `workflow_search` | New command | workflow search |
| `workflow_validate` | New command | workflow validate |

Additional CLI facilities: `action context`, `agent-predecessors`, `pane-wait`,
`agent-wait`, `data catalog/fetch/count/apply`, and `skills list/show/install`.
Extension-supplied Hocus tools remain chat features; their external operations
use the coding agent's native service tools unless the extension exposes its own CLI.
Conversation migration and model-settings administration are outside this interface.
