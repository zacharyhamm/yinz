use crate::message::{Job, Message, Status};
use crate::worker::read_pane_text;
use crate::*;
use hocus_proto::location::Location;
use hocus_proto::notes;
use hocus_proto::run_history;
use hocus_proto::store::Backend;
use std::collections::BTreeMap;
use zellij_tile::prelude::TabId;
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
            if self.is_planning() && !self.tool_is_read_only(&call.name) {
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
            // A plugin the loader would refuse is refused here, before anyone is asked to
            // approve writing it: the compile error is the whole answer.
            if let Some(refusal) = tools::uncompilable_write(&call) {
                self.finish_head(refusal);
                continue;
            }
            // Likewise an edit whose old_string is not there: the diagnosis is the whole
            // answer, and a prompt to approve a change that cannot be made is noise.
            if let Some(refusal) = self.inapplicable_edit(&call) {
                self.finish_head(refusal);
                continue;
            }
            if crate::workflows::TOOLS.contains(&call.name.as_str()) {
                if let Err(error) = crate::workflows::arguments(&call) {
                    self.finish_head(format!("invalid workflow arguments: {error}"));
                    continue;
                }
            }
            let needs_confirmation = self.call_needs_confirmation(&call);
            // A scheduled run with no authority of its own does not park: nobody
            // may be there to press `y`, and a parked run would hold every other
            // job in this chat behind it. The call is answered, not silently
            // dropped - the model must see a reply for every call it made.
            if let (true, Some(run)) = (needs_confirmation, self.scheduled_run) {
                if !run.auto_confirm {
                    self.log(
                        "tool_confirm",
                        serde_json::json!({
                            "tool_call_id": call.id,
                            "tool_name": call.name,
                            "decision": "scheduled_decline",
                            "job_id": run.job_id,
                        }),
                    );
                    self.finish_head(format!(
                        "declined: `{}` needs confirmation and this is a scheduled run, which \
                         cannot ask. Schedule the job with auto_confirm to allow it, or leave \
                         this for the user.",
                        call.name
                    ));
                    continue;
                }
            }
            if needs_confirmation && self.scheduled_run.is_none() {
                if !self.start_tool_review(&call) {
                    self.show_tool_confirmation(&call);
                }
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
            // `true` means the head was answered and popped. One that was not - a handler
            // that deferred but said otherwise - would send this loop straight back into it,
            // and `start_plugin_load` once did exactly that: the same reload was asked of the
            // host thousands of times a second without `update()` ever returning, and every
            // plugin in the session froze behind it. So the contract is checked here, once
            // per iteration, and a breach is answered as a refusal rather than looped on.
            if self.pending.front().is_some_and(|head| head.id == call.id) {
                self.finish_head(format!(
                    "refused: `{}` neither answered nor deferred; this is a bug in hocus",
                    call.name
                ));
            }
        }

        // A rate limit can arrive while the last tool is using `Status`. Once that tool
        // releases it, the resend owns the only request slot; queued asks and follow-ups wait
        // until the retry succeeds.
        if self.retry_armed() {
            self.status = Status::AwaitingRetry;
            return;
        }

        // Only the user's own calls were in the queue: nothing to send.
        self.status = Status::Idle;
        if !std::mem::take(&mut self.owes_reply) {
            return;
        }

        let Ok(cfg) = self.cfg.clone() else {
            self.scheduled_run = None;
            return;
        };
        // Tool results make a valid boundary for steering: every call from the
        // assistant turn is answered, but its follow-up request has not left.
        if self.start_queued_question(&cfg) {
            return;
        }

        // Every call answered: hand the results back so the model can use them,
        // unless this batch spent the turn's budget.
        self.tool_rounds += 1;
        let budget = self.max_tool_rounds();
        if self.tool_rounds >= budget {
            self.scheduled_run = None;
            let err = format!(
                "stopped after {budget} rounds of tool calls; type continue to start a fresh budget"
            );
            self.messages.push(Message::error(err.clone()));
            self.log("error", serde_json::json!({"error": log::truncate(&err)}));
            return;
        }
        self.start_request(&cfg);
    }

    /// Dispatch the head. Returns whether the queue advanced — `false` means the
    /// result is deferred and `pump_tools` must stop and wait.
    pub(crate) fn run_head(&mut self, call: &ToolCall) -> bool {
        if crate::workflows::TOOLS.contains(&call.name.as_str()) {
            return self.start_workflow_tool(call);
        }
        if matches!(
            call.name.as_str(),
            "project_config_read" | "project_config_write"
        ) {
            let result = (|| {
                let args = tools::Args::parse(&call.arguments)?;
                let root = self.project_target()?.root.clone();
                let snapshot = if call.name == "project_config_write" {
                    zellij_tile::shim::project_config_write(
                        root.into(),
                        args.source.ok_or("source is required")?,
                        args.expected,
                    )
                } else {
                    zellij_tile::shim::project_config_read(root.into())
                }?;
                serde_json::to_string(&snapshot).map_err(|e| e.to_string())
            })();
            self.finish_head(result.unwrap_or_else(|e: String| e));
            return true;
        }

        if self.extensions.tool(&call.name).is_some() {
            return self.start_extension_tool(call);
        }
        // Needs the resolved config and defers on a web request, neither of
        // which tools::dispatch can reach.
        if call.name == "web_search" {
            return self.start_search(call);
        }
        if call.name == "openrouter_decisions" {
            return self.start_decisions(call);
        }
        if tools::SWAMP_TOOLS.contains(&call.name.as_str()) {
            return self.start_swamp_issue(call);
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
        if tools::SKILL_TOOLS.contains(&call.name.as_str()) {
            return self.start_skill_tool(call);
        }
        // Same shape again, pointed at the `zellij` CLI: a wasm plugin cannot run the
        // Firecracker provider, and the CLI already knows how.
        if tools::VM_TOOLS.contains(&call.name.as_str()) {
            return self.start_vm_tool(call);
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
        if matches!(
            call.name.as_str(),
            "note_add" | "note_edit" | "note_archive" | "note_delete" | "note_link"
        ) {
            return self.start_note_write(call);
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

    pub(crate) fn tool_is_read_only(&self, name: &str) -> bool {
        tools::READ_ONLY.contains(&name)
            || self
                .extensions
                .tool(name)
                .is_some_and(|tool| tool.safety == extensions::Safety::ReadOnly)
    }

    pub(crate) fn call_needs_confirmation(&self, call: &ToolCall) -> bool {
        self.extensions.tool(&call.name).map_or_else(
            || tools::call_needs_confirmation(call),
            |tool| tool.safety == extensions::Safety::Confirm,
        )
    }

    pub(crate) fn confirmation_body(&self, call: &ToolCall) -> Vec<(String, bool)> {
        let Some(tool) = self.extensions.tool(&call.name) else {
            let mut lines = Vec::new();
            if let Some(consequence) = self.confirmation_consequence(call) {
                lines.push((consequence, true));
            }
            lines.extend(
                tools::argument_lines(call)
                    .into_iter()
                    .map(|line| (line, false)),
            );
            return lines;
        };
        let mut lines = Vec::new();
        if let Some(consequence) = &tool.consequence {
            lines.push((consequence.clone(), true));
        }
        lines.extend(
            tools::argument_lines(call)
                .into_iter()
                .map(|line| (line, false)),
        );
        lines
    }

    pub(crate) fn confirmation_toast(&self, call: &ToolCall) -> String {
        let Some(tool) = self.extensions.tool(&call.name) else {
            let mut lines = vec![tools::TOAST_HEADLINE.to_owned(), call.name.clone()];
            if let Some(consequence) = self.confirmation_consequence(call) {
                lines.push(format!("  {consequence}"));
            }
            lines.extend(
                tools::toast_argument_lines(call)
                    .into_iter()
                    .map(|line| format!("  {line}")),
            );
            return lines.join("\n");
        };
        let mut lines = vec![tools::TOAST_HEADLINE.to_owned(), call.name.clone()];
        if let Some(consequence) = &tool.consequence {
            lines.push(format!("  {consequence}"));
        }
        lines.extend(
            tools::toast_argument_lines(call)
                .into_iter()
                .map(|line| format!("  {line}")),
        );
        lines.join("\n")
    }

    pub(crate) fn confirmation_consequence(&self, call: &ToolCall) -> Option<String> {
        let mut consequence = tools::consequence(call)?;
        if call.name != "plugin_edit" {
            return Some(consequence);
        }
        let granted = self.plugin_edit_permissions(call);
        if !granted.is_empty() {
            consequence.push_str(&format!(
                ", and grants it: {}",
                tools::permission_names(&granted).join(", ")
            ));
        }
        Some(consequence)
    }

    /// Why a `plugin_edit` cannot be applied to the file as it is, or `None` when it can - or
    /// when nothing can be said yet because the file could not be read, which the tool itself
    /// reports in the ordinary way once it runs.
    fn inapplicable_edit(&self, call: &ToolCall) -> Option<String> {
        if call.name != "plugin_edit" {
            return None;
        }
        let args = tools::Args::parse(&call.arguments).ok()?;
        let fs_path = tools::plugin_fs_path(args.name.as_deref()).ok()?;
        let previous = tools::plugin_fs_read(&fs_path).ok()?;
        tools::edit_source_and_permissions(&previous, &args)
            .and_then(|(edited, _)| {
                tools::formatted_source(
                    args.name.as_deref().unwrap_or_default(),
                    &edited.source,
                    zellij_tile::plugin_authoring::PluginCheckMode::Prepare,
                )
            })
            .err()
    }

    fn plugin_edit_permissions(&self, call: &ToolCall) -> Vec<PermissionType> {
        let Ok(args) = tools::Args::parse(&call.arguments) else {
            return Vec::new();
        };
        let Ok(fs_path) = tools::plugin_fs_path(args.name.as_deref()) else {
            return Vec::new();
        };
        let Ok(previous) = tools::plugin_fs_read(&fs_path) else {
            return Vec::new();
        };
        tools::edit_source_and_permissions(&previous, &args)
            .map(|(_, permissions)| permissions)
            .unwrap_or_default()
    }

    fn start_extension_tool(&mut self, call: &ToolCall) -> bool {
        let Some(tool) = self.extensions.tool(&call.name).cloned() else {
            // `run_head` only gets here after the same lookup succeeded, so this is not
            // reached; it answers the head anyway, because a `true` that leaves the head in
            // place is the shape of the bug `pump_tools` guards against.
            self.finish_head(format!("refused: `{}` is not a tool", call.name));
            return true;
        };
        let arguments: serde_json::Map<String, serde_json::Value> =
            match serde_json::from_str(&call.arguments) {
                Ok(arguments) => arguments,
                Err(error) => {
                    self.finish_head(format!("invalid arguments for {}: {error}", call.name));
                    return true;
                },
            };
        let payload = serde_json::to_string(&extensions::Call {
            version: extensions::VERSION,
            call_id: &call.id,
            name: format!("{}{}", extensions::TOOL_HANDLER_PREFIX, tool.local_name),
            payload: arguments.into(),
        })
        .expect("serializing an extension call cannot fail");
        extensions::call(tool.owner, payload);
        self.status = Status::AwaitingExtension;
        self.extension_call_started_ms = Some(self.mono_ms);
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
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
        // Ahead of every other arm, and ahead of the `tools_busy` guard below:
        // help runs nothing, so there is no state it could be illegal in.
        if let Some(name) = help::split_help_request(line) {
            // A skill is a prompt and takes the rest of the line as its
            // arguments, so `--help` after one is text for the model. Skills
            // fall through to `run_skill`, which is where they are answered.
            if self.skills.get(name).is_none() {
                return match help::lookup(name, &self.extensions) {
                    Some(text) => {
                        self.messages.push(Message::notice(text.clone()));
                        self.log(
                            "notice",
                            serde_json::json!({"content": log::truncate(&text)}),
                        );
                        true
                    },
                    // The same sentence `run_manual_call` gives a typo, so the
                    // two paths into a wrong name read alike.
                    None => {
                        let error = format!(
                            "no such tool or skill: {name} — /help lists the commands, /skills \
                             the skills"
                        );
                        self.messages.push(Message::error(error.clone()));
                        self.log("error", serde_json::json!({"error": error}));
                        true
                    },
                };
            }
        }
        if line == "/" || line == "/help" {
            let help = present::commands_with(self.extensions.names(), self.skills.names());
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
        // The same as Ctrl-l, for whoever reaches for the word instead of the
        // chord: the chat is parked in its tab and an empty one takes the
        // foreground. Legal mid-tool for the same reason the chord is.
        if line == "/clear" {
            return self.new_chat();
        }
        if line == "/keys" {
            self.messages.push(Message::notice(KEYS_HELP));
            self.log(
                "notice",
                serde_json::json!({"content": log::truncate(KEYS_HELP)}),
            );
            return true;
        }
        // The list of skills, or a fresh read of their folders. The reload
        // answers when its scans land, since off the host nothing is known
        // until then.
        if line == "/skills" || line.starts_with("/skills ") {
            let tail = line["/skills".len()..].trim();
            let words: Vec<&str> = tail.split_whitespace().collect();
            if let [action @ ("enable" | "disable"), scope, name] = words.as_slice() {
                return self.run_manual_call(
                    format!("skill_{action}"),
                    serde_json::json!({ "scope": scope, "name": name }).to_string(),
                );
            }
            return match tail {
                "" => {
                    self.show_skills();
                    true
                },
                "reload" => self.reload_skills(),
                _ => {
                    let error = "/skills lists skills; /skills reload rescans; /skills \
                                 enable|disable <user|project> <name> changes one. To run one, \
                                 type /<skill> <text>.";
                    self.messages.push(Message::error(error));
                    self.log("error", serde_json::json!({"error": error}));
                    true
                },
            };
        }
        // Above the `tools_busy` guard below: entering the mode changes what the
        // *next* request advertises, so it is legal while a call is still
        // running, and the prompt itself queues like any other typed message.
        if line == "/plan" || line.starts_with("/plan ") {
            let prompt = line["/plan".len()..].trim();
            // The way out of the mode, and the only half of the old toggle that
            // is back: leaving needs no prompt, while entering does.
            if prompt == "off" {
                return self.stop_plan();
            }
            // `on` was the other half. Left to the rule above it would be sent
            // to the model as a one-word planning question, which is what
            // typing it out of habit would silently do.
            if prompt.is_empty() || prompt == "on" {
                let error = "/plan needs a prompt — /plan <what to plan>. /plan off leaves \
                             planning mode; /execute carries a plan out.";
                self.messages.push(Message::error(error));
                self.log("error", serde_json::json!({"error": error}));
                return true;
            }
            return self.start_plan(prompt);
        }
        // The answer to a waiting plan. Trailing text amends the approval rather
        // than going out as a turn of its own; `--clear` first carries the
        // plan into a chat of its own.
        if line == "/execute" || line.starts_with("/execute ") {
            let tail = line["/execute".len()..].trim();
            let (fresh, note) = match tail.strip_prefix("--clear") {
                Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
                    (true, rest.trim())
                },
                _ => (false, tail),
            };
            return self.approve_plan(note, fresh);
        }
        // A skill is a prompt, not a tool: it queues behind a running tool the
        // way typed text does, which is why this too sits above the guard.
        // After the plugin's own commands, so none of those can be shadowed;
        // before the tools, which a skill is never allowed to be named like.
        if let Some((name, tail)) = skills::split_command(line) {
            if let Some(skill) = self.skills.get(name).cloned() {
                return self.run_skill(&skill, tail);
            }
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
        self.run_manual_call(name, arguments)
    }

    fn run_manual_call(&mut self, name: String, arguments: String) -> bool {
        if self.tools_busy() {
            let notice = "a tool is already running — <Ctrl c> aborts it";
            self.messages.push(Message::notice(notice));
            self.log("notice", serde_json::json!({"content": notice}));
            return true;
        }
        // A name nothing answers to is a typo, and a typo is an error line,
        // not a call the model is then shown the failure of. Every `dispatch`
        // arm is in `tools::specs`, so this refuses nothing that would run.
        if !tools::names().contains(&name) && self.extensions.tool(&name).is_none() {
            let error = format!(
                "no such tool or skill: {name} — /help lists the commands, /skills the skills"
            );
            self.messages.push(Message::error(error.clone()));
            self.log("error", serde_json::json!({"error": error}));
            return true;
        }

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

    /// `/<skill> <tail>`: the skill's prompt goes out as a user turn, shown as
    /// the line that was typed.
    ///
    /// Nothing here is a tool: no `pending`, no `run_head`, no `manual_head`.
    /// Whatever the model does with the prompt is its own call and gets the
    /// confirmation its calls get — typing a skill is not typing `/close_pane`.
    /// Through `send_user_message`, so a skill queues behind a reply, carries
    /// an @-mention's context, answers a waiting plan and is a planning turn
    /// while planning, all exactly as typed text is.
    pub(crate) fn run_skill(&mut self, skill: &skills::Skill, tail: &str) -> bool {
        let display = if tail.is_empty() {
            format!("/{}", skill.name)
        } else {
            format!("/{} {tail}", skill.name)
        };
        let content = skills::expand(skill, tail);
        self.log(
            "skill",
            serde_json::json!({
                "name": skill.name,
                "source": skill.source.label(),
                "path": skill.path,
                "arguments": tail,
            }),
        );
        self.send_user_message(content, Some(display))
    }

    /// `/skills`: what is loaded, what was refused and why.
    pub(crate) fn show_skills(&mut self) {
        let listing = present::skills_listing(&self.skills, &self.skills_dir());
        self.messages.push(Message::notice(listing.clone()));
        self.log(
            "notice",
            serde_json::json!({"content": log::truncate(&listing)}),
        );
        self.pin_to_bottom();
    }

    /// `/skills reload`: read both folders again and list the result when the
    /// last scan lands. Before the grant there is nothing to read with.
    pub(crate) fn reload_skills(&mut self) -> bool {
        if !self.granted {
            let error = "the skills folders cannot be read before hocus is granted its \
                         permissions";
            self.messages.push(Message::error(error));
            self.log("error", serde_json::json!({"error": error}));
            return true;
        }
        self.skills_announce = true;
        self.start_user_skill_scan();
        // Forgetting the root is what makes the sync read the folder again.
        self.skills_project_root = None;
        self.sync_project_skills();
        if self.skills_scans_pending == 0 {
            // Neither tier has a folder to read: answer now.
            self.skills_announce = false;
            self.show_skills();
        }
        true
    }

    /// Record a result for the head call and drop it from the queue.
    pub(crate) fn finish_head<S: Into<String>>(&mut self, result: S) {
        if let Some(call) = self.pending.pop_front() {
            self.workflow_call = None;
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

    pub(crate) fn start_swamp_issue(&mut self, call: &ToolCall) -> bool {
        let argv = match tools::swamp_argv(&call.name, &call.arguments) {
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
            std::path::PathBuf::from("."),
            BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]),
            MAX_TOOL_OUTPUT,
        );
        self.status = Status::AwaitingCommand;
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

    /// Run one of the four VM tools by shelling out to the `zellij` CLI. The result
    /// comes back as `RunCommandResult` carrying this call's id, the way the project
    /// readers and the Slack tools answer.
    ///
    /// `vm_close` uses the CLI to confirm pane closure in the owning session before release.
    /// A failed stop may already have terminated the VM and retained its disk for recovery.
    pub(crate) fn start_vm_tool(&mut self, call: &ToolCall) -> bool {
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        // `vm_list` is machine-wide and names no session; the other three act on one
        // VM, which is held by a session. An explicit `vm_session` is how the model
        // reaches a VM `vm_list` showed under another session's name.
        let explicit_session = args
            .vm_session
            .as_deref()
            .map(str::trim)
            .filter(|session| !session.is_empty());
        let current = if call.name != "vm_list" && explicit_session.is_none() {
            match tools::current_session_info() {
                Ok(session) => Some(session),
                Err(error) => {
                    self.finish_head(error);
                    return true;
                },
            }
        } else {
            None
        };
        let session = explicit_session
            .or_else(|| current.as_ref().map(|session| session.name.as_str()))
            .unwrap_or_default();
        let Ok(zellij) = self.cfg.as_ref().map(|cfg| cfg.zellij.clone()) else {
            self.finish_head("hocus is not configured");
            return true;
        };
        let argv = match tools::vm_argv(&call.name, &args, &zellij, session) {
            Ok(argv) => argv,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let context = BTreeMap::from([("tool_call_id".to_owned(), call.id.clone())]);
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_command_with_output_limit(
            &borrowed,
            self.env
                .iter()
                .filter(|(key, _)| {
                    matches!(key.as_str(), "ZELLIJ_CONFIG_FILE" | "ZELLIJ_CONFIG_DIR")
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            // The CLI resolves nothing relative to the cwd, and `.` is the one value the
            // host does not rewrite into a preopen that does not exist - see
            // `start_project_read` for the whole story.
            std::path::PathBuf::from("."),
            context,
            MAX_TOOL_OUTPUT,
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

    /// Run the skill tools against one of the two existing skill roots. The generated host
    /// command owns path containment and returns a fresh tier scan after every mutation.
    pub(crate) fn start_skill_tool(&mut self, call: &ToolCall) -> bool {
        if call.name == "skill_list" {
            self.finish_head(present::skills_listing(&self.skills, &self.skills_dir()));
            return true;
        }
        let args = match tools::Args::parse(&call.arguments) {
            Ok(args) => args,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let name = args.name.as_deref().unwrap_or_default();
        let source = match args.scope.as_deref() {
            Some(scope) => match skills::Source::parse(scope) {
                Some(source) => source,
                None => {
                    self.finish_head("scope must be user or project");
                    return true;
                },
            },
            None if call.name == "skill_write" => skills::Source::User,
            None if call.name == "skill_read" => match self.skills.get(name) {
                Some(skill) => skill.source,
                None => {
                    self.finish_head(
                        "no enabled skill with that name; pass scope=user or scope=project to read a disabled copy",
                    );
                    return true;
                },
            },
            None => {
                self.finish_head("scope is required: user or project");
                return true;
            },
        };
        if call.name == "skill_write" && self.reserved_command_names().contains(name) {
            self.finish_head("the name is taken by a tool or command");
            return true;
        }
        let root = match source {
            skills::Source::User => {
                let root = self.skills_dir();
                if root.is_empty() {
                    self.finish_head(
                        "no user skill folder is configured; set skills_dir or user_plugin_dir",
                    );
                    return true;
                }
                root
            },
            skills::Source::Project => match self.project_target() {
                Ok(project) => format!("{}/.hocus/skills", project.root.trim_end_matches('/')),
                Err(error) => {
                    self.finish_head(error);
                    return true;
                },
            },
        };
        let (argv, env) = match skills::tool_argv(
            &call.name,
            &root,
            args.name.as_deref(),
            args.source.as_deref(),
            args.overwrite,
        ) {
            Ok(command) => command,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_command_with_output_limit(
            &borrowed,
            env,
            std::path::PathBuf::from("."),
            BTreeMap::from([
                ("tool_call_id".to_owned(), call.id.clone()),
                (skills::SOURCE_CONTEXT.to_owned(), source.label().to_owned()),
                (skills::ROOT_CONTEXT.to_owned(), root),
            ]),
            if call.name == "skill_read" {
                skills::MAX_SKILL_BYTES + 1
            } else {
                skills::MAX_SCAN_OUTPUT
            },
        );
        self.status = Status::AwaitingCommand;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// The plugin `only`, or every plugin in the folder with the backups left out (as a listing
    /// leaves them out), as `(name.ts, contents)`. Reads answer inline on the plugin thread, so
    /// a folder is read up to `FOLDER_FILES_CAP` files and each up to `FOLDER_READ_BYTES`; one
    /// of many that cannot be read is skipped, since a search is about the ones that can.
    fn plugin_corpus(&self, only: Option<&str>) -> Result<(Vec<(String, String)>, usize), String> {
        let names = match only {
            Some(name) => {
                tools::resolve_plugin_file(&self.user_plugin_dir, Some(name))?;
                vec![plugin_source::file_name(name)]
            },
            None => tools::listing_names(
                tools::PLUGINS_ROOT,
                &tools::plugin_fs_list(tools::PLUGINS_ROOT)?,
            ),
        };
        let mut skipped = names.len().saturating_sub(tools::FOLDER_FILES_CAP);
        let mut files = Vec::new();
        for name in names.into_iter().take(tools::FOLDER_FILES_CAP) {
            match tools::plugin_fs_path(Some(&name))
                .and_then(|path| tools::plugin_fs_read_capped(&path, tools::FOLDER_READ_BYTES))
            {
                Ok(contents) => files.push((name, contents)),
                Err(error) if only.is_some() => return Err(error),
                Err(_) => skipped += 1,
            }
        }
        Ok((files, skipped))
    }

    /// Run one of the plugin-authoring tools.
    ///
    /// Every one answers here and now: `plugin_docs` from compiled-in text, `plugin_check` from
    /// the host's compiler, `plugin_log` from the host's read of its own log, and the four that
    /// touch the plugin folder through the host's `fs_*` commands against the `/plugins` root,
    /// which answer synchronously.
    /// A cleared `plugin_reload` is the one that defers rather than answering: the load is
    /// fire-and-forget, so the head stays pending until the pane has had time to show what
    /// happened, and this returns `false` so `pump_tools` stops there instead of running the
    /// same head again.
    ///
    /// Everything refusable without touching anything is refused first: no plugin folder, a bad
    /// name, an empty source.
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
            // Three grains, cheapest first: a query answers with hits, a range with one page of
            // a section, and a bare `section` with the whole of it.
            let answer = if args.query.is_some() {
                match tools::search_options(&args) {
                    Ok(options) => plugin_docs::search(args.section.as_deref(), &options),
                    Err(error) => error,
                }
            } else if args.start_line.is_some() || args.line_count.is_some() {
                let (start, end) = tools::read_range(&args);
                plugin_docs::page(args.section.as_deref(), start, end)
            } else {
                plugin_docs::docs(args.section.as_deref())
            };
            // The only tool result that used to go out uncapped. Sub-sectioning keeps every
            // answer far under this, so it is a floor under the floor: a guide section that
            // grows past what a tool call should cost cannot silently re-create the problem.
            self.finish_head(log::truncate_str(&answer, plugin_docs::MAX_DOCS_OUTPUT));
            return true;
        }
        // Explicit drafts need no folder; omitted source reads the saved file first.
        if call.name == "plugin_check" {
            let result = match (args.name.as_deref(), args.source.as_deref()) {
                (None, _) => "name is required".to_owned(),
                (Some(_), None) if self.user_plugin_dir.trim().is_empty() => {
                    tools::NO_PLUGIN_FOLDER.to_owned()
                },
                (Some(name), None) => tools::plugin_fs_path(Some(name))
                    .and_then(|path| tools::plugin_fs_read(&path))
                    .map(|source| tools::semantic_report(name, &source))
                    .unwrap_or_else(|error| error),
                // The lint rides along with the compile result, which is the whole point of
                // checking before writing: a source that compiles can still be spelling a
                // namespace or a permission the store will refuse at run time.
                (Some(name), Some(source)) => tools::semantic_report(name, source),
            };
            self.finish_head(result);
            return true;
        }

        if call.name == "plugin_test" {
            let Some(name) = args.name.as_deref() else {
                self.finish_head("name is required");
                return true;
            };
            let options = serde_json::from_value::<zellij_tile::plugin_authoring::PluginTestOptions>(
                serde_json::json!({
                    "kind": args.kind.as_deref().unwrap_or("all"), "filter": args.filter, "timeout_ms": args.timeout_ms.unwrap_or(30_000)
                }),
            );
            return match options {
                Ok(options) => {
                    zellij_tile::shim::test_plugin(name, options, &call.id);
                    self.status = Status::AwaitingPluginTest;
                    false
                },
                Err(error) => {
                    self.finish_head(error.to_string());
                    true
                },
            };
        }
        if call.name == "plugin_format" && args.source.is_some() {
            let answer = args
                .name
                .as_deref()
                .ok_or_else(|| "name is required".to_owned())
                .and_then(|name| {
                    tools::formatted_source(
                        name,
                        args.source.as_deref().unwrap_or_default(),
                        zellij_tile::plugin_authoring::PluginCheckMode::Format,
                    )
                });
            self.finish_head(answer.unwrap_or_else(|error| error));
            return true;
        }

        let dir = self.user_plugin_dir.clone();
        if dir.trim().is_empty() {
            self.finish_head(tools::NO_PLUGIN_FOLDER);
            return true;
        }
        // `plugin_search` may name a plugin or take the folder whole, so it goes before a name
        // is resolved.
        if call.name == "plugin_search" {
            let result = match tools::search_options(&args) {
                Err(error) => error,
                Ok(options) => match self.plugin_corpus(args.name.as_deref()) {
                    Err(error) => error,
                    Ok((files, skipped)) => {
                        let refs: Vec<(&str, &str)> = files
                            .iter()
                            .map(|(name, contents)| (name.as_str(), contents.as_str()))
                            .collect();
                        match plugin_source::search(&refs, &options) {
                            Ok(hits) => {
                                let answer = hits.unwrap_or_else(|| {
                                    tools::no_matches(args.name.as_deref(), files.len())
                                });
                                let answer = if skipped > 0 {
                                    format!("Warning: incomplete search; {skipped} files skipped (file cap or failed reads).\n\n{answer}")
                                } else {
                                    answer
                                };
                                log::truncate_str(&answer, MAX_TOOL_OUTPUT)
                            },
                            Err(error) => error,
                        }
                    },
                },
            };
            self.finish_head(result);
            return true;
        }
        // `plugin_list` names no plugin - it is the folder itself that is the target. Each
        // plugin is opened for its length: what the model needs to decide how to read it.
        if call.name == "plugin_list" {
            let result = match tools::plugin_fs_list(tools::PLUGINS_ROOT) {
                Ok(entries) => {
                    let names = tools::listing_names(tools::PLUGINS_ROOT, &entries);
                    let backed_up = tools::backed_up_names(tools::PLUGINS_ROOT, &entries);
                    let listed: Vec<plugin_source::Listed> = names
                        .iter()
                        .take(tools::FOLDER_FILES_CAP)
                        .map(|name| plugin_source::Listed {
                            name,
                            lines: tools::plugin_fs_path(Some(name))
                                .and_then(|path| {
                                    tools::plugin_fs_read_capped(&path, tools::FOLDER_READ_BYTES)
                                })
                                .map(|source| source.lines().count()),
                            backup: backed_up.contains(name),
                        })
                        .collect();
                    let mut answer = plugin_source::listing(&listed);
                    if names.len() > listed.len() {
                        answer.push_str(&format!(
                            "\n… {} more plugins not read",
                            names.len() - listed.len()
                        ));
                    }
                    log::truncate_str(&answer, MAX_TOOL_OUTPUT)
                },
                Err(error) => error,
            };
            self.finish_head(result);
            return true;
        }

        // Two spellings of one file: the real path, for the reload URL and the permission
        // grant, and the `/plugins` path the host's file commands take. Both go through the
        // same rules, so they cannot name different files.
        let (target, fs_path) = match (
            tools::resolve_plugin_file(&dir, args.name.as_deref()),
            tools::plugin_fs_path(args.name.as_deref()),
        ) {
            (Ok(target), Ok(fs_path)) => (target, fs_path),
            (Err(error), _) | (_, Err(error)) => {
                self.finish_head(error);
                return true;
            },
        };
        let name = args.name.clone().unwrap_or_default();

        match call.name.as_str() {
            "plugin_read" => {
                let (start, end) = tools::read_range(&args);
                let fetch = format!("plugin_read name={}", name.trim());
                let result = match tools::plugin_fs_read(&fs_path) {
                    Ok(contents) => log::truncate_str(
                        &tools::read_answer(&contents, start, end, &fetch),
                        MAX_TOOL_OUTPUT,
                    ),
                    Err(error) => error,
                };
                self.finish_head(result);
                true
            },
            "plugin_outline" => {
                let result = match tools::plugin_fs_read(&fs_path) {
                    Ok(contents) => log::truncate_str(
                        &plugin_source::outline_answer(&name, &contents),
                        MAX_TOOL_OUTPUT,
                    ),
                    Err(error) => error,
                };
                self.finish_head(result);
                true
            },
            "plugin_write" => {
                let source = match tools::plugin_source(&args) {
                    Ok(source) => source,
                    Err(error) => {
                        self.finish_head(error);
                        return true;
                    },
                };
                let source = match tools::formatted_source(
                    &name,
                    &source,
                    zellij_tile::plugin_authoring::PluginCheckMode::Prepare,
                ) {
                    Ok(source) => source,
                    Err(error) => {
                        self.finish_head(error);
                        return true;
                    },
                };
                // Preserve the old version only after checks pass, before replacing it.
                let replaced = match tools::plugin_fs_exists(&fs_path) {
                    Ok(true) if !args.overwrite => {
                        self.finish_head(tools::exists_report(&name));
                        return true;
                    },
                    Ok(true) => match tools::plugin_fs_read(&fs_path) {
                        Ok(previous) => Some(previous),
                        Err(error) => {
                            self.finish_head(error);
                            return true;
                        },
                    },
                    Ok(false) => None,
                    Err(error) => {
                        self.finish_head(error);
                        return true;
                    },
                };
                let granted = match tools::source_permissions(&name, &source) {
                    Ok(granted) => granted,
                    Err(error) => {
                        self.finish_head(error);
                        return true;
                    },
                };
                let backup = tools::keep_replaced(&name, replaced.as_deref(), &source);
                if backup == tools::Kept::Failed {
                    self.finish_head("backup failed; plugin source and permissions are unchanged");
                    return true;
                }
                let result = match tools::plugin_fs_write(&fs_path, &source, args.overwrite) {
                    Ok(true) => {
                        // The user approved this source, and with it the permissions it asks
                        // for - the confirmation said so. Granting them now is what keeps the
                        // plugin from prompting in a pane nobody in this loop is looking at.
                        if !granted.is_empty() {
                            tools::grant_permissions(&target, &granted);
                        }
                        tools::with_lint(
                            tools::write_report(&name, &granted, &backup),
                            &name,
                            &source,
                        )
                    },
                    Ok(false) => tools::exists_report(&name),
                    Err(error) => error,
                };
                self.finish_head(result);
                true
            },
            "plugin_edit" => {
                let previous = match tools::plugin_fs_read(&fs_path) {
                    Ok(previous) => previous,
                    Err(error) => {
                        self.finish_head(error);
                        return true;
                    },
                };
                let (mut edited, granted) =
                    match tools::edit_source_and_permissions(&previous, &args) {
                        Ok(edit) => edit,
                        Err(error) => {
                            self.finish_head(error);
                            return true;
                        },
                    };
                let prepared = match tools::formatted_source(
                    &name,
                    &edited.source,
                    zellij_tile::plugin_authoring::PluginCheckMode::Prepare,
                ) {
                    Ok(source) => source,
                    Err(error) => {
                        self.finish_head(tools::edit_compile_report(&name, &error));
                        return true;
                    },
                };
                if edited.source != prepared {
                    edited.changed = vec![crate::plugin_source::LineSpan {
                        first: 1,
                        last: prepared.lines().count().max(1),
                    }];
                    edited.source = prepared;
                }
                let backup = tools::keep_replaced(&name, Some(&previous), &edited.source);
                if backup == tools::Kept::Failed {
                    self.finish_head("backup failed; plugin source and permissions are unchanged");
                    return true;
                }
                let result = match tools::plugin_fs_write(&fs_path, &edited.source, true) {
                    Ok(_) => {
                        if !granted.is_empty() {
                            tools::grant_permissions(&target, &granted);
                        }
                        tools::with_lint(
                            tools::edit_report(&name, &granted, &backup, &edited),
                            &name,
                            &edited.source,
                        )
                    },
                    Err(error) => error,
                };
                self.finish_head(result);
                true
            },
            "plugin_format" => {
                let result = (|| -> Result<String, String> {
                    let previous = tools::plugin_fs_read(&fs_path)?;
                    let source = tools::formatted_source(
                        &name,
                        &previous,
                        zellij_tile::plugin_authoring::PluginCheckMode::Format,
                    )?;
                    let backup = tools::keep_replaced(&name, Some(&previous), &source);
                    if backup == tools::Kept::Failed {
                        return Err("backup failed; plugin source is unchanged".into());
                    }
                    tools::plugin_fs_write(&fs_path, &source, true)?;
                    Ok(tools::write_report(&name, &[], &backup))
                })();
                self.finish_head(result.unwrap_or_else(|error| error));
                true
            },
            "plugin_reload" => match tools::plugin_fs_exists(&fs_path) {
                Ok(true) => self.start_plugin_load(),
                Ok(false) => {
                    self.finish_head(format!("{target}: no such plugin; write it first"));
                    true
                },
                Err(error) => {
                    self.finish_head(error);
                    true
                },
            },
            "plugin_log" => {
                let lines = args.line_count.unwrap_or(tools::PLUGIN_LOG_LINES).max(1);
                let answer = tools::plugin_log_answer(&name, tools::plugin_log_tail(&name, lines));
                self.finish_head(log::truncate_str(&answer, MAX_TOOL_OUTPUT));
                true
            },
            other => {
                self.finish_head(format!("no such tool: {other}"));
                true
            },
        }
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
            tools::LOAD_TOOL
                | "tool_result_read"
                | "schedule"
                | "unschedule"
                | "project_list"
                | "project_assign"
                | "project_open"
                | "project_forget"
                | "project_rename"
                | "project_merge"
                | "todo_list"
                | "todo_search"
                | "note_list"
                | "note_get"
                | "note_search"
                | "agent_register"
                | "agent_list"
                | "agent_runs"
                | "agent_run"
                | "agent_history_search"
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
            // Nothing to run: the load is the call itself being in the transcript,
            // which `tools::loaded_families` reads on the way out. This only says
            // what the next request will advertise.
            "tool_result_read" => {
                api::tool_result_read(&self.messages, &args).unwrap_or_else(|error| error)
            },
            tools::LOAD_TOOL => tools::load_family(args.family.as_deref(), self.is_planning()),
            "schedule" => self.schedule(&args),
            "unschedule" => self.unschedule(args.job_id),
            "project_list" => self.project_list(),
            "project_assign" => self.project_assign(&args),
            "project_open" => self.project_open(&args),
            "project_forget" => self.project_forget(&args),
            "project_rename" | "project_merge" => self
                .project_change(&args, call.name == "project_merge")
                .unwrap_or_else(|error| error),
            "todo_list" => self.todo_list(&args),
            "todo_search" => self.todo_search(&args),
            "note_list" => self.note_list(&args),
            "note_get" => self.note_get(&args),
            "note_search" => self.note_search(&args),
            "agent_register" => self.agent_register(&args),
            "agent_list" => self.agent_list(&args),
            "agent_runs" => self.agent_runs(&args),
            "agent_run" => self.agent_run(&args),
            "agent_history_search" => self.agent_history_search(&args),
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
        // Three readings of `firecracker_image`, kept apart on purpose: a key the
        // model omitted keeps whatever is stored, an empty one clears it, and
        // anything else names the image to use from now on.
        let image = match args.firecracker_image.as_deref().map(str::trim) {
            None => None,
            Some("") => Some(None),
            Some(image) => {
                validate_firecracker_vm_name(image)?;
                Some(Some(image.to_owned()))
            },
        };
        // Read before `adopt` replaces the entry: a project remembered again with
        // its root and nothing else keeps the image it already had.
        if args
            .worktree_dir
            .as_deref()
            .is_some_and(|dir| dir.contains('\0'))
        {
            return Err("worktree_dir must not contain NUL bytes".to_owned());
        }
        let rows = self
            .store()
            .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?;
        let before = projects::from_rows(&rows);
        let stored = before.get(name).cloned();
        if let Some(root) = args.root.as_deref() {
            let mut project = projects::validate(name, root)?;
            project.worktree_dir = stored
                .as_ref()
                .and_then(|project| project.worktree_dir.clone());
            let image_explicit = image.is_some();
            project.firecracker_image = match image {
                Some(image) => image,
                None => stored.and_then(|project| project.firecracker_image),
            };
            let (mut adopted, mut mutations) =
                projects::plan_adopt(&rows, project, image_explicit)?;
            if let Some(dir) = &args.worktree_dir {
                adopted.project.worktree_dir = (!dir.is_empty()).then(|| dir.clone());
                // Apply the explicit override after the adoption's inherited defaults.
                mutations.push(projects::to_mutation(&adopted.project));
            }
            let report = self.write_store(mutations).map_err(|error| {
                if error.is_conflict() {
                    "projects changed; call project_list and retry".to_owned()
                } else {
                    error.into()
                }
            })?;
            let project = adopted.project;
            let mut registry = before;
            let mut ids = projects::row_ids(&rows);
            if let Some(gone) = adopted.replaced {
                registry.remove(&gone);
                ids.remove(&gone);
            }
            if let Some(id) = report.ids.last() {
                ids.insert(project.name.clone(), *id);
            }
            registry.insert(project.name.clone(), project.clone());
            self.take_projects(registry, ids);
            Ok(project)
        } else {
            let mut project = stored.ok_or_else(|| {
                format!("unknown project {name}; root is required for a new project")
            })?;
            if image.is_none() && args.worktree_dir.is_none() {
                return Ok(project);
            }
            // The image alone, on a project already known: an update in place,
            // written the same way a new project is.
            if let Some(image) = image {
                project.firecracker_image = image;
            }
            if let Some(dir) = &args.worktree_dir {
                project.worktree_dir = (!dir.is_empty()).then(|| dir.clone());
            }
            let mutations = vec![
                projects::assert_projects(&rows, "name", &[&project.name])?,
                projects::to_mutation(&project),
            ];
            self.write_store(mutations)
                .map_err(|error| error.to_string())?;
            let mut registry = before;
            registry.insert(project.name.clone(), project.clone());
            self.take_projects(registry, projects::row_ids(&rows));
            Ok(project)
        }
    }

    fn project_change(&mut self, args: &tools::Args, merge: bool) -> Result<String, String> {
        let (source, target) = if merge {
            (args.source.as_deref(), args.target.as_deref())
        } else {
            (args.name.as_deref(), args.new_name.as_deref())
        };
        let (Some(source), Some(target)) = (source, target) else {
            return Err(if merge {
                "source and target are required"
            } else {
                "name and new_name are required"
            }
            .into());
        };
        let rows = self
            .store()
            .fetch(&FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT))?;
        let change = if merge {
            projects::plan_merge(&rows, source, target)?
        } else {
            projects::plan_rename(&rows, source, target)?
        };
        let message = if merge {
            format!(
                "merged {} into {}; kept the target's root and settings",
                change.replaced, change.project.name
            )
        } else {
            format!("renamed {} to {}", change.replaced, change.project.name)
        };
        self.apply_project_change(&rows, change)?;
        Ok(message)
    }

    pub(crate) fn apply_project_change(
        &mut self,
        rows: &DataRows,
        change: projects::Change,
    ) -> Result<(), String> {
        let report = self.write_store(change.mutations).map_err(|error| {
            if error.is_conflict() {
                "projects changed; call project_list and retry".to_owned()
            } else {
                error.into()
            }
        })?;
        let mut registry = projects::from_rows(rows);
        registry.remove(&change.replaced);
        registry.insert(change.project.name.clone(), change.project.clone());
        let mut ids = projects::row_ids(rows);
        ids.remove(&change.replaced);
        if let Some(id) = report.ids.first() {
            ids.insert(change.project.name.clone(), *id);
        }
        self.take_projects(registry, ids);
        Ok(())
    }

    pub(crate) fn project_list(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .projects
            .values()
            .map(|project| {
                let ids = |assigned: &BTreeMap<TabId, String>| {
                    let mut ids: Vec<TabId> = assigned
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
                    "firecracker_image": project.firecracker_image,
                    "worktree_dir": project.worktree_dir,
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
        // The focused agent's tab before the active one. hocus's overlay does sit in a
        // tab now, but that tab is the active one by construction — it follows every
        // switch — so "the tab I am in" still answers the wrong question. The agent the
        // user was last looking at is the tab they mean.
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
            self.sync_project_skills();
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
            .and_then(|value| value["tab_id"].as_u64())
            .and_then(|id| TabId::try_from(id).ok());
        let Some(tab_id) = tab_id else {
            return result.to_owned();
        };
        self.tab_projects.insert(tab_id, name.to_owned());
        self.auto_tab_projects.remove(&tab_id);
        self.active_project = Some(name.to_owned());
        self.sync_project_skills();
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
    pub(crate) fn todo_write(
        &self,
        backend: impl Backend,
        call: &ToolCall,
    ) -> Result<String, String> {
        let args = tools::Args::parse(&call.arguments)?;
        let (root, name) = self.todo_target(&args)?;
        let list = todos::fetch_list(&backend, &root)?;

        match call.name.as_str() {
            "todo_add" => {
                let text = args.text.as_deref().ok_or("text is required")?;
                let (mutations, text) = todos::plan_add(&list, &root, text, args.parent_id)?;
                let report = backend.apply(mutations).map_err(todos::topology_error)?;
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
                let (mutations, text) = todos::plan_edit(&list, id, text, args.expect_rev)?;
                if let Err(error) = backend.apply(mutations) {
                    if !error.is_conflict() {
                        return Err(error.into());
                    }
                    // Deliberately not retried. The guard refused because somebody edited this
                    // item between the read and the write, and the text on offer was composed
                    // from the older wording — writing it anyway is the loss the guard exists
                    // to prevent. So the model is told what the item says now and decides.
                    //
                    // The revision goes out with the wording, so a retry composed from what it
                    // says here can name what it was composed from.
                    let now = todos::fetch_list(&backend, &root)
                        .ok()
                        .and_then(|list| todos::item_of(&list, id).cloned());
                    return Err(match now {
                        Some(item) => format!(
                            "todo item {id} was edited by somebody else and now reads {:?} at \
                             rev {}; call todo_edit again with that expect_rev if the change is \
                             still wanted",
                            item.text, item.rev
                        ),
                        None => format!("todo item {id} was removed by somebody else"),
                    });
                }
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
                backend.apply(mutations).map_err(todos::topology_error)?;
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
        let answer = match self.todo_write(self.store(), call) {
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
        // Each item that has notes says which, so the model can read one
        // before touching the item it is about.
        let ids = notes::todo_ids_in(&value);
        if !ids.is_empty() {
            match notes::links_for_todos(&self.store(), &ids) {
                Ok(links) => notes::annotate_todo_list(&mut value, &links),
                Err(error) => return error,
            }
        }
        value.to_string()
    }

    // ------------------------------------------------------------------ notes

    /// Who a write is by: the user for a command they typed, hocus for a call
    /// the model made. `manual_head` is set for exactly the former and taken
    /// back when the call finishes, which is after this runs.
    pub(crate) fn call_author(&self) -> &'static str {
        if self.manual_head {
            "user"
        } else {
            "hocus"
        }
    }

    /// The scope a note tool means. `all_projects` beats `global` beats
    /// `project`, and a bare call means the active project, as the todo tools
    /// read it.
    pub(crate) fn note_where(
        &self,
        args: &tools::Args,
        reading: bool,
    ) -> Result<notes::Where, String> {
        if reading && args.all_projects {
            return Ok(notes::Where::All);
        }
        if args.global {
            return Ok(notes::Where::Global);
        }
        let (root, name) = self
            .todo_target(args)
            .map_err(|error| format!("{error}, or set global for a note tied to no project"))?;
        Ok(notes::Where::Project { root, name })
    }

    /// A note as the model reads it, with its project named.
    pub(crate) fn note_reply(&self, note: &notes::Note) -> serde_json::Value {
        let mut value = notes::note_json(note, crate::transitions::now_secs() * 1000);
        value["project"] =
            serde_json::json!(notes::project_name(&self.projects, note.root.as_deref()));
        value
    }

    /// A note as a write answers it: everything but the body, which the model has just
    /// written and so already holds. Echoing it made the reply as long as the note, and a
    /// reply over the per-result cap on the wire is cut at the head - which, keys going out
    /// in alphabetical order in the wasm build, took the `id` after `body` with it.
    /// `body_bytes` says what was stored.
    pub(crate) fn note_written_reply(&self, note: &notes::Note) -> serde_json::Value {
        let mut value = self.note_reply(note);
        if let Some(object) = value.as_object_mut() {
            object.remove("body");
            object.insert("body_bytes".to_owned(), serde_json::json!(note.body.len()));
        }
        value
    }

    /// Change the notes, and answer with what changed.
    ///
    /// Synchronous like [`Self::todo_write`]. The one two-step is `note_add`:
    /// a note's tags and links carry its id, which the store allocates on the
    /// first apply, so they go in a second — and if that one fails the answer
    /// says the note is there without them rather than pretending either way.
    pub(crate) fn note_write(
        &self,
        backend: impl Backend,
        call: &ToolCall,
    ) -> Result<String, String> {
        let args = tools::Args::parse(&call.arguments)?;
        match call.name.as_str() {
            "note_add" => {
                let title = args.title.as_deref().ok_or("title is required")?;
                let body = args.body.as_deref().ok_or("body is required")?;
                let scope = self.note_where(&args, false)?;
                notes::require_todos(&backend, &args.todo_ids)?;
                let count = notes::count_in_scope(&backend, scope.scope())?;
                let tags = args.tags.clone().unwrap_or_default();
                let (mutations, draft) = notes::plan_add(
                    count,
                    scope.root(),
                    title,
                    body,
                    self.call_author(),
                    &tags,
                    &args.todo_ids,
                )?;
                let report = backend.apply(mutations)?;
                let id = u64::try_from(report.ids.first().copied().unwrap_or_default().get())
                    .unwrap_or_default();
                let warning = notes::plan_attach(&backend, id, &draft.tags, &draft.todo_ids)
                    .and_then(|attach| {
                        if !attach.is_empty() {
                            backend.apply(attach).map_err(todos::topology_error)?;
                        }
                        Ok(())
                    })
                    .err();
                let mut value = self.note_written_reply(&existing_note(&backend, id)?);
                if let Some(error) = warning {
                    value["warning"] = serde_json::json!(format!(
                        "the note was written but its tags and links were not: {error}"
                    ));
                }
                Ok(value.to_string())
            },
            "note_edit" => {
                let id = args.id.ok_or("id is required")?;
                let note = existing_note(&backend, id)?;
                let mut mutations = Vec::new();
                let edits_text = args.title.is_some() || args.body.is_some();
                if edits_text {
                    mutations.extend(
                        notes::plan_edit(
                            &note,
                            args.title.as_deref(),
                            args.body.as_deref(),
                            args.expect_rev,
                        )?
                        .0,
                    );
                }
                if let Some(tags) = &args.tags {
                    // Guarded only when the text edit above has not already guarded this note:
                    // two guards on one row in one batch is the second finding the revision the
                    // first bumped. See `notes::plan_set_tags`.
                    mutations
                        .extend(notes::plan_set_tags(&note, tags, args.expect_rev, !edits_text)?.0);
                }
                if mutations.is_empty() {
                    return Err("give a title, a body, tags, or some of each".to_owned());
                }
                if let Err(error) = backend.apply(mutations) {
                    if !error.is_conflict() {
                        return Err(error.into());
                    }
                    // Not retried, for the reason `todo_edit` is not: the title and body on
                    // offer were composed from the version that has since been replaced. The
                    // revision goes out with the title so a retry can name what it read.
                    return Err(match existing_note(&backend, id) {
                        Ok(now) => format!(
                            "note {id} was edited by somebody else and now reads {:?} at rev \
                             {}; call note_get and then note_edit again with that expect_rev if \
                             the change is still wanted",
                            now.title, now.rev
                        ),
                        Err(_) => format!("note {id} was removed by somebody else"),
                    });
                }
                Ok(self
                    .note_written_reply(&existing_note(&backend, id)?)
                    .to_string())
            },
            "note_archive" => {
                let id = args.id.ok_or("id is required")?;
                let note = existing_note(&backend, id)?;
                let archived = !args.undo;
                backend.apply(notes::plan_archive(id, archived))?;
                Ok(serde_json::json!({
                    "id": id,
                    "title": note.title,
                    "project": notes::project_name(&self.projects, note.root.as_deref()),
                    "archived": archived,
                })
                .to_string())
            },
            "note_delete" => {
                let id = args.id.ok_or("id is required")?;
                let note = existing_note(&backend, id)?;
                backend.apply(notes::plan_delete(id))?;
                Ok(serde_json::json!({
                    "id": id,
                    "title": note.title,
                    "project": notes::project_name(&self.projects, note.root.as_deref()),
                    "deleted": true,
                })
                .to_string())
            },
            _ => {
                let note_id = args.note_id.ok_or("note_id is required")?;
                let todo_id = args.todo_id.ok_or("todo_id is required")?;
                existing_note(&backend, note_id)?;
                if args.unlink {
                    backend.apply(notes::plan_unlink(note_id, todo_id))?;
                } else {
                    backend
                        .apply(notes::plan_link(&backend, note_id, todo_id)?)
                        .map_err(todos::topology_error)?;
                }
                Ok(serde_json::json!({
                    "note_id": note_id,
                    "todo_id": todo_id,
                    "linked": !args.unlink,
                })
                .to_string())
            },
        }
    }

    /// Run a note change and finish the call with its answer. Always `true`,
    /// as [`Self::start_todo_write`] is: nothing waits.
    pub(crate) fn start_note_write(&mut self, call: &ToolCall) -> bool {
        let answer = match self.note_write(self.store(), call) {
            Ok(answer) => answer,
            Err(error) => error,
        };
        self.finish_head(answer);
        true
    }

    /// The notes of a scope, without bodies.
    pub(crate) fn note_list(&self, args: &tools::Args) -> String {
        let scope = match self.note_where(args, true) {
            Ok(scope) => scope,
            Err(error) => return error,
        };
        let filter = notes::Filter {
            scope: Some(scope.scope()),
            include_archived: args.include_archived,
            tag: args.tag.as_deref(),
            todo_id: args.todo_id,
        };
        let found = match notes::list_notes(&self.store(), filter) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let listed: Vec<serde_json::Value> =
            found.iter().map(|note| self.note_reply(note)).collect();
        serde_json::json!({
            "scope": scope.label(),
            "count": listed.len(),
            "notes": listed,
        })
        .to_string()
    }

    /// One note, whole.
    pub(crate) fn note_get(&self, args: &tools::Args) -> String {
        let Some(id) = args.id else {
            return "id is required".to_owned();
        };
        match existing_note(&self.store(), id) {
            Ok(note) => self.note_reply(&note).to_string(),
            Err(error) => error,
        }
    }

    pub(crate) fn note_search(&self, args: &tools::Args) -> String {
        let Some(query) = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
        else {
            return "query is required".to_owned();
        };
        let scope = match self.note_where(args, true) {
            Ok(scope) => scope,
            Err(error) => return error,
        };
        let found = match notes::search(&self.store(), scope.scope(), query) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let matches: Vec<serde_json::Value> = found
            .iter()
            .map(|note| {
                let mut value = self.note_reply(note);
                value["snippet"] = serde_json::json!(notes::snippet(&note.body));
                if let Some(object) = value.as_object_mut() {
                    object.remove("body");
                }
                value
            })
            .collect();
        serde_json::json!({
            "query": query,
            "scope": scope.label(),
            "matches": matches,
            "truncated": found.len() >= notes::MAX_MATCHES,
        })
        .to_string()
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
    pub(crate) fn tab_project(&self, tab_id: TabId) -> Option<&str> {
        self.tab_projects
            .get(&tab_id)
            .or_else(|| self.auto_tab_projects.get(&tab_id))
            .map(String::as_str)
    }

    /// Take note of which pane the user is looking at.
    ///
    /// Only an agent pane is remembered. Anything else — hocus itself, a shell, an
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
    fn focused_tab(&self) -> Option<TabId> {
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

    /// The folder that identifies a pane's work, with the machine it is on.
    ///
    /// Every half comes off the pane: the daemon puts the folder there, and the
    /// manifest puts the host and the worktree a Firecracker VM mirrors there.
    /// Keeping them together is what stops two panes sitting in the same path on
    /// different machines from being read as one folder — see
    /// [`agents::checkout_location`], which is where the rule lives.
    fn pane_location(pane: &agents::AgentPane) -> Option<Location> {
        pane.checkout_location()
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

    /// Pane id to the name of the project it is *in*, for every terminal pane.
    ///
    /// The naming counterpart of [`pane_projects`](Self::pane_projects), and
    /// wider on both axes: every pane rather than only agents, and a checkout
    /// nobody registered rather than only a registered project. Both widenings
    /// are safe here for the same reason — a name is worked out afresh from the
    /// panes that are open, so a wrong one costs a redraw, where a wrongly
    /// remembered project costs the user a row they have to delete.
    pub(crate) fn pane_project_labels(&self) -> BTreeMap<u32, String> {
        self.panes
            .iter()
            .chain(&self.suppressed_panes)
            .filter_map(|pane| {
                let location = Self::pane_location(pane)?;
                let label = projects::label_for_cwd(&self.repositories, &self.projects, &location)?;
                Some((pane.pane_id, label))
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

    /// The project that owns a pane's folder. See [`projects::project_for_cwd`].
    fn project_for_cwd(&self, cwd: &Location) -> Option<&projects::Project> {
        projects::project_for_cwd(&self.repositories, &self.projects, cwd)
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
        // this it comes straight back. Only when we have heard of any tab at all: an
        // empty list is "nobody has told us" — hocus's overlay may be sitting in a tab
        // nobody is looking at, and it is the host's routing of `TabUpdate` to overlays
        // in every tab that gets it the news — and is not the same as "no tabs".
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
        self.sync_project_skills();
        self.refresh_tab_project_labels();
        self.send_tab_assignments();
    }

    /// Relay changed manual assignments as project metadata to the daemon's subscribers.
    ///
    /// Here rather than at each of the four places that write `tab_projects`,
    /// because this runs on every poll and so cannot be forgotten by a fifth.
    /// The comparison is what keeps that from becoming a message a second.
    fn send_tab_assignments(&mut self) {
        if self.sent_tab_projects == self.tab_projects {
            return;
        }
        self.sent_tab_projects = self.tab_projects.clone();
        self.send_overrides();
    }

    /// Work out what each tab should be called.
    ///
    /// Not published from here. hocus-store is the one publisher, because a tab
    /// has to be named in a session where no hocus pane is open — which is most
    /// of them. This copy exists because half the plugin reads it (the tab map
    /// the model is shown, and the tests), and it is derived by
    /// [`projects::label_for_cwd`] and [`agents::tab_project_labels`], the same
    /// two the store uses, so the two cannot answer differently.
    fn refresh_tab_project_labels(&mut self) {
        let owners = self.pane_project_labels();
        let detected =
            agents::tab_project_labels(self.panes.iter().chain(&self.suppressed_panes), &owners);
        let mut labels: BTreeMap<TabId, String> = detected
            .into_iter()
            .filter_map(|(tab_id, projects)| Some((tab_id, agents::tab_project_label(&projects)?)))
            .collect();
        if !self.tabs.is_empty() {
            labels.retain(|tab_id, _| self.tabs.iter().any(|tab| tab.tab_id == *tab_id));
        }
        self.tab_project_labels = labels;
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
        let mut gone = Vec::new();
        self.manual_kinds.retain(|id, _| {
            let keep = self.pending_agent_panes.contains(id)
                || self.panes.iter().any(|pane| pane.pane_id == *id);
            if !keep {
                gone.push(*id);
            }
            keep
        });
        self.forget_latches(gone);
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
        self.persist_latch(pane_id, kind);
        // The session's record too, so `list-panes`, `zellij events` and every other plugin
        // see the registration, and so it outlives a hocus reload on its own.
        let cwd = self
            .panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .and_then(|pane| pane.cwd.clone());
        tools::report_pane_agent(pane_id, agents::registered_agent(kind, cwd));
        // hocus stays authoritative for the overrides; the daemon holds a copy
        // only to classify with, so it needs the new one before its next poll.
        self.send_overrides();
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
            &self.hooks,
            read_pane_text,
        )
    }

    // -- reading what agents have already done -------------------------
    //
    // Three tools over `run_history`, and one filter between them. They read the
    // shared store directly and synchronously, as the note and todo tools do, so
    // none of them defers: there is no daemon in the path and nothing to wait for.

    /// The filter these three share, and a phrase naming what it selected.
    ///
    /// The label is not decoration. Everything the store holds is the default, so an
    /// answer with four runs in it has to say whether that is four runs across the
    /// archive or four in this session — otherwise an empty result reads as "this
    /// never happened" when it means "not here".
    fn run_filter(&self, args: &tools::Args) -> Result<(run_history::Filter, String), String> {
        let mut scope = Vec::new();
        let (zellij_session_name, host_id) = match args.this_session {
            false => (None, None),
            true => {
                // Empty until the daemon's first poll reply names it. Matching on
                // `""` would silently select every run that never had a session
                // name written, which is the opposite of what was asked for.
                let name = self.zellij_session_name.as_str().trim();
                if name.is_empty() {
                    return Err(
                        "this session is not named yet; retry in a moment, or drop this_session \
                         to search every session in the archive"
                            .to_owned(),
                    );
                }
                scope.push(format!("session {name}"));
                // The machine as well as the name: with a store shared between machines a
                // session called `main` here and a `main` elsewhere are two different sessions,
                // and "this session" means this one.
                (Some(name.to_owned()), Some(self.host_id.clone()))
            },
        };
        if !args.this_session {
            scope.push("all sessions".to_owned());
        }
        let root = match args
            .project
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            None => None,
            Some(name) => {
                let Some(project) = self.projects.get(name) else {
                    return Err(format!("unknown project {name}; call project_list"));
                };
                scope.push(project.name.clone());
                // The machine as well as the folder. A project is a folder on *this* machine,
                // and on a store shared with a build box a run whose `cwd` spells the same path
                // is work nobody did here — see `run_history::matches_root`.
                Some(run_history::RootScope::new(
                    project.root.clone(),
                    self.host_id.clone(),
                ))
            },
        };
        let kind = args
            .kind
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        if let Some(kind) = kind {
            scope.push(kind.to_owned());
        }
        let since_ms = args.since_ms(agents::now_ms())?;
        if let Some(since) = args.since.as_deref().filter(|_| since_ms.is_some()) {
            scope.push(since.trim().to_owned());
        }
        if let Some(active) = args.active {
            scope.push(match active {
                true => "running".to_owned(),
                false => "finished".to_owned(),
            });
        }
        Ok((
            run_history::Filter {
                zellij_session_name: zellij_session_name.map(Into::into),
                host_id,
                root,
                kind: kind.map(str::to_owned),
                since_ms,
                active: args.active,
            },
            scope.join(" · "),
        ))
    }

    /// One run's header, as a reply object. Shared by all three tools so a run
    /// describes itself the same way wherever it appears.
    fn run_reply(&self, header: &agents::AgentRunHeader, now_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "run_id": header.id,
            "kind": header.kind,
            "title": header.title,
            "pane_id": header.pane_id,
            "cwd": header.cwd,
            // A Firecracker run's `cwd` is a guest path, so its project is the one that
            // owns the host worktree its VM mirrored — the rule `agents::checkout_location`
            // states for a live pane, applied to a run that may have no pane left.
            "project": agents::checkout_location(
                    header.worktree.as_deref(),
                    header.host.as_deref(),
                    header.cwd.as_deref(),
                )
                .and_then(|location| projects::for_cwd(&self.projects, &location))
                .map(|project| project.name.clone()),
            "active": matches!(header.outcome, agents::RunOutcome::Active),
            "outcome": match header.outcome {
                agents::RunOutcome::Active => "active",
                agents::RunOutcome::Completed => "completed",
                agents::RunOutcome::Closed => "closed",
                agents::RunOutcome::Replaced => "replaced",
            },
            "started_at_ms": header.started_at_ms,
            "ended_at_ms": header.ended_at_ms,
            "age": agents::short_age(now_ms, header.started_at_ms),
            "entries": header.revision,
            "summary": header.summary.as_ref().map(|summary| summary.text.clone()),
        })
    }

    pub(crate) fn agent_runs(&self, args: &tools::Args) -> String {
        let (filter, scope) = match self.run_filter(args) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let limit = args.limit.unwrap_or(20);
        let offset = args.offset.unwrap_or(0);
        let page = match run_history::list_runs(&self.store(), &filter, limit, offset) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let now_ms = agents::now_ms();
        let runs: Vec<serde_json::Value> = page
            .headers
            .iter()
            .map(|header| self.run_reply(header, now_ms))
            .collect();
        serde_json::json!({
            "scope": scope,
            "count": runs.len(),
            "offset": offset,
            "more": page.more,
            // The rows this page consumed, not the runs it kept: a project filter is applied
            // after the store has counted the rows, so paging on the kept count hands back
            // the same runs for ever. See `RunPage::scanned`.
            "next_offset": page.more.then(|| offset.saturating_add(page.scanned)),
            "runs": runs,
        })
        .to_string()
    }

    /// Read one run's timeline.
    ///
    /// Three ways to say where to start, and they answer different questions:
    /// `revision` is a hit to read *around* and comes with a lead-in, so a result
    /// arrives with the prompt behind it; `from_revision` is plain forward paging,
    /// fed from the previous reply's `next_revision`; and neither means the end of
    /// the run, which is where a run that is still going has got to.
    pub(crate) fn agent_run(&self, args: &tools::Args) -> String {
        let backend = self.store();
        let run_id = match self.named_run(args) {
            Ok(run_id) => run_id,
            Err(error) => return error,
        };
        let Some(header) = (match run_history::run_header(&backend, &run_id) {
            Ok(header) => header,
            Err(error) => return error,
        }) else {
            return format!("no such run {run_id}; call agent_runs");
        };
        let from = args
            .from_revision
            .or_else(|| args.revision.map(run_history::anchored_from));
        let page = match run_history::entry_page(&backend, &run_id, from, args.limit.unwrap_or(20))
        {
            Ok(page) => page,
            Err(error) => return error,
        };
        let now_ms = agents::now_ms();
        let entries: Vec<serde_json::Value> = page
            .entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "revision": entry.revision,
                    "kind": entry.kind.label(),
                    "created_at_ms": entry.created_at_ms,
                    "age": entry
                        .created_at_ms
                        .map(|at| agents::short_age(now_ms, at)),
                    "text": entry.text,
                })
            })
            .collect();
        let mut reply = self.run_reply(&header, now_ms);
        if let Some(object) = reply.as_object_mut() {
            object.insert("entry_count".to_owned(), serde_json::json!(page.total));
            object.insert("shown".to_owned(), serde_json::json!(entries.len()));
            object.insert("earlier".to_owned(), serde_json::json!(page.earlier));
            object.insert(
                "next_revision".to_owned(),
                serde_json::json!(page.next_revision),
            );
            // The exact call that reads the rest, for the reason `read_answer`
            // names one: a model holding a fragment it believes is whole draws
            // conclusions from the part it happened to get.
            object.insert(
                "more".to_owned(),
                serde_json::json!(page
                    .next_revision
                    .map(|next| format!("agent_run run_id={run_id} from_revision={next}"))),
            );
            object.insert("entries".to_owned(), serde_json::json!(entries));
        }
        reply.to_string()
    }

    /// Which run `agent_run` was asked for: named outright, or the one in a pane.
    ///
    /// A pane is the second way in because the live case is the common one — "what
    /// has the agent in pane 4 been doing" is asked without a run id in hand, and
    /// `agent_list` gives pane ids rather than run ids.
    fn named_run(&self, args: &tools::Args) -> Result<String, String> {
        if let Some(run_id) = args
            .run_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            return Ok(run_id.to_owned());
        }
        let Some(pane_id) = args.pane_id else {
            return Err(
                "run_id is required; call agent_runs, or pass pane_id for a live agent".to_owned(),
            );
        };
        // What the panel already knows, then the pane's own record — a pane whose
        // run the archive pager has not loaded still names it on its `AgentInfo`.
        if let Some(header) = self.active_runs.get(&pane_id) {
            return Ok(header.id.clone());
        }
        self.panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .and_then(|pane| pane.agent.as_ref())
            .and_then(agents::stored_run_id)
            .ok_or_else(|| format!("pane {pane_id} has no agent run; call agent_runs"))
    }

    pub(crate) fn agent_history_search(&self, args: &tools::Args) -> String {
        let Some(query) = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
        else {
            return "query is required".to_owned();
        };
        let (filter, scope) = match self.run_filter(args) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let run_id = args
            .run_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty());
        let found = match run_history::search(
            &self.store(),
            &filter,
            run_id,
            query,
            args.limit.unwrap_or(25),
        ) {
            Ok(found) => found,
            Err(error) => return error,
        };
        let now_ms = agents::now_ms();
        let matches: Vec<serde_json::Value> = found
            .hits
            .iter()
            .map(|hit| {
                let run = found.runs.get(&hit.run_id);
                serde_json::json!({
                    "run_id": hit.run_id,
                    "revision": hit.revision,
                    "kind": hit.kind.label(),
                    "created_at_ms": hit.created_at_ms,
                    "age": hit.created_at_ms.map(|at| agents::short_age(now_ms, at)),
                    "snippet": hit.snippet,
                    "run_title": run.map(|run| run.title.clone()),
                    "agent": run.map(|run| run.kind.clone()),
                    // Both halves of the address, as the call that uses them.
                    "read": format!(
                        "agent_run run_id={} revision={}",
                        hit.run_id, hit.revision
                    ),
                })
            })
            .collect();
        serde_json::json!({
            "query": query,
            "scope": scope,
            "count": matches.len(),
            "truncated": found.truncated,
            "matches": matches,
        })
        .to_string()
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
    /// One route: `tools::spawn_agent`, the host command the New agent modal's button and a
    /// TypeScript plugin's `zellij.agent.spawn` also call, so `agent=cursor model=x prompt=y` means
    /// the same argv and the same worktree defaults everywhere. `launch::plan` is run here
    /// first only to say what the host will refuse before anything is opened - and to know
    /// whether a checkout is being made, which decides what `cwd` means: with one, the
    /// folder is the repository to cut it in; without, it is where the pane runs, with no
    /// git work at all (`command=zsh cwd=/tmp` keeps working).
    pub(crate) fn agent_spawn(&mut self, args: &tools::Args) -> String {
        let mut spec = args.launch_spec();
        let selected_project = args.project.as_deref().or(self.active_project.as_deref());
        let project = match selected_project {
            Some(name) => match self.projects.get(name) {
                Some(project) => Some(project),
                None => return format!("unknown project {name}; call project_list"),
            },
            None => None,
        };
        if spec.worktree_dir.is_none()
            && spec
                .branch
                .as_deref()
                .is_some_and(|branch| !branch.trim().is_empty())
            && spec.worktree != Some(false)
        {
            spec.worktree_dir = project.and_then(|project| project.worktree_dir.clone());
        }
        if args.firecracker {
            spec.firecracker = Some(launch::unique_firecracker_vm());
        }
        apply_project_firecracker_image(
            &mut spec,
            project.and_then(|project| project.firecracker_image.clone()),
        );
        let firecracker = spec
            .firecracker
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty());
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
        let project_firecracker =
            firecracker && (args.project.is_some() || self.active_project.is_some());
        let no_checkout = matches!(checkout, GitCheckout::Current);
        // With no project, `cwd` can only be the caller's: a path on this machine. Handed to
        // the guest as its working directory it would not exist there, and the agent would
        // start wherever the broker happens to be. A project's root is mirrored instead - the
        // host treats it as the checkout to mirror and the provider supplies the guest folder.
        if no_checkout && !project_firecracker && command.target.is_firecracker() && cwd.is_some() {
            return "cwd is a path on this machine, and a Firecracker agent starts in the \
                    VM's workspace; leave cwd unset or name a project"
                .to_owned();
        }
        let run_in = if no_checkout && !project_firecracker {
            cwd.clone()
        } else {
            None
        };
        // A VM is named after the folder it mirrors when there is one: the checkout being cut,
        // or the project's root. Outside both there is no folder for the agent at all, and the
        // unique name stands - the isolated VM `firecracker=true` promises, one per agent.
        let automatic_vm = firecracker && (!no_checkout || project_firecracker);
        let result = tools::spawn_agent(args, &spec, cwd, run_in, automatic_vm);
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
                self.send_overrides();
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

    /// Take the agents pane's announcement of itself.
    ///
    /// The pane id still places/focuses a live agent; a run id asks for selected history.
    pub(crate) fn on_selection(&mut self, message: &PipeMessage) -> bool {
        let Some(selection) = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::Selection>(json).ok())
        else {
            return false;
        };
        if message.source != PipeSource::Plugin(selection.plugin_id)
            || self.listed_as_agent_list(selection.plugin_id) == Some(false)
        {
            return false;
        }
        let announced = self.agents_plugin != Some(selection.plugin_id);
        self.agents_plugin = Some(selection.plugin_id);
        self.send_vm_panel_visibility(!self.hidden);
        if announced && self.multipane.is_none() {
            self.agents_tab_index = None;
            self.apply_overlay_geometry();
        }
        self.selected_agent = selection.pane_id;
        self.selected_run_revision = selection.revision;
        self.selected_run = selection.run_id.or_else(|| {
            selection
                .pane_id
                .and_then(|pane_id| self.active_runs.get(&pane_id))
                .map(|run| run.id.clone())
        });
        if let Some(run_id) = self.selected_run.as_deref() {
            self.send_run_detail(selection.plugin_id, run_id, self.selected_run_revision);
        }
        false
    }

    pub(crate) fn on_agent_predecessors(&mut self, message: &PipeMessage) -> bool {
        let PipeSource::Plugin(plugin_id) = message.source else {
            return false;
        };
        if self.listed_as_agent_list(plugin_id) != Some(true) {
            return false;
        }
        let Some(request) = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::PredecessorsRequest>(json).ok())
        else {
            return false;
        };
        let result = self.agent_predecessors(request);
        if let Ok(payload) = serde_json::to_string(&result) {
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::AGENT_PREDECESSORS_RESULT)
                    .with_destination_plugin_id(plugin_id)
                    .with_payload(payload),
            );
        }
        false
    }

    pub(crate) fn agent_predecessors(
        &self,
        request: wire::PredecessorsRequest,
    ) -> wire::PredecessorsResult {
        let mut result = wire::PredecessorsResult {
            request_id: request.request_id,
            pane_id: request.pane_id,
            anchor: self.active_runs.get(&request.pane_id).cloned(),
            ..Default::default()
        };
        if let Some(anchor) = &result.anchor {
            (result.runs, result.diagnostic) = run_history::predecessors(anchor, |id| {
                if let Some(header) = self
                    .active_runs
                    .values()
                    .chain(self.archived_runs.iter())
                    .find(|run| run.id == id)
                {
                    Ok(Some(header.clone()))
                } else {
                    run_history::run_header(&self.store(), id)
                }
            });
        } else {
            result.diagnostic = "No recorded active run for this agent".to_owned();
        }
        result
    }

    pub(crate) fn on_agent_search(&mut self, message: &PipeMessage) -> bool {
        let Some(plugin_id) = (match message.source {
            PipeSource::Plugin(id) => Some(id),
            _ => None,
        }) else {
            return false;
        };
        if self.listed_as_agent_list(plugin_id) != Some(true) {
            return false;
        }
        let Some(request) = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::SearchRequest>(json).ok())
        else {
            return false;
        };
        let mut result = wire::SearchResult {
            request_id: request.request_id,
            ..Default::default()
        };
        if request.query.trim().is_empty() {
            result.error = "search text is empty".to_owned();
        } else if let Err(error) = self.search_agent_runs(&request.query, &mut result) {
            result.error = error;
        }
        let Ok(payload) = serde_json::to_string(&result) else {
            return false;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(wire::AGENT_SEARCH_RESULT)
                .with_destination_plugin_id(plugin_id)
                .with_payload(payload),
        );
        false
    }

    fn search_agent_runs(
        &self,
        query: &str,
        result: &mut wire::SearchResult,
    ) -> Result<(), String> {
        let found = run_history::search_runs(&self.store(), query)?;
        result.truncated = found.truncated;
        for hit in found.hits {
            let Some(header) = found.runs.get(&hit.run_id) else {
                continue;
            };
            result.runs.push(wire::SearchRun {
                run_id: hit.run_id,
                kind: header.kind.clone(),
                title: header.title.clone(),
                revision: hit.revision,
                snippet: hit.snippet,
                started_at_ms: header.started_at_ms,
                worktree: agents::worktree_cell(header),
            });
        }
        Ok(())
    }

    pub(crate) fn merge_run_headers(&mut self, runs: Vec<agents::AgentRunHeader>) {
        let active: Vec<u32> = runs
            .iter()
            .filter(|run| run.ended_at_ms.is_none())
            .map(|run| run.pane_id)
            .collect();
        self.active_runs
            .retain(|pane_id, _| active.contains(pane_id));
        for run in runs {
            if run.ended_at_ms.is_none() {
                self.active_runs.insert(run.pane_id, run);
            } else if let Some(existing) = self
                .archived_runs
                .iter_mut()
                .find(|existing| existing.id == run.id)
            {
                *existing = run;
            } else {
                if self
                    .archive_loaded_through
                    .as_ref()
                    .is_some_and(|(started_at_ms, id)| {
                        (std::cmp::Reverse(run.started_at_ms), run.id.as_str())
                            < (std::cmp::Reverse(*started_at_ms), id.as_str())
                    })
                {
                    self.archive_offset = self.archive_offset.saturating_add(1);
                }
                self.archived_runs.insert(0, run);
            }
        }
        self.archived_runs.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        if let Some(run) = self
            .selected_agent
            .and_then(|pane_id| self.active_runs.get(&pane_id))
        {
            self.selected_run = Some(run.id.clone());
        }
        if let (Some(plugin_id), Some(run_id)) = (self.agents_plugin, self.selected_run.as_deref())
        {
            self.send_run_detail(plugin_id, run_id, self.selected_run_revision);
        }
    }

    /// Merge an event-fed daemon snapshot.
    pub(crate) fn apply_poll(
        &mut self,
        panes: Vec<agents::AgentPane>,
        activity: agents::Activity,
        summaries: agents::Summaries,
        hooks: agents::HookFeeds,
        focused: Option<u32>,
    ) {
        self.activity = activity;
        let before = std::mem::replace(&mut self.summaries, summaries);
        self.persist_summaries(&before);
        self.hooks = hooks;
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
            self.send_overrides();
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
        let job = Job {
            id,
            every,
            due_in: every,
            prompt,
            show,
            auto_confirm: args.auto_confirm,
        };
        let row = transitions::job_row(&job, self.chat_id, transitions::now_secs());
        self.jobs.push(job);
        self.persist_job(row);
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

    /// Cancel one job by id, or every job. Across every open chat, not only
    /// the one this runs in: jobs live with the chat that armed them, but the
    /// person asking — or the model, on their behalf — is wherever they are,
    /// and a job that can only be stopped from the tab it was set in is a job
    /// that keeps firing until that tab is found.
    pub(crate) fn unschedule(&mut self, id: Option<u64>) -> String {
        let result = match id {
            Some(id) => {
                let before = self.total_job_count();
                for jobs in self.all_jobs_mut() {
                    jobs.retain(|job| job.id != id);
                }
                self.held_jobs.retain(|_, jobs| !jobs.is_empty());
                if self.total_job_count() == before {
                    format!("no job {id}")
                } else {
                    self.forget_jobs(vec![id]);
                    format!("cancelled job {id}")
                }
            },
            None => {
                let count = self.total_job_count();
                let mut gone = Vec::new();
                for jobs in self.all_jobs_mut() {
                    gone.extend(jobs.drain(..).map(|job| job.id));
                }
                self.held_jobs.clear();
                self.forget_jobs(gone);
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

    /// The kill switch that doesn't need the model to cooperate. Every job in
    /// every tab, from whichever tab it is typed in - and every job held for
    /// a chat that is not open, which no tab could reach.
    pub(crate) fn stop_jobs(&mut self) -> bool {
        if self.total_job_count() == 0 {
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

    /// Enter planning mode and ask `prompt` inside it, in one submit.
    ///
    /// Not a toggle: `/execute` is how a plan is carried out and `Ctrl-l` is how
    /// one is abandoned, so nothing here turns the mode off. Typing `/plan`
    /// again while already planning is simply the next planning turn, which is
    /// what makes it usable as a prefix on a correction.
    pub(crate) fn start_plan(&mut self, prompt: &str) -> bool {
        self.pin_to_bottom();
        let was_on = self.plan_mode_on();

        // The mode changes *first*, and the pending call is dealt with second.
        // `decline_head` answers the whole batch and sends nothing; the request
        // that follows is `send_user_text` below, sending the prompt, and it has
        // to go out as a planning request — with the read-only tool set — which
        // it only does if the mode is already on when it leaves.
        self.plan = Plan::Planning;

        // Entering it while a destructive call is waiting on a y/n would leave
        // that call armed inside the mode that exists to disarm it. Declining is
        // the honest reading of "stop acting, start planning", and it keeps the
        // transcript valid — the model gets a result for the call it made.
        //
        // `decline_head` rather than `decline_tool`: the prompt typed after
        // `/plan` is the question this turn is about, so the queue is left alone
        // and `send_user_text` below is what goes out. Anything queued earlier
        // follows it through the usual drain, once the planning reply settles.
        let declined = !was_on
            && self.is_confirming()
            && self.decline_head(
                "not run: the user switched to planning mode, so this tool is withheld. \
                 Finish the plan; the user can approve it and you will get the tool back.",
            );

        // Only on the way in: an established mode does not announce itself again
        // in front of every prompt typed inside it.
        if !was_on {
            let notice = if declined {
                "planning mode on — the pending call was declined; read-only tools until you \
                 /execute the plan"
            } else {
                "planning mode on — read-only tools until you /execute the plan"
            };
            self.messages.push(Message::notice(notice));
            self.log("plan_mode", serde_json::json!({"on": true}));
            self.log("notice", serde_json::json!({"content": notice}));
        }

        // `send_user_text` answers "was anything sent", which is false when there
        // is no API key to send it with. Entering the mode pushed a notice even
        // then, so the pane still has something new to draw.
        let sent = self.send_user_text(prompt.to_owned());
        sent || !was_on
    }

    /// `/execute` on a plan: leave planning mode and let the model carry out what
    /// it just wrote. `note` is the optional amendment typed after the command.
    ///
    /// The approval goes in as a `Role::User` message rather than a notice
    /// because a notice is dropped on the way out (`api::request_body_with_project`
    /// skips `Role::Notice`), so the model would be handed two assistant turns
    /// in a row and nothing that asked for the second.
    ///
    /// With `fresh`, the plan is carried out in a new chat instead: the planning
    /// chat is parked in its tab, plan and research and all, and the new one
    /// starts from a single user turn that quotes the plan. What the executor
    /// sees is the plan and nothing that was read to write it.
    pub(crate) fn approve_plan(&mut self, note: &str, fresh: bool) -> bool {
        const APPROVAL: &str = "Approved. Carry out the plan you just wrote.";
        const FRESH_APPROVAL: &str = "Approved. Carry out this plan:";
        self.pin_to_bottom();
        if !self.awaiting_plan() {
            // The two states mean different things to whoever typed it: one has
            // no plan *yet*, the other was never planning at all.
            let error = if self.plan_mode_on() {
                "nothing to execute yet — hocus has not written a plan"
            } else {
                "not planning — /plan <prompt> starts a plan"
            };
            self.messages.push(Message::error(error));
            self.log("error", serde_json::json!({"error": error}));
            return true;
        }
        // Unwrap-free: a plan is awaiting, so the prose that is the plan is in
        // the transcript. Were it somehow not, in place is the honest fallback.
        let plan = fresh
            .then(|| self.plan_index())
            .flatten()
            .map(|at| self.messages[at].content.clone());
        self.plan = Plan::Off;
        self.log(
            "plan_decision",
            serde_json::json!({"decision": "approve", "fresh": plan.is_some()}),
        );
        let notice = if plan.is_some() {
            "planning mode off — carrying out the plan in a new chat"
        } else {
            "planning mode off — carrying out the plan"
        };
        self.messages.push(Message::notice(notice));
        self.log("notice", serde_json::json!({"content": notice}));
        let content = match &plan {
            Some(plan) if note.is_empty() => format!("{FRESH_APPROVAL}\n\n{plan}"),
            Some(plan) => format!("{FRESH_APPROVAL}\n\n{plan}\n\n{note}"),
            None if note.is_empty() => APPROVAL.to_owned(),
            None => format!("{APPROVAL} {note}"),
        };
        if plan.is_some() {
            // The planning chat has messages, so this always parks it rather
            // than resetting in place; the foreground is empty from here on.
            self.new_chat();
        }
        self.messages.push(Message::user(content.clone()));
        self.log(
            "user_input",
            serde_json::json!({"content": log::truncate(&content)}),
        );
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
        let notice = "still planning — say what to change, /execute to carry it out, or /plan off \
                      to leave planning mode";
        self.messages.push(Message::notice(notice));
        self.log("notice", serde_json::json!({"content": notice}));
        true
    }

    /// `/plan off`: leave planning mode without carrying the plan out.
    ///
    /// The third answer to a plan, beside `/execute` and `n`. Nothing is sent and
    /// nothing is thrown away: the plan stays in the transcript as the assistant
    /// message it already is, and the next thing typed goes out as an ordinary
    /// question with the full tool set behind it. Until this existed the only way
    /// out of the mode that did not carry the plan out was `Ctrl-l`, which parks
    /// the whole conversation and leaves an empty one.
    ///
    /// No `plan_cache` to clear and no user turn to push: the cache is keyed on
    /// what it rendered, so a stale entry is never reused, and a request that is
    /// not being made needs nothing said to the model. The box comes down because
    /// `confirmation_prompt` stops offering it.
    pub(crate) fn stop_plan(&mut self) -> bool {
        if !self.plan_mode_on() {
            // The same wording `approve_plan` gives for "was never planning".
            let error = "not planning — /plan <prompt> starts a plan";
            self.messages.push(Message::error(error));
            self.log("error", serde_json::json!({"error": error}));
            return true;
        }
        let had_plan = self.awaiting_plan();
        self.plan = Plan::Off;
        self.pin_to_bottom();
        self.log(
            "plan_decision",
            serde_json::json!({"decision": "stop", "had_plan": had_plan}),
        );
        self.log("plan_mode", serde_json::json!({"on": false}));
        // A plan that was waiting is the thing the person is answering, so say
        // what became of it; without one there is only the mode to report.
        let notice = if had_plan {
            "planning mode off — the plan was not carried out; it is still above"
        } else {
            "planning mode off — the full tool set is back"
        };
        self.messages.push(Message::notice(notice));
        self.log("notice", serde_json::json!({"content": notice}));
        true
    }

    pub(crate) fn show_tool_confirmation(&mut self, call: &ToolCall) {
        self.status = Status::ConfirmingTool;
        self.pin_to_bottom();
        // Reach the person even when this chat is parked or Hocus is hidden.
        tools::toast_for_chat(
            &self.confirmation_toast(call),
            PaneId::Plugin(self.own_plugin_id),
            self.chat_id,
        );
        self.raise_spotlight(call);
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

    /// `n` on a tool call: the call is answered, and so is every call queued
    /// behind it, and then nothing happens until the user says something.
    ///
    /// The results are recorded because the model must see a reply for every
    /// call it made or the next request is malformed — but they are not sent.
    /// A "no" is the user taking the turn back: handing the decline straight to
    /// the model would have it try something else on its own, and running the
    /// rest of the batch would carry out calls nobody agreed to.
    pub(crate) fn decline_tool(&mut self) -> bool {
        if !self.decline_head("not run: the user declined an earlier call in this batch") {
            return false;
        }
        // Something typed while the prompt was up *is* the next message, and it
        // has been waiting on this answer. The timer would get to it eventually;
        // this is the moment it was queued for. Through `drain_asks` rather than
        // past it: the one-stream-slot and open-picker guards hold here too.
        if !self.drain_asks() {
            // Only when nothing went out — with a question in flight the chat is
            // waiting on the model again, not on the user.
            let notice = "declined — waiting for your next message";
            self.messages.push(Message::notice(notice));
            self.log("notice", serde_json::json!({"content": notice}));
        }
        self.pin_to_bottom();
        true
    }

    /// Answer the head as declined and halt the batch behind it, saying nothing
    /// and sending nothing. The two callers differ only in what that batch is
    /// told: `n` says the user declined, `/plan` says the tools are withheld.
    fn decline_head(&mut self, rest_result: &str) -> bool {
        let Some(head) = self.pending.front() else {
            return false;
        };
        self.log(
            "tool_confirm",
            serde_json::json!({
                "tool_call_id": head.id,
                "tool_name": head.name,
                "decision": "decline",
            }),
        );
        self.halt_tools("declined by the user", rest_result);
        true
    }

    /// Answer the head with `head_result` and everything behind it with
    /// `rest_result`, and leave the queue empty without starting the model's
    /// follow-up. Shared by the two exits that end a turn early — a decline and
    /// an abort — so that neither can drift into pumping the queue.
    fn halt_tools(&mut self, head_result: &str, rest_result: &str) {
        if self.status == Status::AwaitingPluginTest {
            if let Some(call) = self.pending.front() {
                zellij_tile::shim::cancel_plugin_test(&call.id);
            }
        }
        self.drop_spotlight();
        self.status = Status::Idle;
        // The model asked, but it is not getting an answer — the results are
        // recorded for the transcript's sake, not to be sent back.
        self.owes_reply = false;
        self.finish_head(head_result);
        while !self.pending.is_empty() {
            self.finish_head(rest_result);
        }
    }

    /// Give up on the whole run. Every outstanding call still gets an answer, so
    /// the transcript stays valid if the conversation continues.
    ///
    /// This is also the escape hatch for a dobbs call that never returns,
    /// which is why there is no timer.
    pub(crate) fn abort_tools(&mut self) -> bool {
        self.scheduled_run = None;
        self.halt_tools("aborted by the user", "aborted by the user");
        self.messages.push(Message::notice("tools aborted"));
        self.log("notice", serde_json::json!({"content": "tools aborted"}));
        self.log("abort", serde_json::json!({"reason": "user"}));
        self.pin_to_bottom();
        true
    }
}

/// The note an id names, or the error that says what to call.
fn existing_note(backend: &impl Backend, id: u64) -> Result<notes::Note, String> {
    notes::fetch_note(backend, id)?.ok_or_else(|| format!("no note {id}; call note_list"))
}

pub(crate) fn apply_project_firecracker_image(
    spec: &mut launch::Spec,
    project_image: Option<String>,
) {
    if spec
        .firecracker
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty())
        && spec
            .firecracker_image
            .as_deref()
            .is_none_or(|image| image.trim().is_empty())
    {
        spec.firecracker_image = project_image;
    }
}
