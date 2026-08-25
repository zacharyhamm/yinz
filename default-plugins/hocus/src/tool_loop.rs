use crate::message::{Job, Message, Status};
use crate::worker::read_pane_text;
use crate::*;
use hocus_proto::location::Location;
use std::collections::BTreeMap;
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
            // Belt and braces. Planning mode's real enforcement is the tool
            // array: a withheld tool is one the model was never told about. But
            // nothing stops it naming one anyway - from the transcript, or from
            // whatever it thinks it remembers - and `run_head` would dispatch
            // that name as readily as any other.
            //
            // Here rather than in `run_head`, which is what exempts the user:
            // `run_manual` calls `run_head` directly, and typing `/close_pane`
            // is a person acting, not the model slipping the mode.
            if self.is_planning() && !tools::READ_ONLY.contains(&call.name.as_str()) {
                let refusal = format!(
                    "refused: planning mode is on, so `{}` is not available. \
                     Finish the plan; the user can approve it and you will get the tool back.",
                    call.name
                );
                self.log(
                    "plan_refusal",
                    serde_json::json!({"tool_call_id": call.id, "tool_name": call.name}),
                );
                self.finish_head(refusal);
                continue;
            }
            let needs_confirmation = tools::call_needs_confirmation(&call);
            if needs_confirmation && !self.job_auto_confirm {
                self.status = Status::ConfirmingTool;
                self.pin_to_bottom();
                // The prompt renders into the sidebar, and the sidebar is very
                // often not the thing being looked at: Esc hides it, a parked
                // chat can be confirming while another is in the foreground, and
                // a scheduled run with `show: false` never opens it at all. A
                // toast is drawn over whatever pane you are actually in, so the
                // ask cannot stall unseen with every other job queued behind it.
                notify(&tools::confirmation_toast(&call));
                // And for the calls that name a pane, the pane itself: the toast
                // says "types into pane 7", this is what puts pane 7 in front of
                // you, wearing a label, before you agree to it.
                self.raise_spotlight(&call);
                return;
            }
            if needs_confirmation {
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

        let Ok(cfg) = self.cfg.clone() else {
            self.job_auto_confirm = false;
            return;
        };
        // Tool results make a valid boundary for steering: every call from the
        // assistant turn is answered, but its follow-up request has not left.
        if self.start_queued_question(&cfg) {
            return;
        }

        // Every call answered: hand the results back so the model can use them.
        // The request at exactly the budget goes out without tools (see
        // `start_request`), so getting past it means the model called a tool it
        // was never offered. Nothing left to try.
        self.tool_rounds += 1;
        let budget = self.max_tool_rounds();
        if self.tool_rounds > budget {
            self.job_auto_confirm = false;
            let err = format!("stopped after {budget} rounds of tool calls without a reply");
            self.messages.push(Message::error(err.clone()));
            self.log("error", serde_json::json!({"error": log::truncate(&err)}));
            return;
        }
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
        // Same two reasons: the API key comes from the resolved config, and the
        // answer arrives later as a `WebRequestResult`.
        if tools::LINEAR_TOOLS.contains(&call.name.as_str()) {
            return self.start_linear(call);
        }
        // Same reason: the dobbs binary comes from the config.
        if tools::SLACK_TOOLS.contains(&call.name.as_str()) {
            return self.start_slack(call);
        }
        // These need the active project, which dispatch cannot reach, and read
        // files no wasm plugin can open — so they defer on the host as well.
        if tools::PROJECT_READ_TOOLS.contains(&call.name.as_str()) {
            return self.start_project_read(call);
        }
        // Same shape, rooted at the user's plugin folder rather than at a project: the path
        // comes from the host, and a wasm plugin cannot open a file in a config dir either.
        if tools::PLUGIN_TOOLS.contains(&call.name.as_str()) {
            return self.start_plugin_tool(call);
        }
        // The todo store lives in another plugin now, so these defer on its
        // reply the way a command defers on the host's.
        if matches!(call.name.as_str(), "todo_add" | "todo_done" | "todo_edit") {
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
            let help = present::commands();
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
        // Above the `tools_busy` guard below: toggling changes what the *next*
        // request advertises, so it is legal while a call is still running.
        if line == "/plan" || line == "/plan on" || line == "/plan off" {
            return self.toggle_plan(match line {
                "/plan on" => Some(true),
                "/plan off" => Some(false),
                _ => None,
            });
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
                // Only what the *user* typed is presented. A result the model
                // asked for stays exactly as it was: it is context, not an
                // answer, and it is already elided to three lines.
                let display = present::present(&call, &result_str);
                Message::tool_result_shown(&call.id, result_str.clone(), display)
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
        let Ok((search_url, search_proxy)) = self
            .cfg
            .as_ref()
            .map(|cfg| (cfg.search_url.clone(), cfg.search_proxy.clone()))
        else {
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
            search_proxy,
        );
        self.status = Status::AwaitingSearch;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// Ask Linear. Deferred on a `WebRequestResult` exactly like `start_search`.
    ///
    /// The tools stay advertised whether or not a key is configured — the Slack
    /// tools and `web_search` are advertised without dobbs or SearxNG too — so
    /// "not configured" is answered here, as a tool result the model can read,
    /// rather than by withholding the tool.
    pub(crate) fn start_linear(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        // Cloned so the request can be issued without holding a borrow of self.
        let configured = self
            .cfg
            .as_ref()
            .ok()
            .map(|cfg| (cfg.linear_api_key.clone(), cfg.linear_api_key_env.clone()));
        let Some((key, key_env)) = configured else {
            self.finish_head("hocus is not configured");
            return true;
        };
        let Some(key) = key else {
            // Names the variable actually configured, not the default, and
            // repeats the caveat `ConfigError` gives for the LLM key: the
            // environment we see is the session's, captured when it started.
            self.finish_head(format!(
                "Linear is not configured: ${key_env} is not set.\n\nSet it in the \
                 environment the Zellij session was started from — exporting it after \
                 launching zellij will not be picked up. Or set linear_api_key_env in the \
                 plugin configuration to name a different variable."
            ));
            return true;
        };
        let body = match linear::request_for(&call.name, &args) {
            Ok(body) => body,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };

        web_request(
            linear::ENDPOINT,
            HttpVerb::Post,
            linear::headers(&key),
            body,
            // Round-trips through the host, which is how the result is matched
            // back to the call that made it.
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
            None,
        );
        self.status = Status::AwaitingLinear;
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

    /// The project a repository tool reads from.
    ///
    /// Unlike the todo tools this takes no `project` argument: these tools
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

    /// Run a file, Git or GitHub reader rooted at the active project. The result
    /// comes back as `RunCommandResult` carrying this call's id, the same way
    /// the Slack tools answer.
    ///
    /// A wasm plugin's WASI preopens are `/host`, `/data`, `/cache` and `/tmp`,
    /// and a project root is none of them, so there is no reading these files
    /// from in here — the host is the only way to reach them.
    ///
    /// Everything that can be refused without running anything is refused here:
    /// no project, a bad argument, or a path outside the root never reaches a
    /// command.
    pub(crate) fn start_project_read(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let argv = match self.project_target().and_then(|project| {
            if tools::FILE_TOOLS.contains(&call.name.as_str()) {
                // `path` is the file for `read_file` and the folder to search
                // under for the other two; either way it is rooted here.
                let target = tools::resolve_in_root(project, args.path.as_deref())?;
                tools::file_argv(&call.name, &args, &target)
            } else if tools::GIT_TOOLS.contains(&call.name.as_str()) {
                let target = tools::resolve_in_root(project, args.path.as_deref())?;
                tools::git_argv(&call.name, &args, project, &target)
            } else {
                tools::github_argv(&args, project)
            }
        }) {
            Ok(argv) => argv,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };

        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_command_with_output_limit(
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
            MAX_TOOL_OUTPUT,
        );
        self.status = Status::AwaitingCommand;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// Run one of the plugin-authoring tools.
    ///
    /// One of the five answers here and now — `plugin_docs`, from compiled-in text. The other
    /// four shell out and answer later on `RunCommandResult`, exactly as the file tools do.
    /// `plugin_reload` is among them even though `start_or_reload_plugin` is fire-and-forget:
    /// what it waits for is `PLUGIN_SYMLINK_GUARD`, which only the host can run.
    ///
    /// Everything refusable without running anything is refused here: no plugin folder, a bad
    /// name, a source over the size cap.
    pub(crate) fn start_plugin_tool(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };

        // Needs no folder and touches no disk, so it answers even when nothing is configured.
        if call.name == "plugin_docs" {
            self.finish_head(lua_docs::docs(args.section.as_deref()));
            return true;
        }

        let dir = self.user_plugin_dir.clone();
        // `plugin_list` names no plugin - it is the folder itself that is the target.
        let target = match call.name.as_str() {
            "plugin_list" => match dir.trim().is_empty() {
                true => Err(tools::NO_PLUGIN_FOLDER.to_owned()),
                false => Ok(dir.clone()),
            },
            _ => tools::resolve_plugin_file(&dir, args.name.as_deref()),
        };
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };

        let (argv, env) = match tools::plugin_argv(&call.name, &args, &dir, &target) {
            Ok(argv) => argv,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };

        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_command_with_output_limit(
            &borrowed,
            env,
            // `.` for the reason spelled out in `start_project_read`: the host rewrites a cwd
            // under `/tmp`, `/host`, `/data` or `/cache` into the plugin's own folder of that
            // name, and every path in the argv is absolute anyway.
            std::path::PathBuf::from("."),
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
            MAX_TOOL_OUTPUT,
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

    /// The deferred half of the Linear tools, and the twin of `finish_search`.
    ///
    /// The status guard is what keeps the two apart: a `WebRequestResult` is
    /// only ours while a Linear call is the one outstanding.
    pub(crate) fn finish_linear(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !matches!(self.status, Status::AwaitingLinear) {
            return false;
        }
        // A call we already aborted, or someone else's request.
        let Some(head) = self.pending.front() else {
            return false;
        };
        if context.get("tool_call_id") != Some(&head.id) {
            return false;
        }

        let name = head.name.clone();
        let key_env = self
            .cfg
            .as_ref()
            .map(|cfg| cfg.linear_api_key_env.clone())
            .unwrap_or_else(|_| linear::DEFAULT_API_KEY_ENV.to_owned());
        let result = linear::format(&name, status, body, &key_env);

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
            self.push_strip(false);
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
        // The focused agent's tab before the active one: hocus is a global pane, so
        // "the tab I am in" is a question it cannot answer about itself, and the
        // agent the user was last looking at is the tab they mean.
        let tab_id = match args.tab_id.or_else(|| self.focused_tab()).or_else(|| {
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
        self.push_strip(false);
        let _ = self.write_store(vec![projects::delete_mutation(name)]);
        projects::forgotten(&project)
    }

    pub(crate) fn register_opened_project(&mut self, name: &str, result: &str) -> String {
        let tab_id = serde_json::from_str::<serde_json::Value>(result)
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
            "todo_edit" => {
                let id = args.id.ok_or("id is required")?;
                let text = args.text.as_deref().ok_or("text is required")?;
                let (mutations, text) = todos::plan_edit(&list, id, text)?;
                backend.apply(mutations)?;
                Ok(serde_json::json!({
                    "project": name,
                    "id": id,
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

    /// Take note of which pane the user is looking at.
    ///
    /// Only an agent pane is remembered. Anything else — the sidebar, a shell, an
    /// editor — leaves the last agent standing, which is what makes the active
    /// project survive the user coming here to ask about it.
    pub(crate) fn note_focus(&mut self, focused: Option<u32>) {
        if focused.is_some_and(|pane_id| self.is_agent(pane_id)) {
            self.last_focused_agent = focused;
        }
        // A pane that closed names nothing. Same rule as the maps in `set_panes`.
        self.last_focused_agent = self
            .last_focused_agent
            .filter(|pane_id| self.panes.iter().any(|pane| pane.pane_id == *pane_id));
    }

    /// The tab of the agent pane the user last had focused.
    fn focused_tab(&self) -> Option<usize> {
        let pane_id = self.last_focused_agent?;
        let pane = self.panes.iter().find(|pane| pane.pane_id == pane_id)?;
        pane.tab.as_ref().map(|(tab_id, _)| *tab_id)
    }

    /// The project of the agent pane the user last had focused.
    ///
    /// The tab's assignment first and the pane's folder second, so that the
    /// precedence is the one `tab_project` already sets: what the user said by hand
    /// outranks what was worked out from a checkout.
    fn focused_project(&self) -> Option<String> {
        let pane_id = self.last_focused_agent?;
        let pane = self.panes.iter().find(|pane| pane.pane_id == pane_id)?;
        pane.tab
            .as_ref()
            .and_then(|(tab_id, _)| self.tab_projects.get(tab_id))
            .cloned()
            .or_else(|| {
                self.project_for_cwd(&Self::pane_location(pane)?)
                    .map(|project| project.name.clone())
            })
    }

    /// The folder a pane is in, with the machine it is on.
    ///
    /// Both halves come off the pane: the daemon puts the folder there and the
    /// manifest puts the host there. Keeping them together is what stops two
    /// panes sitting in the same path on different machines from being read as
    /// one folder.
    fn pane_location(pane: &agents::AgentPane) -> Option<Location> {
        Some(Location::new(pane.host.clone(), pane.cwd.as_deref()?))
    }

    /// Pane id to project name, for each agent whose folder a project owns.
    pub(crate) fn pane_projects(&self) -> BTreeMap<u32, String> {
        self.panes
            .iter()
            .filter(|pane| agents::classify(pane, &self.manual_kinds).is_some())
            .filter_map(|pane| {
                let owner = self.project_for_cwd(&Self::pane_location(pane)?)?;
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
                let branch = self.branch_for_cwd(&Self::pane_location(pane)?)?;
                Some((pane.pane_id, branch.to_owned()))
            })
            .collect()
    }

    /// The branch of the folder itself, never of a folder above it.
    ///
    /// The stand-in `repository_for_cwd` offers is right about the project and
    /// wrong about the branch, and from the probed folders alone the two cases
    /// cannot be told apart: a checkout at `/repo` with a pane in `/repo/src`
    /// and a checkout at `/repo` with a *worktree* at `/repo/.claude/worktrees/x`
    /// present the same two paths and the same single answer. Git's own layout
    /// puts worktrees wherever it is asked to, and inside the primary checkout
    /// is an ordinary place to put them.
    ///
    /// So this asks only about the folder in hand. A pane whose folder has no
    /// answer yet groups under "no branch" for the round trip it takes to get
    /// one — where the stand-in would have filed it under the primary's branch,
    /// which is a different agent's work. A header that is briefly missing can
    /// be waited out; one that is confidently wrong cannot.
    fn branch_for_cwd(&self, cwd: &Location) -> Option<&str> {
        self.repositories.get(cwd)?.branch.as_deref()
    }

    /// What Git said about an agent's folder, or about the nearest folder above
    /// it that we have an answer for.
    ///
    /// The daemon probes each folder an agent is in, so an agent that `cd`s into
    /// a subdirectory of its checkout has no answer of its own until a fresh
    /// probe lands. It is in the same checkout it was in a moment ago, and on
    /// screen the wait reads as the agent losing its project and getting it back.
    /// The exact answer replaces this stand-in as soon as it arrives.
    ///
    /// For the project only. The branch goes through `branch_for_cwd`, which
    /// takes no stand-in — see the reason there.
    fn repository_for_cwd(&self, cwd: &Location) -> Option<&projects::Repository> {
        if let Some(repository) = self.repositories.get(cwd) {
            return Some(repository);
        }
        self.repositories
            .iter()
            // Component-wise, and on the same machine — the rule
            // `Location::starts_with` puts on a folder, so `/work/hocus-old` is
            // not below `/work/hocus` and neither is `/work/hocus/src` on
            // another host.
            .filter(|(probed, _)| cwd.starts_with(probed))
            .max_by_key(|(probed, _)| probed.depth())
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
    ///
    /// For a pane on another machine there is no second answer, and
    /// `projects::contains` is what says so: a project's root is a path here,
    /// and a folder over there that spells the same path is a different place.
    /// A remote agent is matched by its repository's remote url or not at all —
    /// which is right, because a build box laid out like the laptop would
    /// otherwise put every remote agent in whichever local project sits at its
    /// path.
    fn project_for_cwd(&self, cwd: &Location) -> Option<&projects::Project> {
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

    /// Work out the project of each tab again, and the active project with it.
    ///
    /// The one place the active project is derived. The focused agent decides it;
    /// the active tab is the fallback for a session where nobody has focused an
    /// agent yet, and for a tab the user assigned by hand.
    pub(crate) fn refresh_tab_projects(&mut self) {
        let owners = self.pane_projects();
        self.auto_tab_projects = agents::tab_projects(&self.panes, &self.manual_kinds, &owners);
        // A tab the user assigned is left alone, so the two maps stay disjoint.
        self.auto_tab_projects
            .retain(|tab_id, _| !self.tab_projects.contains_key(tab_id));
        // A closed tab is still named by the panes of the last snapshot, so without
        // this it comes straight back. Only when we have heard of any tab at all:
        // an empty list is "nobody has told us" — hocus is a global pane and an
        // older host sends it no `TabUpdate` — and is not the same as "no tabs".
        if !self.tabs.is_empty() {
            self.auto_tab_projects
                .retain(|tab_id, _| self.tabs.iter().any(|tab| tab.tab_id == *tab_id));
        }
        self.active_project = self.focused_project().or_else(|| {
            self.tabs
                .iter()
                .find(|tab| tab.active)
                .and_then(|tab| self.tab_project(tab.tab_id))
                .map(str::to_owned)
        });
        self.refresh_tab_project_labels();
    }

    /// Work out what each tab should be called, and tell the host when it changes.
    ///
    /// The host does the naming itself from here: it knows the tab order the
    /// `#1`/`#2` of two tabs on one project are read in, and it knows which tabs
    /// the user has named and must be left alone. All it is missing is what a
    /// project is, which is the one thing this plugin can say.
    fn refresh_tab_project_labels(&mut self) {
        let owners = self.pane_projects();
        let detected = agents::tab_project_labels(&self.panes, &self.manual_kinds, &owners);
        let mut labels: BTreeMap<usize, String> = detected
            .into_iter()
            // Two projects in one tab name it after both. A tab is a place to
            // work, not a thing that belongs to one project.
            .map(|(tab_id, projects)| (tab_id, projects.join(" + ")))
            .collect();
        // An assignment the user made outranks what the agents in the tab suggest,
        // the same way it does everywhere else.
        for (tab_id, project) in &self.tab_projects {
            labels.insert(*tab_id, project.clone());
        }
        if !self.tabs.is_empty() {
            labels.retain(|tab_id, _| self.tabs.iter().any(|tab| tab.tab_id == *tab_id));
        }
        if labels != self.tab_project_labels {
            self.tab_project_labels = labels;
            self.publish_tab_projects();
        }
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
        self.send_manual_kinds();
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
                    Self::pane_location(pane)
                        .and_then(|location| self.project_for_cwd(&location))
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
            &self.summaries,
            read_pane_text,
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

    /// Start an agent, and register the kind the caller gave against the pane id.
    ///
    /// Two routes to a pane, one decision about what goes in it. `launch::plan` says
    /// what to run for both — so `agent=cursor model=x prompt=y` means the same
    /// argv here as it does behind the New agent modal's button — and the routes
    /// differ only in where the pane lands:
    ///
    /// - a checkout to make means git work first, which is `launch::open`: the
    ///   modal's own call, with the same worktree defaults and the same errors;
    /// - nothing to make keeps this on `new_pane`, which is what it always did. Going
    ///   through `launch::open` for those too would look tidier and be worse: it
    ///   resolves the repository root, so `command=zsh cwd=/tmp` would start
    ///   failing, and `floating`/`in_new_tab` would quietly stop working.
    pub(crate) fn agent_spawn(&mut self, args: &tools::Args) -> String {
        let spec = args.launch_spec();
        // What to run *and* which route to take are both `launch`'s to say — down to
        // which of `agent` and `command` is missing when both are. Routing on the
        // checkout rather than on `args.branch` is what keeps the two in step: a
        // branch of nothing but spaces is no branch to `launch`, and would otherwise
        // send this down the git route to do no git work.
        let (command, checkout) = match launch::plan(&spec) {
            Ok(planned) => planned,
            Err(error) => return error,
        };
        if let Some(kind) = args.kind.as_deref() {
            if !agents::valid_kind(kind) {
                return "kind must be 1 to 32 characters of a-z, 0-9, _ or -".to_owned();
            }
        }
        let cwd = match self.agent_cwd(args) {
            Ok(cwd) => cwd,
            Err(error) => return error,
        };
        let result = if matches!(checkout, GitCheckout::Current) {
            let host = ToolCall {
                id: String::new(),
                name: "new_pane".to_owned(),
                arguments: serde_json::json!({
                    // From the plan, not from `args`: `agent`, `model` and `prompt`
                    // are all in here, and none of them is a key `new_pane` knows.
                    "command": command.path.display().to_string(),
                    "args": command.args,
                    "cwd": cwd,
                })
                .to_string(),
            };
            tools::dispatch(&host)
        } else {
            tools::spawn_in_checkout(args, cwd)
        };
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
            if !projects::contains(project, &Location::local(cwd)) {
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
                self.send_manual_kinds();
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

    /// The agent selected in the agents pane and what the daemon last made of it, for
    /// the inset drawn at the head of the transcript.
    ///
    /// Drawn only while the agents pane itself holds the focus. That is the moment the
    /// note is for: the user is working the list, deciding which agent to walk into,
    /// and the box says what the one under the cursor was last seen doing. Standing in
    /// an agent, or here in the chat, it is not what anyone is reading, so it goes.
    ///
    /// No idleness gate, unlike `waiting_marker` and the strip's `!`: what is asked for
    /// here is the *last* thing the daemon made of this agent, and an agent that has
    /// gone back to work still has one. The header carries the pane id, so what the box
    /// claims to be is an account of a moment, not of right now.
    pub(crate) fn selected_summary(&self) -> Option<(u32, &str, &agents::Summary)> {
        if self.focused_plugin.is_none() || self.focused_plugin != self.agents_plugin {
            return None;
        }
        let pane_id = self.agents_selected?;
        let summary = self.summaries.get(&pane_id)?;
        let pane = self.panes.iter().find(|pane| pane.pane_id == pane_id)?;
        let (kind, _) = agents::classify(pane, &self.manual_kinds)?;
        Some((pane_id, kind, summary))
    }

    /// Take the agents pane's cursor. Answers whether the inset moved with it.
    pub(crate) fn on_selection(&mut self, message: &PipeMessage) -> bool {
        let Some(selection) = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::Selection>(json).ok())
        else {
            return false;
        };
        let inset = self.inset_key();
        self.agents_plugin = Some(selection.plugin_id);
        self.agents_selected = selection.pane_id;
        self.inset_key() != inset
    }

    /// What the inset is showing, small enough to hold across a snapshot and
    /// compare. The hash names the screen the summary was made from, so a change
    /// here is a change on screen and nothing else is.
    pub(crate) fn inset_key(&self) -> Option<(u32, u64, bool)> {
        self.selected_summary()
            .map(|(pane_id, _, summary)| (pane_id, summary.hash, summary.awaiting))
    }

    /// Merge an event-fed daemon snapshot.
    pub(crate) fn apply_poll(
        &mut self,
        panes: Vec<agents::AgentPane>,
        activity: agents::Activity,
        summaries: agents::Summaries,
        focused: Option<u32>,
    ) {
        self.activity = activity;
        self.summaries = summaries;
        let kinds_pruned = self.set_panes(panes);
        self.note_focus(focused);
        self.focused_pane = focused;
        // Project discovery used to run here off the same snapshot. It lives in
        // hocus-store now, which receives that snapshot directly and pushes the
        // registry back; `refresh_tab_projects` still runs against our copy.
        self.refresh_tab_projects();
        // Only the overrides go back, never a watermark reset: we are *inside*
        // the merge of a snapshot whose `seq` we just recorded, and forgetting it
        // here would re-admit the older snapshots still in flight — one of which
        // still holds the pane whose closing caused this prune in the first
        // place, which is how a closed agent came back.
        if kinds_pruned {
            self.send_manual_kinds();
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

    /// Point at the pane `call` is about, if it names one, and remember that we
    /// did so the mark can come down again.
    ///
    /// Nothing here is fallible from this side: the host works out how much of
    /// the pane it can actually reveal without moving the keyboard, and a pane
    /// that closed between the model asking and this running is simply not found.
    pub(crate) fn raise_spotlight(&mut self, call: &ToolCall) {
        self.drop_spotlight();
        let Some(pane) = tools::confirmation_target(call) else {
            return;
        };
        tools::raise_spotlight(pane, tools::indicator_label(call));
        self.spotlight = Some(pane);
    }

    /// Take the mark back off whatever is wearing it. Every way out of a
    /// confirmation goes through here, so a new one has an obvious place to call
    /// and a pane cannot be left labelled for an answer that has been given.
    pub(crate) fn drop_spotlight(&mut self) {
        if let Some(pane) = self.spotlight.take() {
            tools::drop_spotlight(pane);
        }
    }

    /// Turn planning mode on or off. `want` is `None` for a plain `/plan`,
    /// which toggles.
    ///
    /// Leaving it from `AwaitingApproval` is the same move as leaving it from
    /// `Planning`: the plan stays in the transcript, and the question about it
    /// stops being asked.
    pub(crate) fn toggle_plan(&mut self, want: Option<bool>) -> bool {
        let on = want.unwrap_or(matches!(self.plan, Plan::Off));
        self.pin_to_bottom();
        if on == !matches!(self.plan, Plan::Off) {
            // Idempotent on purpose: `/plan off` twice is a thing people type.
            let notice = if on {
                "already planning"
            } else {
                "not planning"
            };
            self.messages.push(Message::notice(notice));
            self.log("notice", serde_json::json!({"content": notice}));
            return true;
        }

        // The mode changes *first*, and the pending call is dealt with second.
        // `decline_tool` answers the call and pumps the queue, and an emptied
        // queue starts the model's follow-up request — which would go out with
        // the whole tool set if the mode were still off when it left. Setting
        // the mode first also puts the rest of the queue behind `pump_tools`'
        // own read-only gate, so a batch is refused rather than half-run.
        self.plan = if on { Plan::Planning } else { Plan::Off };

        // Turning it on while a destructive call is waiting on a y/n would leave
        // that call armed inside the mode that exists to disarm it. Declining is
        // the honest reading of "stop acting, start planning", and it keeps the
        // transcript valid — the model gets a result for the call it made.
        let declined = on && self.is_confirming() && self.decline_tool();

        let notice = match (on, declined) {
            (true, true) => {
                "planning mode on — the pending call was declined; read-only tools until you \
                 approve a plan"
            },
            (true, false) => "planning mode on — read-only tools until you approve a plan",
            (false, _) => "planning mode off",
        };
        self.messages.push(Message::notice(notice));
        self.log("plan_mode", serde_json::json!({"on": on}));
        self.log("notice", serde_json::json!({"content": notice}));
        true
    }

    /// `y` on a plan: leave planning mode and let the model carry out what it
    /// just wrote.
    ///
    /// The approval goes in as a `Role::User` message rather than a notice
    /// because a notice is dropped on the way out (`api::request_body_with_project`
    /// skips `Role::Notice`), so the model would be handed two assistant turns
    /// in a row and nothing that asked for the second.
    pub(crate) fn approve_plan(&mut self) -> bool {
        if !self.awaiting_plan() {
            return false;
        }
        self.plan = Plan::Off;
        self.pin_to_bottom();
        self.log("plan_decision", serde_json::json!({"decision": "approve"}));
        let notice = "planning mode off — carrying out the plan";
        self.messages.push(Message::notice(notice));
        self.log("notice", serde_json::json!({"content": notice}));
        let content = "Approved. Carry out the plan you just wrote.";
        self.messages.push(Message::user(content));
        self.log("user_input", serde_json::json!({"content": content}));
        let Ok(cfg) = self.cfg.clone() else {
            return true;
        };
        // A fresh budget, the same as any other new question: the research is
        // spent, and the work is what the rounds are for now.
        self.tool_rounds = 0;
        self.start_request(&cfg);
        true
    }

    /// `n` on a plan: keep planning. The plan stays where it is, and the next
    /// thing typed refines it.
    pub(crate) fn decline_plan(&mut self) -> bool {
        if !self.awaiting_plan() {
            return false;
        }
        self.plan = Plan::Planning;
        self.pin_to_bottom();
        self.log("plan_decision", serde_json::json!({"decision": "decline"}));
        let notice = "still planning — say what to change, or /plan to leave";
        self.messages.push(Message::notice(notice));
        self.log("notice", serde_json::json!({"content": notice}));
        true
    }

    pub(crate) fn confirm_tool(&mut self) -> bool {
        let Some(call) = self.pending.front().cloned() else {
            return false;
        };
        self.drop_spotlight();
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
        self.drop_spotlight();
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
        self.drop_spotlight();
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
