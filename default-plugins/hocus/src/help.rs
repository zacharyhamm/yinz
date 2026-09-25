//! `/<name> --help`: what a command does, written for the person who types it.
//!
//! Two sources, deliberately kept apart. The **prose** below is hand written for
//! a human and is the only text in the plugin written that way. The
//! **structure** — the usage line, the argument table, the enum lists — is
//! derived from `tools::specs()`, so it cannot drift from the schema a tool is
//! actually dispatched against.
//!
//! The prose is not the tool's `description` from `specs()` and must never
//! become it. Those descriptions go out on every request and every tool round,
//! so they are held near the byte floor on purpose (see the note on
//! `tools::Family`); a paragraph that explains a tool to a person would be a
//! standing tax on every conversation. The same split `tools::consequence`
//! already makes for the confirmation prompt, one room further along.
//!
//! Every paragraph here is written in ASD-STE100 Simplified Technical English:
//! active voice, one topic per sentence, imperative for an instruction, no
//! `-ing` verbs, articles kept, and one word for one meaning throughout.

use serde_json::Value;

use crate::extensions::{self, Safety};
use crate::tools;

/// Help for the plugin's own commands, which have no schema.
///
/// The first line of each entry is its usage line, because nothing else can
/// supply one: these are not tools and `parse_command` never sees them.
const COMMAND_HELP: &[(&str, &str)] = &[
    (
        "help",
        "/help\n\nShow every command in groups: hocus's own commands, the skills that are loaded, and every tool by its family. A bare / does the same. Type /<name> --help to read about one command.",
    ),
    (
        "history",
        "/history\n\nShow the saved chats, the newest first, up to fifty of them. Every session sharing this archive is included, not this session alone. A dot marks a chat that still runs. Select a chat to open it again. The chat on the screen stays in its tab.",
    ),
    (
        "models",
        "/models\n\nShow the models you can use. The list opens on the model in use now. Select a model to change to it; the change applies to the next message. The change holds for this session only: hocus reads the model from your config and your environment again at every start.",
    ),
    (
        "stop_jobs",
        "/stop_jobs\n\nCancel all the repeated prompts that the schedule tool armed, in this chat and in the other chats. With no job armed, the command shows nothing at all. Use unschedule to cancel one job.",
    ),
    (
        "keys",
        "/keys\n\nShow all the keys the panel obeys. The chat has no help line, thus this is the only list of them.",
    ),
    (
        "plan",
        "/plan <what to plan>\n\nAsk the model for a plan before it does the work. While the model makes the plan, it can only read: it cannot change your session. A tool that you type still runs, because a typed command is a person who acts. Read the plan, then type /execute to approve it, or press n to keep refining it. Ctrl+l leaves the plan behind in its own tab. A tool call that waits for a y/n is declined when the plan starts. A prompt is necessary; /plan on and /plan off do not exist.",
    ),
    (
        "execute",
        "/execute [--clear] [note]\n\nApprove the plan that waits for an answer, and let the model start the work. Text after the command changes the approval. Use --clear to move the plan into a new chat first. The command refuses when hocus has written no plan yet, and when there is no plan at all.",
    ),
    (
        "clear",
        "/clear\n\nStart a new chat. A chat that holds messages, jobs or work of its own stays in a tab; an empty chat is reset where it is, and no tab opens. Ctrl+l does the same.",
    ),
    (
        "skills",
        "/skills [reload | enable <user|project> <name> | disable <user|project> <name>]\n\nShow the skills that are loaded, then the skills that hocus refused, and why. Use reload to read the skill folders again; there is no file watcher. Use enable or disable with a scope and a name, which is three words exactly, to change one copy of a skill. To run a skill, type /<skill> <text>.",
    ),
];

/// Help for the tools, in the order `tools::specs()` declares them, so the two
/// files can be read beside each other.
///
/// A paragraph says what the argument table cannot: what the tool does, what it
/// refuses, and what it silently does not do. Defaults recorded in the schema are
/// left to the table, which reads them from `specs()` and cannot be wrong about
/// them.
const TOOL_HELP: &[(&str, &str)] = &[
    ("workflow_search", "Find workflow definitions in the active project’s control checkout. Start here, inspect with workflow_get, and fetch workflow_schema before authoring."),
    ("workflow_get", "Read a definition, complete source.content, and source ownership. Prepare replacement definitions from source.content to preserve all fields. Pass the complete result as expected to workflow_edit or workflow_delete. A source change causes the write to be refused."),
    ("workflow_schema", "Read the authoring schema before creating or editing. When workflow is supplied, read the inputs schema for that definition."),
    ("workflow_create", "Create a Swamp scaffold with an assigned ID. Read it with workflow_get, then edit and validate it. Never invent or replace its ID."),
    ("workflow_edit", "Replace a definition using the complete inspected result as expected. Swamp preserves its ID. Repository extension edits become project overrides. Downloaded definitions are refused."),
    ("workflow_delete", "Delete the inspected project definition and its run history through Swamp. Deleting a project override reveals the extension definition again. Extension source files cannot be deleted through this operation."),
    ("workflow_validate", "Check the saved DAG before execution. Validation failures return diagnostics without launching work. Diagnose, edit, validate, then resume the exact failed run."),
    ("workflow_evaluate", "Resolve expressions with workflow inputs. Evaluation can save state, so it needs confirmation and is unavailable in planning mode."),
    ("workflow_active", "Read active executions in the control checkout. Use the returned run UUID to inspect or cancel one."),
    ("workflow_history", "Search past workflow runs by name, query, or a CEL filter over status, inputs and tags. Read an exact result with workflow_run_get."),
    ("workflow_run_get", "Inspect one exact run, including bridge-owned children, questions and versioned output references. Inspect reports and logs after failure before editing or resuming."),
    ("workflow_logs", "Read a bounded tail of run logs. Use contains to focus on a step name or diagnostic. Use tool_result_read to recover retained text without another server read."),
    ("workflow_reports", "Find stored reports by workflow, model, or query. Use workflow_report to inspect one. Reports provide diagnostics even when execution fails."),
    ("workflow_report", "Read a stored report. Select a version or variant when available. After a failure, read its workflow summary and method summary before retrying."),
    ("workflow_data", "Query versioned outputs with a CEL predicate. Select exact modelName, name and version, or tags.workflowRunId. Use select to project fields or slice long text."),
    ("workflow_models", "Search models available to workflow steps. Inspect the model and method schema before authoring a model-method step."),
    ("workflow_model", "Read one model definition and state without invoking any model method."),
    ("workflow_model_types", "Search installed model types without installing extensions or changing models."),
    ("workflow_model_type", "Read the installed type description, method list, and schemas before choosing a workflow step."),
    ("workflow_model_method", "Read a specific method schema for an existing model. This does not execute the method."),
    ("workflow_launch", "Validate and launch through the managed bridge. The response identifies the launch and run. If delivery is uncertain, inspect history before another launch."),
    ("workflow_resume", "Validate and resume one exact run, optionally changing inputs or its starting step. Owned launch and session identity cannot change. Closed lifecycles are refused."),
    ("workflow_approvals", "Read the steps waiting for approval. Inspect the run before approving or rejecting a step."),
    ("workflow_approve", "Approve a named step on an exact inspected run. This decision does not itself resume execution."),
    ("workflow_reject", "Reject a named step on an exact inspected run. The run ID prevents a decision from applying to a newer run."),
    ("workflow_answer", "Submit an answer through the question model which owns the inspected question. The model records the answer, approves the step and resumes execution. Inspect submission state after uncertainty."),
    ("workflow_cancel", "Cancel only the active run identified by the supplied UUID through the bridge. The managed server cancels active executions only. Suspended runs remain persisted."),
    ("workflow_cancel_step", "Interrupt an owned active step using the run, work and key from workflow_run_get. Supervisor and process ownership are verified before signalling. Inspect the operation until termination is verified."),

    (
        "list_session",
        "Show the tabs and the panes of this session, with their IDs. Almost every other pane tool needs an ID from this list. A pane ID and a plugin ID can be the same number, thus the list also gives the kind of each pane.",
    ),
    (
        "list_other_sessions",
        "Show the other zellij sessions on this machine, with each visible pane ID and its is_plugin value. The list gives the tab_id of each tab as well as its number in the tab bar; the tools take the tab_id. Use warp_pane_here or warp_tab_here to move one of those panes or tabs into this session.",
    ),
    (
        "warp_pane_here",
        "Move a visible pane and its hidden contents into this session. Selecting a stack member moves the whole list. Terminal programs continue with their scrollback. Plugin instances restart. Hidden pane IDs cannot initiate a warp. Use list_other_sessions first to get the session name and the pane ID. A session that gives away its last tab stops.",
    ),
    (
        "warp_tab_here",
        "Move one whole tab out of a different session and into this session. Every pane in the tab comes with it. Use list_other_sessions first to get the session name and the tab ID, which is not the number in the tab bar. A session that gives away its last tab stops.",
    ),
    (
        "list_orphaned_panes",
        "Show the panes that stayed alive after their session stopped. Each one has a host ID. Use adopt_orphaned_pane to bring one into this session.",
    ),
    (
        "adopt_orphaned_pane",
        "Bring one pane from list_orphaned_panes into this session. The pane keeps the program that runs in it.",
    ),
    (
        "read_pane",
        "Read the text in a pane. The tool gives you the visible screen only, unless you set full_scrollback to true. The full scrollback of a busy pane can be very long.",
    ),
    (
        "send_input",
        "Send text to a pane, and then the named keys, in the given sequence. The program in the pane receives the text as if you typed it. Most prompts need the enter key after the text. Give text, or keys, or both: a call with neither is an error.",
    ),
    (
        "new_pane",
        "Open a new pane. Without a command, the pane opens your shell in the cwd of the focused pane. With a command, the pane runs that command, and rerun_pane can start it again after it exits. In a Firecracker VM the cwd is a path in the guest, thus hocus never uses the host cwd there: leave the cwd unset to land in the working directory of the VM. With in_new_tab and no command, hocus opens the tab only, and the answer gives a tab ID and no pane ID.",
    ),
    (
        "new_remote_pane",
        "Open a pane whose program runs on a different machine through ssh. The remote machine needs a zellij of the same version on its PATH. Give remote_host as a host name, as an alias from your ssh config, or as user@host. Without a command, the pane opens the login shell of that machine. Without a cwd, the pane opens the remote home directory.",
    ),
    (
        "new_tab",
        "Open a new tab. Without a cwd, the new tab uses the cwd of the focused pane.",
    ),
    (
        "focus",
        "Move the focus to one pane or to one tab. Give a tab, or a pane; a pane needs pane_id and is_plugin together. A call that gives neither is an error. A pane that you cannot see becomes a floating pane, so that the focus is where you can see it.",
    ),
    (
        "rename",
        "Change the name of one pane or of one tab. Give a tab, or a pane; a pane needs pane_id and is_plugin together. There is no focused-pane default: a name on its own is an error.",
    ),
    (
        "move_pane",
        "Move one tiled pane to the position of the neighbor in the given direction. Without a direction, the pane moves to the next position in the layout. The tool refuses a direction that has more than one neighbor, and a direction that has none. It also refuses while the tab is full screen, and while the floating panes are on the screen. For two named panes, use swap_panes. For a floating pane, use set_floating_pane_geometry.",
    ),
    (
        "swap_panes",
        "Exchange the positions of two tiled panes that touch each other. Give exactly two panes, and give each one as a pane_id with an is_plugin. Both panes must be in one tab, and the edge between them must have one pane on each side. The tool refuses while the tab is full screen, and while the floating panes are on the screen.",
    ),
    (
        "resize_pane",
        "Make a pane larger or smaller. Without a direction, the pane changes on all its sides.",
    ),
    (
        "toggle_pane",
        "Change a pane between full screen and its usual size, or between floating and tiled. The tool reverses the current state.",
    ),
    (
        "move_panes_to_tab",
        "Move one or more panes into a different tab. Give each pane as a pane_id with an is_plugin. Give a tab_id for a tab that exists, or set new_tab to true for a new tab, but never both; new_tab_name belongs to a new tab only. The tab_id is the ID from list_session, not the number in the tab bar.",
    ),
    (
        "stack_panes",
        "Put two or more panes into one stack. A stack shows one pane at a time and keeps the other panes as title bars. Give at least two panes, and give each one as a pane_id with an is_plugin.",
    ),
    (
        "set_pane_pinned",
        "Keep a floating pane on the screen when you change tabs, or stop that behavior. The pane must float.",
    ),
    (
        "set_floating_pane_geometry",
        "Set the position or the size of a floating pane. Give each value in cells (for example 80) or in percent (for example 50%). A percent above 100 is an error, and a width or a height of zero is an error. The tool changes only the values you give, and a call that gives none of them is an error.",
    ),
    (
        "rerun_pane",
        "Start the command of a command pane again, after that command exits. The tool refuses a plugin pane. On a shell pane, and on a command that still runs, the tool changes nothing and still answers rerun.",
    ),
    (
        "scroll_pane",
        "Scroll a terminal pane by one line, by one page, or to its top or its bottom. On a plugin pane the tool changes nothing and still answers scrolled.",
    ),
    (
        "clear_pane",
        "Erase a terminal pane. Its scrollback goes, the text on the screen goes with it, and its colors, its cursor and its character set go back to the start. The program in the pane continues. You cannot recover the erased text. A pane that shows a full-screen program, such as vim or less, keeps everything: the tool changes nothing and still answers cleared.",
    ),
    (
        "close_pane",
        "Close a pane and stop the program in it. Unsaved work in that program is lost.",
    ),
    (
        "close_tab",
        "Close a tab and every pane in it. The tab ID is necessary: there is no focused-tab default.",
    ),
    (
        "web_search",
        "Search the web. The answer holds short extracts, not the full pages, and it gives eight results at the most. The search needs a SearxNG instance that answers in JSON: hocus uses search_url from its config block, or the SEARXNG_URL variable, or http://localhost:8888.",
    ),
    (
        "openrouter_decisions",
        "Evaluate named questions against the supplied state through OpenRouter Decisions. Use choice for options, score for ordered criteria, and noul for a boolean likelihood from 0 to 1. Set OPENROUTER_API_KEY before you start the Zellij session. The default model is typesafe/jev-1.13. The model argument overrides the Decisions model for one call. Configure the endpoint and credentials through decisions_url and decisions_api_key_env, not tool arguments. Answers include usage and cost, separate from chat usage. Ctrl+c stops the wait and discards a late response. Requests are not retried automatically.",
    ),
    (
        "schedule",
        "Send a prompt again and again, after each interval, until you cancel the job. The shortest interval is five seconds. Use unschedule or /stop_jobs to cancel it. The job refuses a tool that needs approval, unless you set auto_confirm.",
    ),
    (
        "unschedule",
        "Cancel one repeated prompt. Without a job ID, the tool cancels all of them. The tool reaches the jobs of your other chats as well as the jobs of this one.",
    ),
    (
        "project_list",
        "Show the projects, their roots, their saved worktree_dir settings, their worktrees, their tabs, and which project is active. The file tools and the git tools all work in the active project.",
    ),
    (
        "project_assign",
        "Make a tab belong to a project. A project that does not exist needs an absolute root path. Without a tab ID, the tool takes the tab of the agent you looked at last, and the active tab after that. The project becomes the active one only when that tab is the active tab. The file tools and the git tools all work in the project of the active tab. Set worktree_dir for future worktrees. Omit it to keep the setting, or give an empty string to clear it. Launch overrides take precedence, then this setting, then .worktrees/. Zellij appends the branch name. Relative paths resolve against the primary checkout, including sibling paths. Absolute paths are also accepted. Shell variables and ~ are not expanded. NUL bytes are rejected. Existing worktrees stay where they are.",
    ),
    (
        "project_forget",
        "Remove a project from the list. Every tab that belongs to it loses the project, thus the session can be left with no active project. The todos and the notes of the project stay. A project that hocus found by itself comes back while an agent runs in it.",
    ),
    (
        "project_rename",
        "Change a project's name with name=old new_name=new. The new name must be unused. The root and settings stay the same. The project becomes user-managed, so discovery keeps its name and root.",
    ),
    (
        "project_merge",
        "Combine two projects with source=duplicate target=kept. Both must have equivalent nonempty Git origins. HTTPS and SSH origins for the same repository match. The target keeps its name, root and settings and becomes user-managed. Source todos and notes move to the target root with their IDs, hierarchy, tags and links intact. The source is removed. A merge across roots is refused if another project shares the source root. Checkout paths, Git configuration and historical runs stay unchanged.",
    ),
    (
        "project_open",
        "Open a project in a new tab, and make the tab belong to it. The new tab starts in the root of the project. A project that does not exist needs an absolute root path. Set worktree_dir for future worktrees. Omit it to keep the setting, or give an empty string to clear it. Launch overrides take precedence, then this setting, then .worktrees/. Zellij appends the branch name. Relative paths resolve against the primary checkout, including sibling paths. Absolute paths are also accepted. Shell variables and ~ are not expanded. NUL bytes are rejected. Existing worktrees stay where they are.",
    ),
    (
        "todo_add",
        "Add a todo item that hocus keeps on disk. The text is Markdown. Give a parent ID to put the new item under an item that exists. The tree holds five levels, and a project holds 200 open items and 1000 items in all. An item that you add under a completed item opens that item again.",
    ),
    (
        "todo_list",
        "Show the todo tree of a project, with the ID of each item. The other todo tools need these IDs. Completed items stay hidden unless you set include_done. Outside a project the tool answers with an error: give a project.",
    ),
    (
        "todo_search",
        "Search the text of the todo items, completed items included. The answer gives 50 items at the most. Outside a project the tool answers with an error: give a project, or set all_projects.",
    ),
    (
        "todo_done",
        "Mark a todo item and all the items below it as complete. Set undo to true to open one item again: the items below it stay complete, and the items above it open with it.",
    ),
    (
        "todo_edit",
        "Change the Markdown text of one todo item.",
    ),
    (
        "note_add",
        "Add a note that hocus keeps on disk. A note has a title and a Markdown body. Give tags to group notes, and todo_ids to attach the note to todo items; each of those items must exist. Set global to true for a note that belongs to no project. The title holds 200 characters, the body holds 16 kilobytes, and a note holds 16 tags of 32 characters.",
    ),
    (
        "note_list",
        "Show the notes with their IDs, their tags and their linked todo IDs. The list gives no bodies; use note_get to read one note. Archived notes stay hidden unless you set include_archived. Outside a project the tool answers with an error: give a project, or set global or all_projects.",
    ),
    (
        "note_get",
        "Read one note fully, its body included.",
    ),
    (
        "note_edit",
        "Change the title, the body or the tags of a note. Give at least one of the three. What you do not give stays as it is, but the tags you give replace all the tags of the note.",
    ),
    (
        "note_archive",
        "Hide a note from the lists, but keep it. Set undo to true to show it again.",
    ),
    (
        "note_delete",
        "Delete a note permanently, with its tags and its links. You cannot recover it. Use note_archive to hide a note instead.",
    ),
    (
        "note_link",
        "Attach a note to a todo item. Set unlink to true to remove the attachment.",
    ),
    (
        "note_search",
        "Search the titles and the bodies of the notes, archived notes included. The answer gives 50 notes at the most, each with a part of its body of 160 characters.",
    ),
    (
        "agent_register",
        "Tell hocus that a terminal pane holds a coding agent that hocus did not recognize. Give the kind of the agent in lowercase letters, digits, _ or -, up to 32 characters. After this, agent_list shows the pane.",
    ),
    (
        "agent_list",
        "Show the panes that hold coding agents, with the state of each agent. In a tab that belongs to a project, the tool shows the agents of that project only; give a project to see a different one. The other agent tools need the pane IDs from this list.",
    ),
    (
        "agent_send",
        "Send text to an agent from agent_list, and then the named keys. A prompt usually needs the enter key after the text. The tool refuses a pane that it does not hold as an agent: use agent_register first.",
    ),
    (
        "agent_control",
        "Stop, start again, or close an agent from agent_list. An interrupt sends one Ctrl+c, thus an agent that does not obey Ctrl+c continues. A restart works on an agent that runs as a command, and changes nothing on an agent in a shell pane. A close stops the agent and closes the pane. The tool refuses a pane that it does not hold as an agent.",
    ),
    (
        "agent_runs",
        "Show what the agents in this archive did, the most recent first. Every session sharing it is included, and a session that stopped is included too. Take a run_id from here to agent_run. With this_session, the tool answers with an error until the daemon knows the name of this session.",
    ),
    (
        "agent_run",
        "Read the prompts, the plans, the questions and the results of one agent run. Give a run_id from agent_runs, or the pane_id of an agent that runs now; a call with neither is an error. Without a revision, the tool reads the end of the run.",
    ),
    (
        "agent_history_search",
        "Search what the agents in this archive were asked, and what they answered. Take the run_id and the revision of a hit to agent_run, to read the text around it. With this_session, the tool answers with an error until the daemon knows the name of this session.",
    ),
    (
        "agent_spawn",
        "Start a coding agent in a new pane. Give agent for a known agent, or command for a different program; a call with neither is an error. Give kind to name an agent that hocus does not know: lowercase letters, digits, _ or -, up to 32 characters. Give branch to cut a new branch. The branch gets a git worktree of its own, named after it, in the explicit worktree_dir, then the saved project worktree_dir, then .worktrees/. Relative directories resolve against the primary checkout; absolute and sibling directories are supported, with no shell variable or ~ expansion; with worktree set to false, this checkout moves to the new branch instead. A branch or worktree name already taken is cut afresh as <branch>-2, unless reuse says the existing one is the work you mean. An agent that would start anywhere but a linked worktree - the primary checkout, or a folder that is no repository - is refused unless allow_outside_worktree says so. A cwd outside the project is an error. Give permission_mode as skip for no permission checks, or as one of the modes of the agent itself. To write a zellij TypeScript plugin, use plugin_write instead of an agent.",
    ),
    (
        "slack_status",
        "Report the state of the local Slack archive and of the daemon that fills it. Use this after a Slack read that is empty or that fails. Every Slack tool needs the dobbs program, on your PATH or at dobbs_path in the hocus config block.",
    ),
    (
        "slack_search",
        "Search the local Slack archive. The archive holds what the daemon collected and what slack_backfill fetched, and nothing else. Use slack_read to see a hit with the messages around it.",
    ),
    (
        "slack_read",
        "Read the messages of one Slack conversation from the local archive. Give a channel name, or an ID for a direct message. An empty answer means that the archive has no messages for that window: use slack_backfill, then read again.",
    ),
    (
        "slack_backfill",
        "Fetch the messages of one Slack channel, for a window of dates, into the local archive. Do this before slack_read or slack_search can find them.",
    ),
    (
        "slack_post",
        "Send a message to Slack. The message goes out immediately. Give thread as the timestamp of a message, which slack_read and slack_search both give you, to answer in the thread of that message.",
    ),
    (
        "find_files",
        "Find files in the active project by their name. The tool obeys your gitignore files: a new file that git does not know yet is found, and a file in a folder whose name starts with a dot is not. Give a name fragment, which the tool puts between two stars, or a glob: a pattern that holds a star, a question mark, a bracket, a brace or a slash stays exactly as you wrote it. The answer gives 300 files, and 2000 with max_results.",
    ),
    (
        "search_files",
        "Search the text of the files in the active project with a regular expression. The tool obeys your gitignore files: a new file that git does not know yet is searched, and a file in a folder whose name starts with a dot is not. Each hit gives a line number that you can send to read_file. The answer gives 300 lines, and 2000 with max_results; a longer answer is cut, and says how many lines it holds.",
    ),
    (
        "read_file",
        "Read a range of lines from one file in the active project. The first line is line 1, as in an editor. The path is relative to the project, or an absolute path that another tool gave you; a path that leaves the project is an error, and a path that holds two dots is an error. A start_line after the end of the file answers with no lines.",
    ),
    (
        "git_log",
        "Show the commit history of the active project. Give a path for the history of one file or of one directory. The answer gives 100 commits, and 500 with max_results. To read the next page, give before_commit as the hash of the last commit you saw.",
    ),
    (
        "git_show",
        "Show one commit of the active project. The commit is a hash of 4 to 64 hexadecimal characters, which git_log gives you: HEAD, a branch name and a tag are all errors. Set patch to true to see the changed lines.",
    ),
    (
        "swamp_issue_search",
        "Search swamp-club Lab issues through the Lab API, as the login that swamp auth login saved on this machine, or SWAMP_API_KEY. Without a login only public issues are found. No active project or workflow server is needed. Optional query, type, status, source and author filters narrow the search. All statuses are searched by default. limit defaults to 100 and must be 1 to 100; offset pages through the rest.",
    ),
    (
        "swamp_issue_get",
        "Read a swamp-club Lab issue by its positive integer number, including its body, assignees, URL, comments and related issues. Uses the same login as swamp_issue_search. No active project or workflow server is needed. This tool changes nothing.",
    ),
    (
        "swamp_issue_create",
        "File a bug, feature or security issue on the swamp-club Lab under your swamp login. Secrets, emails, hostnames, IP addresses and home paths are redacted first, the way the swamp CLI redacts them, and the confirmation shows the redacted text that will be posted. Security issues are seen only by you and the admins; the rest are public. It asks first, every time.",
    ),
    (
        "swamp_issue_comment",
        "Comment on a swamp-club Lab issue under your swamp login. The comment is redacted first, like swamp_issue_create, and it asks first, every time.",
    ),
    (
        "github_read",
        "Read from the GitHub API for the active project. The tool sends GET requests only, thus it changes nothing. The endpoint is relative to the repository, because hocus writes repos/<owner>/<repo> in front of it; a full URL is an error. Without an endpoint, the tool reads the repository itself. The tool needs the gh program on your PATH, with a login, and the active project needs a git remote.",
    ),
    (
        "skill_list",
        "Show the skills of the user and of the active project. The list includes the skills that are disabled, the skills that a project copy hides, and the skills that hocus refused.",
    ),
    (
        "skill_read",
        "Read the complete SKILL.md of one skill. Without a scope, the tool reads the copy that is in use. When no enabled skill has that name, the tool answers with an error: give a scope to read a disabled copy.",
    ),
    (
        "skill_write",
        "Write a complete SKILL.md. The source is the whole file, not a patch. Write $ARGUMENTS where the text after the typed command belongs. The scope is user unless you give project, and the project scope needs an active project. Overwrite is necessary to replace a file that exists. The tool refuses a name that a tool or a command already holds, a file above 24 kilobytes, and a file whose frontmatter it cannot read.",
    ),
    (
        "skill_enable",
        "Enable one skill, in the user scope or in the project scope. The scope is necessary, and the project scope needs an active project. A project skill that you enable hides the user skill of the same name. A name with no SKILL.md is an error.",
    ),
    (
        "skill_disable",
        "Disable one skill, but keep its file. The scope is necessary, and the project scope needs an active project. If you disable a project skill, an enabled user skill of the same name becomes visible.",
    ),
    (
        "plugin_docs",
        "Read the SDK declarations and examples for zellij TypeScript plugins. Without a section, the tool lists the sections with their sizes. Give a query, which is a regular expression, to find the section that holds a subject. Then read that section, or a part of it.",
    ),
    (
        "plugin_list",
        "Show the TypeScript and JavaScript plugins of the user, with the length of each one, and which of them have a .bak file from an earlier edit.",
    ),
    (
        "plugin_outline",
        "Show the shape of one TypeScript or JavaScript plugin without its code: every function, every zellij.on handler, every import, every permission request, every data.define and every hocus tool, each with its line number. Use these line numbers to read only the part you need.",
    ),
    (
        "plugin_search",
        "Search the TypeScript and JavaScript plugins of the user with a regular expression. Each hit gives file:LINE:text, and each context line gives file-LINE-text, with two hyphens between the groups. The text after the second separator is the line exactly as it is in the file: copy that text, and not the prefix, into the old_string of plugin_edit. The answer gives 40 hits, and 200 with max_results; context is 0 lines, and 10 at the most. A search of every plugin stops after a number of files, and says how many files it left out.",
    ),
    ("project_config_read", "Read the active project workflow settings and their exact source. Missing configuration has a null source. Invalid configuration returns an error."),
    ("project_config_write", "Validate and write .zellij/swamp-workflows.kdl after confirmation. Supply the complete source and the exact previous source as expected, or null for an absent file. Stale changes are refused. This does not execute CI or grant permissions."),
    (
        "plugin_read",
        "Read one TypeScript or JavaScript plugin of the user. Use explicit .ts filenames for new plugins. Extensionless names select TypeScript. The tool reads 300 lines from start_line. Use plugin_outline or plugin_search first, then read only the range that you must change.",
    ),
    (
        "tool_result_read",
        "Read the original result or arguments of a tool call in this chat. Give tool_call_id and part (result or arguments). Offset counts characters from zero. Each page has at most 4,000 characters and 8 KiB. Use next_offset for the next page. A null next_offset means the end. This tool never runs the original action again.",
    ),
    (
        "plugin_test",
        "Run adjacent TypeScript unit and integration tests by plugin name. Use kind, filter, and timeout_ms to select cases and their deadline. Returns per-case JSON. Test before plugin_reload; use plugin_docs section=sdk_testing to author tests.",
    ),
    (
        "plugin_check",
        "Check a saved file by name, or give source to check an unwritten TypeScript or JavaScript draft. Write nothing and load nothing; JS/TS also checks types and local imports. Use this to try a correction before you write it. Writes and edits format the proposed source and require strict types and lint. A failed check leaves files, backups, and grants unchanged. A source that checks can still fail at load; plugin_reload shows that.",
    ),
    (
        "plugin_write",
        "Write a complete TypeScript plugin with an explicit .ts filename. The source is the whole file, not a patch. Overwrite is necessary to replace a file that exists, and the earlier version stays as <name>.bak. The name can be folder/module, for a file that a plugin imports. Approval of the write also grants the permissions that the source asks for. Hocus formats and strictly checks every write before saving. Create imported dependencies first. Fix diagnostics and check again before reload. Use plugin_reload after this, and plugin_edit to change a plugin that works.",
    ),
    (
        "plugin_format",
        "Format TypeScript or JavaScript with the shared Oxfmt policy. Give source to return formatted draft text without writing. Omit source to format a saved file after confirmation, keeping the previous version as a backup. Only the named file changes.",
    ),
    (
        "plugin_edit",
        "Change a part of a TypeScript or JavaScript plugin. The old_string becomes the new_string, and the old_string must match the file exactly and one time only, unless you set replace_all. Give edits for more than one change under one approval. Hocus formats and strictly checks the result, and writes nothing if any change or check fails. The earlier version stays as <name>.bak, unless the new file is the same as the old one.",
    ),
    (
        "plugin_reload",
        "Load one TypeScript or JavaScript plugin of the user again. A plugin that does not run opens in a pane, and the answer gives the pane ID and the first thing the plugin shows. A plugin in an extension namespace loads without a pane, and the answer says whether it registered its hocus tools. A name with no file is an error. Use plugin_log to see what the plugin wrote.",
    ),
    (
        "plugin_log",
        "Show the last lines that a plugin wrote with zellij.log. The tool gives 50 lines.",
    ),
    (
        "vm_list",
        "Show every Firecracker VM on this machine, with its state, its image, its worktree, its branch, and the disks it holds.",
    ),
    (
        "vm_changes",
        "Count the guest paths that a VM changed after its last sync. The tool writes nothing on either side.",
    ),
    (
        "vm_sync",
        "Merge the work of a running VM into its host worktree. If a file changed on both sides, the host copy stays, and the guest copy goes beside it as <file>.guest.",
    ),
    (
        "vm_close",
        "Release the lease that this session holds on a VM, and ask the matching panes of this session to close. The panes close only when the VM belongs to this session. The last lease starts the merge and the teardown. A VM that is not ready keeps its disk, so that you can recover the work.",
    ),
    (
        "tools_load",
        "Make one family of tools available to the model. Hocus holds most families back, because the model pays for every tool on every request. This changes nothing for you: you can type any tool as a command at any time, loaded or not.",
    ),
];

// ---------------------------------------------------------------------------
// The help request
// ---------------------------------------------------------------------------

/// The name in a bare `/<name> --help`, or `None` for any other line.
///
/// Bare on purpose. `--help` is also an ordinary string a tool can be given —
/// `/search_files query=--help` is a search for the flag, and the tests hold two
/// such lines — so only a line with the flag and nothing else is a request for
/// this. `-h` is accepted for the habit.
pub fn split_help_request(line: &str) -> Option<&str> {
    let line = line.trim().strip_prefix('/')?;
    let mut words = line.split_whitespace();
    let name = words.next()?;
    match words.next() {
        Some("--help" | "-h") => {},
        _ => return None,
    }
    // A third word makes it a call again, whatever the second one was.
    words.next().is_none().then_some(name)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// The line that says how to type the command.
///
/// Required arguments bare and first, optional ones after them in brackets,
/// each with its type — the shape a shell's own usage line has, because that is
/// the shape the reader already knows. Sort each half alphabetically regardless
/// of serde_json's map ordering, which workspace feature unification can change.
///
/// Past `USAGE_OPTIONS_CAP` optional arguments the line says `[options]`
/// instead of naming them. `agent_spawn` has eighteen, and eighteen bracketed
/// pairs wrap into a paragraph of punctuation that answers nothing; the table
/// below names every one of them anyway, with its type.
fn usage_line(name: &str, parameters: &Value) -> String {
    let Some(properties) = parameters["properties"].as_object() else {
        return format!("/{name}");
    };
    let required = required_names(parameters);
    let mut keys: Vec<_> = properties.keys().collect();
    keys.sort_unstable();
    let mut line = format!("/{name}");
    for key in keys
        .iter()
        .copied()
        .filter(|key| required.contains(&key.as_str()))
    {
        line.push_str(&format!(" {key}=<{}>", type_label(&properties[key])));
    }
    let optional: Vec<&String> = keys
        .into_iter()
        .filter(|key| !required.contains(&key.as_str()))
        .collect();
    if optional.len() > USAGE_OPTIONS_CAP {
        line.push_str(" [options]");
        return line;
    }
    for key in optional {
        line.push_str(&format!(" [{key}=<{}>]", type_label(&properties[key])));
    }
    line
}

/// How many optional arguments a usage line will name before it gives up and
/// says `[options]`.
const USAGE_OPTIONS_CAP: usize = 6;

/// The names a schema marks required, in the order it lists them.
fn required_names(parameters: &Value) -> Vec<&str> {
    parameters["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// A property's type as the table names it. `array` carries the type of its
/// items, because `keys=<array>` alone does not say what to put in it.
fn type_label(property: &Value) -> String {
    match property["type"].as_str() {
        Some("array") => match property["items"]["type"].as_str() {
            Some(item) => format!("array of {item}"),
            None => "array".to_owned(),
        },
        Some(name) => name.to_owned(),
        // A property built from a `$ref` or from nothing at all. Rare, and a
        // blank column is better than a lie about the type.
        None => "value".to_owned(),
    }
}

/// The closed set a property accepts, from the property itself or from the
/// items of an array. Empty for a property that accepts any value of its type.
fn enum_values(property: &Value) -> Vec<String> {
    let values = match property["enum"].as_array() {
        Some(values) => values,
        None => match property["items"]["enum"].as_array() {
            Some(values) => values,
            None => return Vec::new(),
        },
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A schema `description` as this file will print it.
///
/// The strings in `specs()` are written for the model and are allowed the
/// backtick a model reads as a quote. A notice is rendered through `wrap` and
/// not through the markdown pass, so a backtick would be shown literally, and
/// `help_is_plain_text` refuses one. Stripped here rather than rewritten there:
/// those strings go out on every request and are held near a byte floor, and a
/// second copy of each of them for the reader is the tax this file exists to
/// avoid paying.
fn plain(text: &str) -> String {
    text.chars().filter(|ch| !"`#*".contains(*ch)).collect()
}

/// The argument table: one row per property, then that property's own
/// description and the closed set it accepts, indented under its own row.
///
/// Both go on their own lines rather than into the row, because a row is
/// already as wide as the panel and `wrap` does not preserve the indent of a
/// line it breaks — a 26-value list in the last column would come back as
/// ragged text under the first.
///
/// The description comes from the schema, so a default recorded there ("Defaults
/// to new_tab") reaches the reader without a paragraph repeating it, and cannot
/// drift from the value the tool actually uses. Meaning first, then the closed
/// set: a list of words answers nothing until you know what the word is for.
fn argument_table(parameters: &Value) -> Option<String> {
    let properties = parameters["properties"].as_object()?;
    if properties.is_empty() {
        return None;
    }
    let required = required_names(parameters);
    let width = properties
        .keys()
        .map(|key| key.chars().count())
        .max()
        .unwrap_or_default();
    let mut rows = vec!["arguments".to_owned()];
    let mut properties: Vec<_> = properties.iter().collect();
    properties.sort_unstable_by_key(|(key, _)| *key);
    for (key, property) in properties {
        let mark = match required.contains(&key.as_str()) {
            true => "  required",
            false => "",
        };
        rows.push(format!(
            "  {key:<width$}  {}{mark}",
            type_label(property),
            width = width
        ));
        if let Some(description) = property["description"].as_str() {
            rows.push(format!("      {}", plain(description)));
        }
        let values = enum_values(property);
        if !values.is_empty() {
            rows.push(format!("      one of: {}", values.join(", ")));
        }
    }
    Some(rows.join("\n"))
}

/// The sentence that says a tool stops for a y/n, and that typing it does not.
///
/// Worth its own line for every tool that has it: the manual path skips the
/// confirmation on purpose (`run_manual_call` goes straight to `run_head`), and
/// a person who has only ever seen the prompt would not guess that.
const CONFIRMS: &str =
    "Hocus checks approval before the model can do this. Your typed command is the \
                        approval, thus the tool starts immediately.";

/// The confirmation sentence for one tool, or `None` for a tool that never
/// stops.
///
/// Four tools stop only in one shape, and the shape is in the arguments
/// (`tools::call_needs_confirmation`). Each gets its own sentence, because the
/// generic one would say a bare `/new_pane` stops, and it does not. The arms
/// here and there have to agree; a test holds them to it.
fn confirms_line(name: &str) -> Option<&'static str> {
    let tailored = match name {
        "new_pane" | "new_remote_pane" => {
            "A pane with no command opens a shell and does not stop. A pane with a command runs \
             what the model chose, thus hocus checks approval first. Your typed command is the approval."
        },
        "schedule" => {
            "A job with auto_confirm does not stop again for each tool it uses, thus hocus checks \
             approval once, when you arm it. Your typed command is the approval."
        },
        "agent_spawn" => {
            "An agent with no prompt waits for a person to type, and does not stop. An agent \
             with a prompt starts work immediately, thus hocus checks approval first. Your typed \
             command is the approval."
        },
        name => return tools::needs_confirmation(name).then_some(CONFIRMS),
    };
    Some(tailored)
}

/// One tool's help, from its spec plus whatever prose there is for it.
///
/// `prose` is `None` for an extension tool, which has no entry in `TOOL_HELP`:
/// its registration carries a description written by whoever wrote the
/// extension, and that is used in place of the paragraph.
fn render(
    name: &str,
    parameters: &Value,
    prose: &str,
    confirms: Option<&str>,
    footer: Option<String>,
) -> String {
    let mut blocks = vec![usage_line(name, parameters), prose.to_owned()];
    if let Some(confirms) = confirms {
        blocks.push(confirms.to_owned());
        blocks.push("With Decisions configured, Jev checks the call against your own words and approves it when it is clearly what you asked for and scores low on destruction, secret exposure, and bypassed controls. Otherwise Hocus asks you, and says what Jev answered. Set decisions_auto_approve false to require manual confirmation; decisions_min_requested and decisions_max_hazard can raise the bar above the defaults of 0.75 and 0.25, never lower it.".to_owned());
    }
    if let Some(table) = argument_table(parameters) {
        blocks.push(table);
        blocks.push(
            "Give the arguments as key=value pairs, or as one JSON object. A value with a space \
             in it needs the JSON form."
                .to_owned(),
        );
    }
    if let Some(footer) = footer {
        blocks.push(footer);
    }
    blocks.join("\n\n")
}

/// The help for `name`, or `None` for a name nothing answers to.
///
/// The order is the order `run_manual` dispatches in: the plugin's own commands,
/// then the builtin tools, then the extension tools. Skills are absent by
/// design — `--help` after a skill is arguments for the model, and `run_manual`
/// never asks this about one.
pub fn lookup(name: &str, extensions: &extensions::Registry) -> Option<String> {
    if let Some((_, text)) = COMMAND_HELP.iter().find(|(command, _)| *command == name) {
        return Some((*text).to_owned());
    }
    if let Some(spec) = tools::specs().as_array().and_then(|specs| {
        specs
            .iter()
            .find(|spec| spec["function"]["name"].as_str() == Some(name))
            .cloned()
    }) {
        let prose = TOOL_HELP
            .iter()
            .find(|(tool, _)| *tool == name)
            .map(|(_, text)| *text)
            // Unreachable while `every_tool_has_help` passes; the description is
            // a poor paragraph but a better answer than nothing.
            .unwrap_or_else(|| spec["function"]["description"].as_str().unwrap_or_default());
        let footer = tools::family_of(name).map(|family| match family.deferred {
            true => format!(
                "Family: {} — the model asks for it with tools_load. You can type it now either \
                 way.",
                family.name
            ),
            false => format!("Family: {}.", family.name),
        });
        return Some(render(
            name,
            &spec["function"]["parameters"],
            prose,
            confirms_line(name),
            footer,
        ));
    }
    let tool = extensions.tool(name)?;
    Some(render(
        name,
        &tool.parameters,
        &tool.description,
        (tool.safety == Safety::Confirm).then_some(CONFIRMS),
        Some(format!(
            "From the {} extension, which is a plugin and not a part of hocus.",
            tool.namespace
        )),
    ))
}

#[cfg(test)]
#[path = "unit/help_tests/mod.rs"]
mod help_tests;
