use crate::message::{Job, Message, Status};
use crate::worker::read_pane_text;
use crate::*;
use std::collections::BTreeMap;
use std::path::Path;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// The tool loop
// ---------------------------------------------------------------------------

impl State {
    /// Work through `pending` until something has to wait — a confirmation, a
    /// command result — or the queue empties and the results go back to the
    /// model.
    pub(crate) fn pump_tools(&mut self) {
        while let Some(call) = self.pending.front().cloned() {
            let grants_job_authority = call.name == "schedule"
                && tools::Args::parse(&call.arguments).is_ok_and(|args| args.auto_confirm);
            if (grants_job_authority || tools::needs_confirmation(&call.name))
                && !self.job_auto_confirm
            {
                self.status = Status::ConfirmingTool;
                self.pin_to_bottom();
                return;
            }
            if tools::needs_confirmation(&call.name) {
                self.log(
                    "tool_confirm",
                    serde_json::json!({
                        "tool_call_id": call.id,
                        "tool_name": call.name,
                        "decision": "automatic",
                    }),
                );
            }
            if !self.run_head(&call) {
                return;
            }
        }

        // Only the user's own calls were in the queue: nothing to send.
        self.status = Status::Idle;
        if !std::mem::take(&mut self.owes_reply) {
            return;
        }

        // Every call answered: hand the results back so the model can use them.
        // The request at exactly MAX_TOOL_ROUNDS goes out without tools (see
        // `start_request`), so getting past it means the model called a tool it
        // was never offered. Nothing left to try.
        self.tool_rounds += 1;
        if self.tool_rounds > MAX_TOOL_ROUNDS {
            self.job_auto_confirm = false;
            let err =
                format!("stopped after {MAX_TOOL_ROUNDS} rounds of tool calls without a reply");
            self.messages.push(Message::error(err.clone()));
            self.log("error", serde_json::json!({"error": log::truncate(&err)}));
            return;
        }
        let Ok(cfg) = self.cfg.clone() else {
            self.job_auto_confirm = false;
            return;
        };
        self.start_request(&cfg);
    }

    /// Dispatch the head. Returns whether the queue advanced — `false` means the
    /// result is deferred and `pump_tools` must stop and wait.
    pub(crate) fn run_head(&mut self, call: &ToolCall) -> bool {
        // Needs the resolved config and defers on a web request, neither of
        // which tools::dispatch can reach.
        if call.name == "web_search" {
            return self.start_search(call);
        }
        // Same reason: the dobbs binary comes from the config.
        if tools::SLACK_TOOLS.contains(&call.name.as_str()) {
            return self.start_slack(call);
        }
        // These need the active project, which dispatch cannot reach, and read
        // files no wasm plugin can open — so they defer on the host as well.
        if tools::FILE_TOOLS.contains(&call.name.as_str()) {
            return self.start_file_tool(call);
        }
        // The todo store lives in another plugin now, so these defer on its
        // reply the way a command defers on the host's.
        if matches!(call.name.as_str(), "todo_add" | "todo_done") {
            return self.start_todo_write(call);
        }
        // The scheduling tools live here rather than in tools::dispatch, which
        // is a free function with no access to the state they mutate.
        if let Some(result) = self.run_local(call) {
            self.finish_head(result);
            return true;
        }
        self.finish_head(tools::dispatch(call));
        true
    }

    /// Run a tool the user typed as `/name key=value …`.
    ///
    /// Recorded as a real call-and-result pair rather than a notice, so the model
    /// sees what was done on the next message — and so the transcript stays valid
    /// for `api::request_body`, which sends every tool result and would otherwise
    /// be left with an orphan.
    ///
    /// No confirmation prompt: typing the command *is* the confirmation, which is
    /// why this calls `run_head` rather than going through `pump_tools`.
    pub(crate) fn run_manual(&mut self, line: &str) -> bool {
        self.pin_to_bottom();
        let line = line.trim();
        if line == "/" || line == "/help" {
            let help = format!(
                "/{}  /{}",
                LOCAL_COMMANDS.join("  /"),
                tools::names().join("  /")
            );
            self.messages.push(Message::notice(help.clone()));
            self.log(
                "notice",
                serde_json::json!({"content": log::truncate(&help)}),
            );
            return true;
        }
        // The plugin's own lists, not tools: the model is never offered these.
        if line == "/history" {
            self.show_chats();
            return true;
        }
        if line == "/models" {
            self.show_models();
            return true;
        }
        if line == "/stop_jobs" {
            return self.stop_jobs();
        }
        if line == "/keys" {
            self.messages.push(Message::notice(KEYS_HELP));
            self.log(
                "notice",
                serde_json::json!({"content": log::truncate(KEYS_HELP)}),
            );
            return true;
        }
        // A reply arriving is no reason to wait, but another *tool* is: the
        // queue runs one call at a time, and its head owns the deferred result
        // the host is about to deliver.
        if self.tools_busy() {
            let notice = "a tool is already running — <Esc> aborts it";
            self.messages.push(Message::notice(notice));
            self.log("notice", serde_json::json!({"content": notice}));
            return true;
        }
        let (name, arguments) = match tools::parse_command(line) {
            Ok(parsed) => parsed,
            Err(error) => {
                let e = error.clone();
                self.messages.push(Message::error(error));
                self.log("error", serde_json::json!({"error": log::truncate(&e)}));
                return true;
            },
        };

        let call = ToolCall {
            // Unique within the transcript: every manual call appends at least
            // the summary message below before the next one can be typed.
            id: format!("manual-{}", self.messages.len()),
            name: name.clone(),
            arguments: arguments.clone(),
        };
        self.messages.push(Message::tool_calls(vec![call.clone()]));
        self.log(
            "tool_call",
            serde_json::json!({
                "tool_call_id": call.id,
                "tool_name": call.name,
                "arguments": log::truncate(&call.arguments),
                "manual": true,
            }),
        );
        // The result is for the person who typed the command, so it is rendered
        // whole; `finish_head` takes this back when the head is answered.
        self.manual_head = true;
        self.pending.push_back(call.clone());
        // Straight to `run_head` rather than through `pump_tools`: typing the
        // command is itself the confirmation, so a destructive one runs without
        // the y/n prompt the model's calls get.
        self.run_head(&call);
        true
    }

    /// Record a result for the head call and drop it from the queue.
    pub(crate) fn finish_head<S: Into<String>>(&mut self, result: S) {
        if let Some(call) = self.pending.pop_front() {
            let result_str = result.into();
            // Cleared with the head it belongs to: whatever is behind it in the
            // queue came from the model and is elided as usual.
            let message = if std::mem::take(&mut self.manual_head) {
                Message::tool_result_full(&call.id, result_str.clone())
            } else {
                Message::tool_result(&call.id, result_str.clone())
            };
            self.messages.push(message);
            self.log(
                "tool_result",
                serde_json::json!({
                    "tool_call_id": call.id,
                    "tool_name": call.name,
                    "result": log::truncate(&result_str),
                }),
            );
            self.pin_to_bottom();
        }
    }

    /// Ask the configured SearxNG. Deferred like the Slack tools: the answer
    /// arrives as the `WebRequestResult` carrying this call's id.
    ///
    /// ponytail: a plain `web_request` has no stream id, so there is nothing to
    /// cancel — `Esc` stops waiting and the late reply is dropped because the
    /// head no longer matches, exactly as for an aborted Slack call. Move to
    /// `web_request_stream` + `cancel_web_request` if hung searches turn out to
    /// matter.
    pub(crate) fn start_search(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let Some(query) = args.query.filter(|query| !query.trim().is_empty()) else {
            self.finish_head("query is required and must not be empty");
            return true;
        };
        // Cloned so the request can be issued without holding a borrow of self.
        let Ok(search_url) = self.cfg.as_ref().map(|cfg| cfg.search_url.clone()) else {
            self.finish_head("hocus is not configured");
            return true;
        };

        web_request(
            api::search_query_url(&search_url, &query),
            HttpVerb::Get,
            BTreeMap::from([("Accept".to_owned(), "application/json".to_owned())]),
            Vec::new(),
            // Round-trips through the host, which is how the result is matched
            // back to the call that made it.
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
        );
        self.status = Status::AwaitingSearch;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// Run one of the Slack tools by shelling out to dobbs. The result comes back
    /// as `RunCommandResult` carrying this call's id, which is the only deferred
    /// path left through `on_command_result`.
    ///
    /// dobbs owns the Slack credentials and reaches its daemon over TCP, which a
    /// wasm plugin cannot do; the CLI is the whole integration.
    ///
    /// ponytail: no timeout — a dobbs call that never exits waits forever and
    /// only Esc recovers. Add set_timeout + Event::Timer here if hanging calls
    /// turn out to be common in practice.
    pub(crate) fn start_slack(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let argv = match tools::slack_argv(&call.name, &args) {
            Ok(argv) => argv,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        // Cloned so the command can be run without holding a borrow of self.
        let Ok((dobbs, env)) = self
            .cfg
            .as_ref()
            .map(|cfg| (cfg.dobbs.clone(), cfg.dobbs_env()))
        else {
            self.finish_head("hocus is not configured");
            return true;
        };

        let mut command = vec![dobbs];
        command.extend(argv);
        let borrowed: Vec<&str> = command.iter().map(String::as_str).collect();
        run_command_with_env_variables_and_cwd(
            &borrowed,
            env,
            // dobbs is cwd-independent: it finds its archive and daemon under
            // ~/.dobbs, so there is nothing to inherit from the focused pane.
            std::path::PathBuf::from("."),
            // Round-trips through the host, which is how the result is matched
            // back to the call that made it.
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
        );
        self.status = Status::AwaitingCommand;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// The project a file tool reads from.
    ///
    /// Unlike the todo tools this takes no `project` argument: the file tools
    /// read whatever the user is looking at, and the active project follows the
    /// active tab. A pane sitting outside any checkout has none, and that is an
    /// answer rather than a guess — reading a project the user is not in is
    /// exactly what the root check exists to prevent.
    pub(crate) fn project_target(&self) -> Result<&projects::Project, String> {
        let Some(name) = self.active_project.as_deref() else {
            return Err(
                "no active project; open a tab in a checkout, or call project_list".to_owned(),
            );
        };
        self.projects
            .get(name)
            .ok_or_else(|| format!("unknown project {name}; call project_list"))
    }

    /// Run one of the file tools by shelling out to `rg` or `sed`, rooted at the
    /// active project. The result comes back as `RunCommandResult` carrying this
    /// call's id, the same way the Slack tools answer.
    ///
    /// A wasm plugin's WASI preopens are `/host`, `/data`, `/cache` and `/tmp`,
    /// and a project root is none of them, so there is no reading these files
    /// from in here — the host is the only way to reach them.
    ///
    /// Everything that can be refused without running anything is refused here:
    /// no project, a bad argument, or a path outside the root never reaches a
    /// command.
    pub(crate) fn start_file_tool(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let argv = match self.project_target().and_then(|project| {
            // `path` is the file for `read_file` and the folder to search under
            // for the other two; either way it is checked against the root.
            let target = tools::resolve_in_root(project, args.path.as_deref())?;
            tools::file_argv(&call.name, &args, &target)
        }) {
            Ok(argv) => argv,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };

        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_command_with_env_variables_and_cwd(
            &borrowed,
            BTreeMap::new(),
            // Every path in the argv is absolute, so the command has nothing to
            // resolve and the cwd decides nothing.
            //
            // It cannot be the project root, tempting as that reads. The host
            // rewrites a cwd under `/tmp`, `/host`, `/data` or `/cache` into the
            // plugin's own folder of that name — those are a plugin's WASI
            // preopens — so a project checked out under any of them would run in
            // a folder that does not exist, and the tool would report `rg` as
            // missing. Arguments are passed through untouched; only the cwd is
            // translated. A relative one lands on the plugin's own directory,
            // which always exists, so `.` is the one that is never rewritten
            // into nonsense. The Slack tools pass `.` for the same reason.
            std::path::PathBuf::from("."),
            // Round-trips through the host, which is how the result is matched
            // back to the call that made it.
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
        );
        self.status = Status::AwaitingCommand;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// The deferred half of `web_search`. Returns whether this response was one:
    /// a `WebRequestResult` with no `tool_call_id` is the head of an API stream.
    pub(crate) fn finish_search(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !matches!(self.status, Status::AwaitingSearch) {
            return false;
        }
        // A search we already aborted, or someone else's request.
        let Some(head) = self.pending.front() else {
            return false;
        };
        if context.get("tool_call_id") != Some(&head.id) {
            return false;
        }

        let result = if (200..300).contains(&status) {
            api::search_results(body, api::SEARCH_RESULTS)
        } else {
            api::api_error_message(status, body)
        };

        self.status = Status::Idle;
        self.finish_head(result);
        self.pump_tools();
        true
    }

    /// The tools that mutate our own state rather than the session. `None`
    /// means the call belongs to `tools::dispatch`.
    pub(crate) fn run_local(&mut self, call: &ToolCall) -> Option<String> {
        if !matches!(
            call.name.as_str(),
            "schedule"
                | "unschedule"
                | "project_list"
                | "project_assign"
                | "project_open"
                | "project_forget"
                | "todo_list"
                | "todo_search"
                | "agent_register"
                | "agent_list"
                | "agent_send"
                | "agent_control"
                | "agent_spawn"
        ) {
            return None;
        }
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => return Some(error),
        };
        Some(match call.name.as_str() {
            "schedule" => self.schedule(&args),
            "unschedule" => self.unschedule(args.job_id),
            "project_list" => self.project_list(),
            "project_assign" => self.project_assign(&args),
            "project_open" => self.project_open(&args),
            "project_forget" => self.project_forget(&args),
            "todo_list" => self.todo_list(&args),
            "todo_search" => self.todo_search(&args),
            "agent_register" => self.agent_register(&args),
            "agent_list" => self.agent_list(&args),
            "agent_spawn" => self.agent_spawn(&args),
            _ => self.agent_act(call, &args),
        })
    }

    pub(crate) fn remember_project(
        &mut self,
        args: &tools::Args,
    ) -> Result<projects::Project, String> {
        let name = args
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "name is required".to_owned())?;
        if let Some(root) = args.root.as_deref() {
            let project = projects::validate(name, root)?;
            let before = self.projects.clone();
            // The user just gave this folder a better name than a poll could.
            if let Some(gone) = projects::adopt(&mut self.projects, project.clone()) {
                self.rename_project(&gone, &project.name);
            }
            // Applied to our copy first and written after: `project_open` needs
            // the project in hand to open its tab, and the watch notification
            // confirming the same change is queued through the plugin thread
            // rather than delivered inline, so it arrives later.
            let mutations = projects::diff_mutations(&before, &self.projects);
            let _ = self.write_store(mutations);
            Ok(project)
        } else {
            self.projects.get(name).cloned().ok_or_else(|| {
                format!("unknown project {name}; root is required for a new project")
            })
        }
    }

    pub(crate) fn project_list(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .projects
            .values()
            .map(|project| {
                let ids = |assigned: &BTreeMap<usize, String>| {
                    let mut ids: Vec<usize> = assigned
                        .iter()
                        .filter_map(|(id, name)| (name == &project.name).then_some(*id))
                        .collect();
                    ids.sort_unstable();
                    ids
                };
                let auto_tab_ids = ids(&self.auto_tab_projects);
                let mut tab_ids = ids(&self.tab_projects);
                tab_ids.extend(auto_tab_ids.iter().copied());
                tab_ids.sort_unstable();
                tab_ids.dedup();
                serde_json::json!({
                    "name": project.name,
                    "root": project.root,
                    "remote": project.remote,
                    "worktree_status": project.worktree_status,
                    "discovered": project.discovered,
                    "tab_ids": tab_ids,
                    "auto_tab_ids": auto_tab_ids,
                })
            })
            .collect();
        serde_json::json!({ "active": self.active_project, "projects": items }).to_string()
    }

    pub(crate) fn project_assign(&mut self, args: &tools::Args) -> String {
        let tab_id = match args.tab_id.or_else(|| {
            self.tabs
                .iter()
                .find(|tab| tab.active)
                .map(|tab| tab.tab_id)
        }) {
            Some(id) if self.tabs.iter().any(|tab| tab.tab_id == id) => id,
            Some(id) => return format!("tab {id} is not available"),
            None => return "no active tab is available".to_owned(),
        };
        let project = match self.remember_project(args) {
            Ok(project) => project,
            Err(error) => return error,
        };
        self.tab_projects.insert(tab_id, project.name.clone());
        // The two maps stay disjoint, so `project_list` cannot report a tab as
        // both assigned and found.
        self.auto_tab_projects.remove(&tab_id);
        if self
            .tabs
            .iter()
            .any(|tab| tab.tab_id == tab_id && tab.active)
        {
            self.active_project = Some(project.name.clone());
        }
        format!("assigned tab {tab_id} to project {}", project.name)
    }

    pub(crate) fn project_open(&mut self, args: &tools::Args) -> String {
        let project = match self.remember_project(args) {
            Ok(project) => project,
            Err(error) => return error,
        };
        let call = ToolCall {
            id: String::new(),
            name: "new_tab".to_owned(),
            arguments: serde_json::json!({
                "name": args.tab_name.as_deref().unwrap_or(&project.name),
                "cwd": project.root,
            })
            .to_string(),
        };
        let result = tools::dispatch(&call);
        self.register_opened_project(&project.name, &result)
    }

    /// Drop a project from the registry, here and in the store.
    ///
    /// Applied to our copy first for the same reason `remember_project` is: the
    /// answer is known without asking, and everything that named the project
    /// has to stop naming it now rather than a round trip later, or `agent_list`
    /// answers "unknown project" for a tab in between.
    pub(crate) fn project_forget(&mut self, args: &tools::Args) -> String {
        let Some(name) = args
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            return "name is required".to_owned();
        };
        let Some(project) = self.projects.remove(name) else {
            return format!("unknown project {name}; call project_list");
        };
        self.tab_projects.retain(|_, assigned| assigned != name);
        // Rebuilds the tabs the poll worked out and the active project with
        // them, which is what the watch notification would do anyway.
        self.refresh_tab_projects();
        let _ = self.write_store(vec![projects::delete_mutation(name)]);
        projects::forgotten(&project)
    }

    pub(crate) fn register_opened_project(&mut self, name: &str, result: &str) -> String {
        let tab_id = serde_json::from_str::<serde_json::Value>(&result)
            .ok()
            .and_then(|value| value["tab_id"].as_u64());
        let Some(tab_id) = tab_id else {
            return result.to_owned();
        };
        self.tab_projects.insert(tab_id as usize, name.to_owned());
        self.auto_tab_projects.remove(&(tab_id as usize));
        self.active_project = Some(name.to_owned());
        result.to_owned()
    }

    /// The project a todo tool acts on, as `(root, name)`.
    ///
    /// A list is keyed on the root folder, so a rename of the project keeps the
    /// list. `project` takes an exact name, or an absolute root for a list whose
    /// project is gone. The name comes back only to put in the answer.
    pub(crate) fn todo_target(
        &self,
        args: &tools::Args,
    ) -> Result<(String, Option<String>), String> {
        let selected = args.project.as_deref().or(self.active_project.as_deref());
        let Some(selected) = selected else {
            return Err("no active project; give project, or call project_list".to_owned());
        };
        todos::resolve(&self.projects, selected)
    }

    /// Change a todo list, and answer with what changed.
    ///
    /// The project is resolved here rather than anywhere else because "which
    /// project" is a question about *this* pane's active tab, which only hocus
    /// knows.
    ///
    /// The whole thing is synchronous now. It used to be piped to hocus-store
    /// and the tool call parked waiting on the reply, with a five-second timeout
    /// in case no store was running to answer. An apply is a host call that has
    /// either happened or failed by the time it returns, so the queue never
    /// parks and there is no timeout to get wrong.
    pub(crate) fn todo_write(&mut self, call: &ToolCall) -> Result<String, String> {
        let args = tools::Args::parse(&call.arguments)?;
        let (root, name) = self.todo_target(&args)?;
        let backend = self.store();
        let list = todos::fetch_list(&backend, &root)?;

        match call.name.as_str() {
            "todo_add" => {
                let text = args.text.as_deref().ok_or("text is required")?;
                let mutations = todos::plan_add(&list, &root, text, args.parent_id)?;
                let report = backend.apply(mutations)?;
                Ok(serde_json::json!({
                    "project": name,
                    // The first put of the batch is the new item; anything after
                    // it reopens an ancestor and inserts nothing.
                    "id": report.ids.first().copied().unwrap_or_default(),
                    "text": text,
                })
                .to_string())
            },
            _ => {
                let id = args.id.ok_or("id is required")?;
                let done = !args.undo;
                let (mutations, change) = todos::plan_set_done(&list, &root, id, done)?;
                backend.apply(mutations)?;
                Ok(serde_json::json!({
                    "project": name,
                    "id": change.id,
                    "text": change.text,
                    "done": change.done,
                    "cascaded": change.cascaded,
                    "reopened": change.reopened,
                })
                .to_string())
            },
        }
    }

    /// Run a todo change and finish the call with its answer.
    ///
    /// Always returns `true`: the queue always advances, because nothing waits.
    pub(crate) fn start_todo_write(&mut self, call: &ToolCall) -> bool {
        let answer = match self.todo_write(call) {
            Ok(answer) => answer,
            Err(error) => error,
        };
        self.finish_head(answer);
        true
    }

    /// One project's list, fetched when asked for.
    ///
    /// hocus used to hold every list of every project so this could answer out
    /// of memory. It is a tool call, which is already behind a model round trip,
    /// so a fetch here costs nothing anyone can perceive — and it drops the
    /// largest thing hocus was keeping a copy of.
    pub(crate) fn todo_list(&self, args: &tools::Args) -> String {
        let (root, name) = match self.todo_target(args) {
            Ok(target) => target,
            Err(error) => return error,
        };
        let list = match todos::fetch_list(&self.store(), &root) {
            Ok(list) => list,
            Err(error) => return error,
        };
        let mut one = todos::Store::new();
        one.insert(root.clone(), list);
        let mut value = todos::list_json(&one, &root, args.include_done);
        value["project"] = serde_json::json!(name);
        value.to_string()
    }

    /// The name of the project that owns a folder, for a search result.
    pub(crate) fn project_named(&self, root: &str) -> Option<String> {
        self.projects
            .values()
            .find(|project| project.root == root)
            .map(|project| project.name.clone())
    }

    pub(crate) fn todo_search(&self, args: &tools::Args) -> String {
        let Some(query) = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
        else {
            return "query is required".to_owned();
        };
        // Every list is searched by folder, so a list whose project went away is
        // still found. The rows are the lasting record; the registry is worked
        // out again on every poll.
        let (scope_root, scope) = if args.all_projects {
            (None, "all".to_owned())
        } else {
            match self.todo_target(args) {
                Ok((root, name)) => (Some(root.clone()), name.unwrap_or(root)),
                Err(error) => return error,
            }
        };
        let found = match todos::search(&self.store(), scope_root.as_deref(), query) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let items: Vec<serde_json::Value> = found
            .iter()
            .map(|found| {
                serde_json::json!({
                    "id": found.id,
                    "text": found.text,
                    "done": found.done,
                    "project": self.project_named(&found.root),
                    "root": found.root,
                    "trail": found.trail,
                })
            })
            .collect();
        serde_json::json!({
            "query": query,
            "scope": scope,
            "matches": items,
            "truncated": found.len() >= todos::MAX_MATCHES,
        })
        .to_string()
    }

    /// The project of a tab: what the user assigned, then what the poll found.
    pub(crate) fn tab_project(&self, tab_id: usize) -> Option<&str> {
        self.tab_projects
            .get(&tab_id)
            .or_else(|| self.auto_tab_projects.get(&tab_id))
            .map(String::as_str)
    }

    /// Pane id to project name, for each agent whose folder a project owns.
    pub(crate) fn pane_projects(&self) -> BTreeMap<u32, String> {
        self.panes
            .iter()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| {
                let owner = self.project_for_cwd(pane.cwd.as_deref()?)?;
                Some((pane.pane_id, owner.name.clone()))
            })
            .collect()
    }

    /// Pane id to branch name, for each agent sitting in a checkout.
    ///
    /// Deliberately not gated on the folder belonging to a project the way
    /// `pane_projects` is: a checkout nobody named is still on a branch, and an
    /// agent working in one is exactly what the picker has to tell apart.
    pub(crate) fn pane_branches(&self) -> BTreeMap<u32, String> {
        self.panes
            .iter()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| {
                let repository = self.repository_for_cwd(pane.cwd.as_deref()?)?;
                Some((pane.pane_id, repository.branch.clone()?))
            })
            .collect()
    }

    /// What Git said about an agent's folder, or about the nearest folder above
    /// it that we have an answer for.
    ///
    /// The daemon probes each folder an agent is in, so an agent that `cd`s into
    /// a subdirectory of its checkout has no answer of its own until a fresh
    /// probe lands. It is in the same checkout it was in a moment ago, and on
    /// screen the wait reads as the agent losing its project and getting it back.
    /// The exact answer replaces this stand-in as soon as it arrives.
    fn repository_for_cwd(&self, cwd: &str) -> Option<&projects::Repository> {
        if let Some(repository) = self.repositories.get(cwd) {
            return Some(repository);
        }
        self.repositories
            .iter()
            // Component-wise, the same rule `projects::contains` puts on a
            // project root, so `/work/hocus-old` is not below `/work/hocus`.
            .filter(|(probed, _)| Path::new(cwd).starts_with(probed))
            .max_by_key(|(probed, _)| Path::new(probed).components().count())
            .map(|(_, repository)| repository)
    }

    /// The project that owns an agent's folder, when Git claims that folder.
    ///
    /// A folder outside a repository belongs to no project even when a project
    /// root contains it: an agent that is not in a checkout is not working on a
    /// project. The daemon fills `repositories` from its Git probe, so an agent
    /// has no project for the moment between its pane appearing and that probe
    /// landing.
    ///
    /// Git identity is asked first and the folder second. What a project *is* is
    /// a repository; containment is a proxy for that, and it misfires for a
    /// checkout that happens to sit under an unrelated project's root. The
    /// containment answer is still worth having second — `same_repository` only
    /// matches when both sides carry Git metadata, so a project the user named
    /// on a plain folder is found by nothing else, and two projects sharing a
    /// remote fall through to the folder rather than to nothing at all.
    fn project_for_cwd(&self, cwd: &str) -> Option<&projects::Project> {
        let repository = self
            .repository_for_cwd(cwd)
            .filter(|repository| repository.is_checkout())?;
        projects::for_repository(&self.projects, repository)
            .or_else(|| projects::for_cwd(&self.projects, cwd))
    }

    /// Point everything that names a project that went away at the project that
    /// took its folders. Without this, the active project can name nothing and
    /// `agent_list` answers "unknown project".
    pub(crate) fn rename_project(&mut self, gone: &str, kept: &str) {
        for assigned in self
            .tab_projects
            .values_mut()
            .chain(self.auto_tab_projects.values_mut())
            .chain(self.active_project.iter_mut())
        {
            if assigned == gone {
                kept.clone_into(assigned);
            }
        }
    }

    /// Work out the project of each tab again, and of the active tab with it.
    pub(crate) fn refresh_tab_projects(&mut self) {
        let owners = self.pane_projects();
        self.auto_tab_projects = agents::tab_projects(&self.panes, &self.manual_kinds, &owners);
        // A tab the user assigned is left alone, so the two maps stay disjoint.
        self.auto_tab_projects
            .retain(|tab_id, _| !self.tab_projects.contains_key(tab_id));
        self.active_project = self
            .tabs
            .iter()
            .find(|tab| tab.active)
            .and_then(|tab| self.tab_project(tab.tab_id))
            .map(str::to_owned);
    }

    /// The snapshot every agent tool works from. Split out so the host calls in
    /// Host-side pane reads are the only part that cannot run under `make test`.
    pub(crate) fn set_panes(&mut self, panes: Vec<agents::AgentPane>) -> bool {
        self.panes = panes;
        let before = self.manual_kinds.len();
        self.pending_agent_panes
            .retain(|id| !self.panes.iter().any(|pane| pane.pane_id == *id));
        // A pane that closed is a registration that no longer names anything. A
        // newly spawned pane is the exception until its first snapshot arrives.
        self.manual_kinds.retain(|id, _| {
            self.pending_agent_panes.contains(id)
                || self.panes.iter().any(|pane| pane.pane_id == *id)
        });
        self.activity
            .retain(|id, _| self.panes.iter().any(|pane| pane.pane_id == *id));
        self.manual_kinds.len() != before
    }

    /// Name a pane as an agent by hand, for what detection cannot know.
    pub(crate) fn agent_register(&mut self, args: &tools::Args) -> String {
        let Some(pane_id) = args.pane_id else {
            return "pane_id is required".to_owned();
        };
        let Some(kind) = args.kind.as_deref() else {
            return "kind is required".to_owned();
        };
        if !agents::valid_kind(kind) {
            return "kind must be 1 to 32 characters of a-z, 0-9, _ or -".to_owned();
        }
        // `panes` holds terminal panes only, so this one check also answers a plugin
        // pane and a pane id the model invented.
        if !self.panes.iter().any(|pane| pane.pane_id == pane_id) {
            return format!("pane {pane_id} is not available");
        }
        self.manual_kinds.insert(pane_id, kind.to_owned());
        // hocus stays authoritative for the overrides; the daemon holds a copy
        // only to classify with, so it needs the new one before its next poll.
        self.subscribe_to_agentd();
        format!("registered pane {pane_id} as {kind}")
    }

    /// Every agent, with the tail of what each one is showing.
    pub(crate) fn agent_list(&self, args: &tools::Args) -> String {
        let selected = args.project.as_deref().or(self.active_project.as_deref());
        let project = match selected {
            Some(name) => match self.projects.get(name) {
                Some(project) => Some(project),
                None => return format!("unknown project {name}; call project_list"),
            },
            None => None,
        };
        let panes: Vec<agents::AgentPane> = self
            .panes
            .iter()
            .filter(|pane| {
                project.is_none_or(|project| {
                    pane.cwd
                        .as_deref()
                        .and_then(|cwd| self.project_for_cwd(cwd))
                        .is_some_and(|owner| owner.name == project.name)
                })
            })
            .cloned()
            .collect();
        agents::list_json(
            &panes,
            &self.manual_kinds,
            &self.activity,
            &self.pane_projects(),
            &self.pane_branches(),
            |pane_id| read_pane_text(pane_id),
        )
    }

    /// `agent_send` and `agent_control`, which do their own work through the tool
    /// that already does it. Nothing here talks to the host directly.
    pub(crate) fn agent_act(&mut self, call: &ToolCall, args: &tools::Args) -> String {
        match self.agent_host_call(call, args) {
            Ok(host) => tools::dispatch(&host),
            Err(error) => error,
        }
    }

    /// Open an agent pane through the tool that already opens panes, then register
    /// the kind the caller gave against the pane id `new_pane` reports.
    pub(crate) fn agent_spawn(&mut self, args: &tools::Args) -> String {
        if args.command_words().is_empty() {
            return "command is required".to_owned();
        }
        if let Some(kind) = args.kind.as_deref() {
            if !agents::valid_kind(kind) {
                return "kind must be 1 to 32 characters of a-z, 0-9, _ or -".to_owned();
            }
        }
        let cwd = match self.agent_cwd(args) {
            Ok(cwd) => cwd,
            Err(error) => return error,
        };
        let host = ToolCall {
            id: String::new(),
            name: "new_pane".to_owned(),
            arguments: serde_json::json!({
                "command": args.command,
                "args": args.args,
                "cwd": cwd,
            })
            .to_string(),
        };
        let result = tools::dispatch(&host);
        self.register_spawned(args.kind.as_deref(), &result)
    }

    pub(crate) fn agent_cwd(&self, args: &tools::Args) -> Result<Option<String>, String> {
        let selected = args.project.as_deref().or(self.active_project.as_deref());
        let project = match selected {
            Some(name) => match self.projects.get(name) {
                Some(project) => Some(project),
                None => return Err(format!("unknown project {name}; call project_list")),
            },
            None => None,
        };
        if let (Some(project), Some(cwd)) = (project, args.cwd.as_deref()) {
            if !projects::contains(project, cwd) {
                return Err(format!("cwd must be inside project {}", project.name));
            }
        }
        Ok(args
            .cwd
            .clone()
            .or_else(|| project.map(|project| project.root.clone())))
    }

    /// Fold a spawn's kind into `manual_kinds`, keyed by the pane id `new_pane`
    /// reported. The pane is not necessarily in the snapshot yet, so its id stays
    /// pending until one arrives. No kind means detection covers it.
    pub(crate) fn register_spawned(&mut self, kind: Option<&str>, result: &str) -> String {
        let pane_id = serde_json::from_str::<serde_json::Value>(result)
            .ok()
            .and_then(|value| value["pane_id"].as_u64());
        match (kind, pane_id) {
            (Some(kind), Some(pane_id)) => {
                let pane_id = pane_id as u32;
                self.manual_kinds.insert(pane_id, kind.to_owned());
                self.pending_agent_panes.insert(pane_id);
                self.subscribe_to_agentd();
                format!("{result} (registered as {kind})")
            },
            _ => result.to_owned(),
        }
    }

    /// The existing tool call an agent call means, or why it means none.
    ///
    /// Pure, so every mapping is testable on the host triple — which matters, because
    /// this is what keeps `agent_control close` from reaching a pane the user never
    /// called an agent.
    pub(crate) fn agent_host_call(
        &self,
        call: &ToolCall,
        args: &tools::Args,
    ) -> Result<ToolCall, String> {
        let Some(pane_id) = args.pane_id else {
            return Err("pane_id is required".to_owned());
        };
        if !self.is_agent(pane_id) {
            return Err(format!(
                "pane {pane_id} is not an agent; call agent_list, or agent_register it first"
            ));
        }
        // An agent is always a terminal pane, so is_plugin is settled here rather
        // than asked of the model.
        let (name, arguments) = match call.name.as_str() {
            // Empty input is left to send_input, which already refuses it.
            "agent_send" => (
                "send_input",
                serde_json::json!({
                    "pane_id": pane_id,
                    "is_plugin": false,
                    "text": args.text,
                    "keys": args.keys,
                }),
            ),
            _ => match args.action.as_deref() {
                Some("interrupt") => (
                    "send_input",
                    serde_json::json!({
                        "pane_id": pane_id,
                        "is_plugin": false,
                        "keys": ["ctrl_c"],
                    }),
                ),
                Some("restart") => (
                    "rerun_pane",
                    serde_json::json!({ "pane_id": pane_id, "is_plugin": false }),
                ),
                Some("close") => (
                    "close_pane",
                    serde_json::json!({ "pane_id": pane_id, "is_plugin": false }),
                ),
                Some(other) => return Err(format!("unknown agent action: {other}")),
                None => return Err("action is required".to_owned()),
            },
        };
        Ok(ToolCall {
            id: call.id.clone(),
            name: name.to_owned(),
            arguments: arguments.to_string(),
        })
    }

    /// Whether this pane is an agent right now: in the last snapshot, and either
    /// registered by hand or detected.
    pub(crate) fn is_agent(&self, pane_id: u32) -> bool {
        self.panes.iter().any(|pane| {
            pane.pane_id == pane_id && agents::classify(pane, &self.manual_kinds).is_some()
        })
    }

    /// Merge an event-fed daemon snapshot.
    pub(crate) fn apply_poll(&mut self, panes: Vec<agents::AgentPane>, activity: agents::Activity) {
        self.activity = activity;
        let kinds_pruned = self.set_panes(panes);
        // Project discovery used to run here off the same snapshot. It lives in
        // hocus-store now, which receives that snapshot directly and pushes the
        // registry back; `refresh_tab_projects` still runs against our copy.
        self.refresh_tab_projects();
        if kinds_pruned {
            self.subscribe_to_agentd();
        }
    }

    /// A worker reply. Only the poll worker sends any.
    pub(crate) fn schedule(&mut self, args: &tools::Args) -> String {
        let Some(prompt) = args
            .prompt
            .clone()
            .filter(|prompt| !prompt.trim().is_empty())
        else {
            return "schedule needs a prompt".to_owned();
        };
        // NaN and anything under the floor both land on the floor: f64::max
        // returns the other operand for NaN.
        let every = args.seconds.unwrap_or(0.0).max(MIN_JOB_SECONDS);

        self.job_seq += 1;
        let id = self.job_seq;
        let prompt_clone = prompt.clone();
        let show = args.show;
        self.jobs.push(Job {
            id,
            every,
            due_in: every,
            prompt,
            show,
            auto_confirm: args.auto_confirm,
        });
        self.arm_timer(JOB_SECONDS);
        self.log(
            "job_schedule",
            serde_json::json!({
                "job_id": id,
                "every": every,
                "prompt": log::truncate(&prompt_clone),
                "show": show,
                "auto_confirm": args.auto_confirm,
            }),
        );
        format!("scheduled job {id} — runs every {every:.0}s until unscheduled")
    }

    pub(crate) fn unschedule(&mut self, id: Option<u64>) -> String {
        let result = match id {
            Some(id) => {
                let before = self.jobs.len();
                self.jobs.retain(|job| job.id != id);
                if self.jobs.len() == before {
                    format!("no job {id}")
                } else {
                    format!("cancelled job {id}")
                }
            },
            None => {
                let count = self.jobs.len();
                self.jobs.clear();
                format!("cancelled {count} scheduled job(s)")
            },
        };
        self.log(
            "job_cancel",
            serde_json::json!({
                "job_id": id,
                "result": log::truncate(&result),
            }),
        );
        result
    }

    /// The kill switch that doesn't need the model to cooperate.
    pub(crate) fn stop_jobs(&mut self) -> bool {
        if self.jobs.is_empty() {
            return false;
        }
        let notice = self.unschedule(None);
        self.messages.push(Message::notice(notice.clone()));
        self.log(
            "notice",
            serde_json::json!({"content": log::truncate(&notice)}),
        );
        self.pin_to_bottom();
        true
    }

    pub(crate) fn confirm_tool(&mut self) -> bool {
        let Some(call) = self.pending.front().cloned() else {
            return false;
        };
        self.log(
            "tool_confirm",
            serde_json::json!({
                "tool_call_id": call.id,
                "tool_name": call.name,
                "decision": "confirm",
            }),
        );
        self.status = Status::Idle;
        if self.run_head(&call) {
            self.pump_tools();
        }
        true
    }

    pub(crate) fn decline_tool(&mut self) -> bool {
        if self.pending.is_empty() {
            return false;
        }
        if let Some(head) = self.pending.front() {
            self.log(
                "tool_confirm",
                serde_json::json!({
                    "tool_call_id": head.id,
                    "tool_name": head.name,
                    "decision": "decline",
                }),
            );
        }
        self.status = Status::Idle;
        // Recorded as the tool's result, not as a notice: the model must see a
        // reply for every call it made or the next request is malformed.
        self.finish_head("declined by the user");
        self.pump_tools();
        true
    }

    /// Give up on the whole run. Every outstanding call still gets an answer, so
    /// the transcript stays valid if the conversation continues.
    ///
    /// This is also the escape hatch for a dobbs call that never returns,
    /// which is why there is no timer.
    pub(crate) fn abort_tools(&mut self) -> bool {
        self.status = Status::Idle;
        // The model asked, but it is not getting an answer — the results are
        // recorded for the transcript's sake, not to be sent back.
        self.owes_reply = false;
        self.job_auto_confirm = false;
        while !self.pending.is_empty() {
            self.finish_head("aborted by the user");
        }
        self.messages.push(Message::notice("tools aborted"));
        self.log("notice", serde_json::json!({"content": "tools aborted"}));
        self.log("abort", serde_json::json!({"reason": "user"}));
        self.pin_to_bottom();
        true
    }
}
