use crate::message::{Message, Role, Status, StreamState};
use crate::*;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

impl State {
    pub(crate) fn active_stream_id(&self) -> Option<String> {
        self.stream.as_ref().map(|stream| stream.id.clone())
    }

    /// Lift the foreground conversation out, leaving an empty one behind.
    pub(crate) fn take_chat(&mut self) -> Chat {
        self.transcript_cache = None;
        Chat {
            messages: std::mem::take(&mut self.messages),
            status: std::mem::take(&mut self.status),
            stream: self.stream.take(),
            reply_at: self.reply_at.take(),
            sse: std::mem::take(&mut self.sse),
            tool_calls: std::mem::take(&mut self.tool_calls),
            pending: std::mem::take(&mut self.pending),
            tool_rounds: std::mem::take(&mut self.tool_rounds),
            owes_reply: std::mem::take(&mut self.owes_reply),
            manual_head: std::mem::take(&mut self.manual_head),
            chat_id: self.chat_id.take(),
            saved: std::mem::take(&mut self.saved),
            scroll: std::mem::take(&mut self.scroll),
            job_auto_confirm: std::mem::take(&mut self.job_auto_confirm),
        }
    }

    /// Put one in the foreground. Whatever was there is overwritten, so this is
    /// always paired with a `take_chat`.
    pub(crate) fn put_chat(&mut self, chat: Chat) {
        self.transcript_cache = None;
        self.messages = chat.messages;
        self.status = chat.status;
        self.stream = chat.stream;
        self.reply_at = chat.reply_at;
        self.sse = chat.sse;
        self.tool_calls = chat.tool_calls;
        self.pending = chat.pending;
        self.tool_rounds = chat.tool_rounds;
        self.owes_reply = chat.owes_reply;
        self.manual_head = chat.manual_head;
        self.chat_id = chat.chat_id;
        self.saved = chat.saved;
        self.scroll = chat.scroll;
        self.job_auto_confirm = chat.job_auto_confirm;
    }

    /// Run `f` against a parked chat by swapping it into the foreground for the
    /// duration and swapping the real one back afterwards.
    ///
    /// Doing it this way is what keeps `on_chunk`, `pump_tools` and the rest
    /// single-conversation code: they only ever see the chat they are for, in
    /// the one place they know to look for it. A chat that settles while it is
    /// in here is archived and dropped rather than re-parked — `/history` reads
    /// it back from the file.
    pub(crate) fn with_parked<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let foreground = self.take_chat();
        let parked = self.parked.remove(index);
        self.put_chat(parked);

        let out = f(self);
        self.autosave();

        let background = self.take_chat();
        self.put_chat(foreground);
        if background.is_live() {
            self.parked.insert(index, background);
        }
        out
    }

    /// Which parked chat a response belongs to, by whichever id its context
    /// carries. `None` means the foreground's — including when it is nobody's,
    /// which the handlers already turn away on their own.
    pub(crate) fn owner_of(&self, context: &BTreeMap<String, String>) -> Option<usize> {
        if let Some(stream_id) = context.get(STREAM_CONTEXT) {
            return self.parked_with_stream(stream_id);
        }
        self.parked_with_call(context.get("tool_call_id")?)
    }

    /// Which parked chat started this stream, if any.
    pub(crate) fn parked_with_stream(&self, stream_id: &str) -> Option<usize> {
        self.parked
            .iter()
            .position(|chat| chat.stream.as_ref().is_some_and(|s| s.id == stream_id))
    }

    /// Which parked chat is waiting on this tool call. Only the head can be:
    /// the queue runs one at a time.
    pub(crate) fn parked_with_call(&self, call_id: &str) -> Option<usize> {
        self.parked
            .iter()
            .position(|chat| chat.pending.front().is_some_and(|call| call.id == call_id))
    }

    /// A reply is arriving, or its trailing chunk is still owed.
    pub(crate) fn is_streaming(&self) -> bool {
        self.stream.is_some()
    }

    /// A reply is arriving *and* is still ours to record — `finishing` means the
    /// outcome is already in the transcript and the rest is being swallowed.
    pub(crate) fn is_receiving(&self) -> bool {
        matches!(&self.stream, Some(stream) if !stream.finishing)
    }

    /// The tool queue is doing something. Independent of the stream: both can be
    /// true at once, which is the point of the split.
    pub(crate) fn tools_busy(&self) -> bool {
        !matches!(self.status, Status::Idle)
    }

    /// Anything at all outstanding. The archive and the `Ctrl-l` teardown care
    /// about this rather than about which half it is.
    pub(crate) fn is_in_flight(&self) -> bool {
        self.is_streaming() || self.tools_busy()
    }

    /// The call awaiting `y`/`n`, if any. Rendered so the prompt names the one
    /// call it is about rather than leaving the user to infer it.
    pub fn confirmation_prompt(&self) -> Option<String> {
        match self.status {
            Status::ConfirmingTool => self.pending.front().map(tools::summarize),
            _ => None,
        }
    }

    /// Whether we are waiting on something *and* nothing is arriving — the one
    /// predicate the whole waiting indicator keys off.
    pub(crate) fn is_loading(&self) -> bool {
        match self.status {
            Status::AwaitingCommand | Status::AwaitingSearch | Status::AwaitingStore => true,
            // A confirmation prompt is waiting on the user rather than on us,
            // so the only thing left to be waiting on is the reply. Once its
            // first token lands the arriving text is its own proof of life; a
            // spinner beside streaming words is just noise. `finishing` has
            // already recorded its outcome and is waiting on nothing.
            Status::Idle => self.is_receiving() && self.reply_is_empty(),
            Status::ConfirmingTool => false,
        }
    }

    /// Whether the timer chain should keep running. Wider than `is_loading`:
    /// a stream that is already producing text still needs frames, because
    /// `on_chunk` no longer asks for a render of its own and this is what
    /// repaints the arriving reply.
    pub(crate) fn wants_frames(&self) -> bool {
        self.is_loading() || self.is_receiving()
    }

    /// The spinner and its phrase, or `None` when nothing is being waited on.
    pub fn loader(&self) -> Option<String> {
        if !self.is_loading() {
            return None;
        }
        let spinner = ui::spinner(self.tick);

        // A Slack call names itself: "running: slack_backfill" beats a joke when
        // what you actually want to know is what you are waiting on.
        let head = self.pending.front();
        if matches!(self.status, Status::AwaitingCommand) {
            if let Some(call) = head {
                return Some(format!("{spinner} running: {}", call.name));
            }
        } else if matches!(self.status, Status::AwaitingSearch) {
            let args = head.and_then(|call| tools::Args::parse(&call.arguments).ok());
            if let Some(query) = args.and_then(|args| args.query) {
                return Some(format!("{spinner} searching: {query}"));
            }
        }

        Some(format!(
            "{spinner} {}",
            ui::phrase(self.tick, self.stream_seq)
        ))
    }

    /// Schedule the next tick. `set_timeout` is one-shot, so the spinner
    /// animation, the streaming repaint and the job countdown are all this
    /// being re-armed from `on_timer` for as long as there is something to do.
    ///
    /// ponytail: one chain, so a request that starts while a slow job tick is
    /// outstanding can show its spinner up to `JOB_SECONDS` late — Zellij has
    /// no cancel_timeout, and a second chain would defeat `timer_armed`.
    pub(crate) fn arm_timer(&mut self, interval: f64) {
        if !self.timer_armed {
            set_timeout(interval);
            self.timer_armed = true;
        }
    }

    /// Tell `hocus-agentd` to push snapshots here, and hand it the manual kind
    /// overrides it needs to classify with.
    ///
    /// Broadcast — no url, no destination id — which looks wasteful and is the
    /// only thing that works. Addressing it by url would *launch* a second
    /// daemon rather than find the running one: the host injects `caller_cwd`
    /// into the target's configuration, plugin identity is (location,
    /// configuration), and so a piped url never matches the instance that
    /// `load_plugins` started. A broadcast goes to already-running plugins only
    /// and launches nothing.
    ///
    /// Cheap and idempotent, so it is re-sent whenever `manual_kinds` changes
    /// rather than tracked; the daemon just records us as its subscriber again.
    pub(crate) fn subscribe_to_agentd(&mut self) {
        let Ok(payload) = serde_json::to_string(&PollRequest {
            manual_kinds: self.manual_kinds.clone(),
            activity: self.activity.clone(),
        }) else {
            return;
        };
        // Forget the watermark. A daemon that was reloaded restarts its `seq` at
        // one, and holding the old high-water mark would drop every snapshot it
        // ever sent again. Re-subscribing is exactly the moment we suspect that,
        // so it is also the moment to stop expecting the old numbering.
        self.agent_seq = 0;
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE).with_payload(payload));
    }

    /// Ask `hocus-store` for the project registry and the todo lists, and to
    /// keep sending them. Broadcast for the same reason the agent subscribe is.
    pub(crate) fn subscribe_to_store(&mut self) {
        // Forget the watermark, same as the agent subscribe: a reloaded store
        // restarts its `seq` at one, and holding the old high-water mark would
        // drop every snapshot it ever sends again.
        self.store_seq = 0;
        pipe_message_to_plugin(MessageToPlugin::new(wire::STORE_SUBSCRIBE));
    }

    /// Send a change to `hocus-store` without waiting for it.
    ///
    /// For changes whose outcome the caller already knows — the store is being
    /// told, not asked. The reply, if any, matches no pending write and is
    /// dropped.
    pub(crate) fn send_store_write(&mut self, op: &str, args: serde_json::Value) {
        let mutation = wire::Mutation {
            request_id: String::new(),
            op: op.to_owned(),
            args,
        };
        if let Ok(payload) = serde_json::to_string(&mutation) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::STORE_MUTATE).with_payload(payload));
        }
    }

    /// Send a change to `hocus-store` and leave the tool call waiting on it.
    ///
    /// The call's own id is the correlation key, which is enough because the
    /// queue runs one call at a time — the reply can only ever be for the head.
    pub(crate) fn start_store_write(&mut self, call: &ToolCall, op: &str, args: serde_json::Value) {
        let mutation = wire::Mutation {
            request_id: call.id.clone(),
            op: op.to_owned(),
            args,
        };
        let Ok(payload) = serde_json::to_string(&mutation) else {
            self.finish_head("could not encode the change");
            return;
        };
        // The store may be absent — an old config with no `hocus-store` in
        // `load_plugins` — and a broadcast to nobody is silent. This is what
        // makes that a failed tool call rather than a tool loop that never
        // moves again.
        self.pending_write = Some((call.id.clone(), STORE_TIMEOUT_SECONDS));
        pipe_message_to_plugin(MessageToPlugin::new(wire::STORE_MUTATE).with_payload(payload));
        self.status = Status::AwaitingStore;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
    }

    /// Take a new copy of the project registry and the todo lists.
    ///
    /// Read-only here: everything downstream — `todo_list`, `agent_list`, the
    /// project named in each request's system message — answers out of this copy
    /// rather than paying a round trip, which is what keeps those tools instant.
    pub(crate) fn on_store_snapshot(&mut self, message: &PipeMessage) -> bool {
        let Some(snapshot) = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::StoreSnapshot>(json).ok())
        else {
            return false;
        };
        // Same watermark rule as the agent snapshot: never go backwards.
        if snapshot.seq <= self.store_seq {
            return false;
        }
        self.store_seq = snapshot.seq;
        self.projects = snapshot.projects;
        self.todos = snapshot.todos;
        // A project can be renamed or dropped out from under the tab maps —
        // discovery replaces a project when a shallower folder takes its
        // folders. Anything naming a project that no longer exists is dropped,
        // or `agent_list` answers "unknown project" for a tab forever.
        self.tab_projects
            .retain(|_, name| self.projects.contains_key(name));
        self.auto_tab_projects
            .retain(|_, name| self.projects.contains_key(name));
        if let Some(active) = &self.active_project {
            if !self.projects.contains_key(active) {
                self.active_project = None;
            }
        }
        self.refresh_tab_projects();
        true
    }

    /// Finish the tool call that was waiting on a change.
    pub(crate) fn on_store_result(&mut self, message: &PipeMessage) -> bool {
        let Some(result) = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::StoreResult>(json).ok())
        else {
            return false;
        };
        // Only the call we are actually waiting on. A late reply to a write
        // that already timed out would otherwise answer whatever call happens
        // to be at the head now.
        if self.pending_write.as_ref().map(|(id, _)| id.as_str())
            != Some(result.request_id.as_str())
        {
            return false;
        }
        self.pending_write = None;
        self.status = Status::Idle;
        self.finish_head(result.message);
        self.pump_tools();
        true
    }

    /// Give up on a store write that was never answered.
    ///
    /// Without this a store that is missing or wedged leaves the tool queue
    /// parked forever, which looks exactly like a hung model.
    pub(crate) fn tick_store(&mut self, elapsed: f64) -> bool {
        let Some((_, remaining)) = &mut self.pending_write else {
            return false;
        };
        *remaining -= elapsed;
        if *remaining > 0.0 {
            return false;
        }
        self.pending_write = None;
        self.status = Status::Idle;
        self.finish_head("the project and todo store did not answer; nothing was changed");
        self.pump_tools();
        true
    }

    pub(crate) fn on_timer(&mut self, elapsed: f64) -> bool {
        self.timer_armed = false;
        self.mono_ms = self.mono_ms.saturating_add((elapsed * 1000.0) as u64);
        let fired = self.tick_jobs(elapsed);
        let gave_up = self.tick_store(elapsed);
        let frames = self.wants_frames();
        if frames {
            self.tick += 1;
        }
        let fired = fired || gave_up;
        if frames || !self.jobs.is_empty() || self.pending_write.is_some() {
            self.arm_timer(if frames { FRAME_SECONDS } else { JOB_SECONDS });
        }
        // A quiet tick changed nothing on screen; asking for a render would
        // recomposite the whole session for nothing.
        frames || fired
    }

    /// Count the jobs down and start at most one due run. Returns whether the
    /// transcript changed.
    ///
    /// Only fires when `Idle`: there is exactly one stream slot, and a job that
    /// comes due mid-reply waits for a later tick rather than stacking runs.
    ///
    /// ponytail: a scheduled run that calls a destructive tool parks on its y/n
    /// prompt and holds every other job with it. Auto-declining confirmations
    /// on scheduled runs is the fix if that ever bites.
    pub(crate) fn tick_jobs(&mut self, elapsed: f64) -> bool {
        for job in &mut self.jobs {
            job.due_in -= elapsed;
        }
        if self.is_in_flight() || self.picker.is_some() {
            return false;
        }
        let Ok(cfg) = self.cfg.clone() else {
            return false;
        };
        let Some(job) = self.jobs.iter_mut().find(|job| job.due_in <= 0.0) else {
            return false;
        };

        job.due_in = job.every;
        let prompt = job.prompt.clone();
        let job_id = job.id;
        self.job_auto_confirm = job.auto_confirm;
        // Popping the pane open is also what makes a confirmation prompt
        // reachable, so the show flag covers both meanings of "tell me".
        if job.show {
            show_self(true);
        }
        // Mentions resolve at fire time, so "check @codex every minute" reads the
        // agent as it is now, not as it was when the job was armed.
        let mut fired = format!("[scheduled] {prompt}");
        if let Some(context) =
            agents::mention_context(&fired, &self.panes, &self.manual_kinds, &self.activity)
        {
            fired.push_str("\n\n");
            fired.push_str(&context);
        }
        self.messages.push(Message::user(fired));
        self.log(
            "job_fire",
            serde_json::json!({
                "job_id": job_id,
                "prompt": log::truncate(&prompt),
            }),
        );
        self.log(
            "user_input",
            serde_json::json!({
                "content": log::truncate(&format!("[scheduled] {prompt}")),
                "scheduled": true,
                "job_id": job_id,
            }),
        );
        // A fresh tool budget, same as a typed question.
        self.tool_rounds = 0;
        self.start_request(&cfg);
        true
    }

    /// The message the arriving reply is being appended to, by index. `None`
    /// once the stream has settled, or if the placeholder was dropped.
    pub(crate) fn reply(&mut self) -> Option<&mut Message> {
        let at = self.reply_at?;
        self.messages
            .get_mut(at)
            .filter(|message| message.role == Role::Assistant)
    }

    pub(crate) fn append_to_reply(&mut self, text: &str) {
        match self.reply() {
            Some(message) => message.content.push_str(text),
            // An error line replaced the placeholder; start a fresh reply rather
            // than appending model output onto the error text.
            None => {
                self.reply_at = Some(self.messages.len());
                self.messages.push(Message::assistant(text));
            },
        }
    }

    /// Whether the arriving reply is the last thing in the transcript. False
    /// once a tool typed mid-reply has pushed anything behind the placeholder.
    pub fn reply_is_last(&self) -> bool {
        self.reply_at == Some(self.messages.len().saturating_sub(1))
    }

    pub(crate) fn reply_is_empty(&self) -> bool {
        matches!(
            self.reply_at.and_then(|at| self.messages.get(at)),
            Some(message) if message.role == Role::Assistant && message.content.is_empty()
        )
    }

    /// Drop the streaming placeholder if nothing ever arrived in it. Partial
    /// text is deliberately kept — a reply cut short is still worth reading.
    ///
    /// Removing it shifts everything a typed tool appended after it, so this is
    /// the one place indices move; `reply_at` is cleared either way, since after
    /// this there is no placeholder left to append to.
    pub(crate) fn pop_empty_placeholder(&mut self) {
        if let (true, Some(at)) = (self.reply_is_empty(), self.reply_at) {
            self.messages.remove(at);
        }
        self.reply_at = None;
    }

    pub(crate) fn cancel_stream(&mut self) -> bool {
        let Some(stream_id) = self.active_stream_id() else {
            return false;
        };
        cancel_web_request(&stream_id);
        self.pop_empty_placeholder();
        self.messages.push(Message::notice("cancelled"));
        self.log("notice", serde_json::json!({"content": "cancelled"}));
        self.log(
            "cancel",
            serde_json::json!({"stream_id": stream_id.clone()}),
        );
        self.job_auto_confirm = false;
        // Not dropped: the host still owes exactly one `done: true` chunk, and
        // `finishing` is what keeps it from being recorded as a second outcome.
        self.stream = Some(StreamState {
            id: stream_id,
            finishing: true,
        });
        true
    }

    /// Unloading a plugin does not cancel its in-flight streams host-side, so
    /// the shutdown path has to.
    pub(crate) fn cancel_in_flight(&mut self) {
        if let Some(stream_id) = self.active_stream_id() {
            cancel_web_request(stream_id);
        }
    }

    /// Start over. The chat being left is *kept* in the archive — detaching from
    /// its file is all this does, so the next save opens a new one.
    pub(crate) fn new_chat(&mut self) -> bool {
        let old_id = self.chat_id;
        self.log_global("new_chat", serde_json::json!({"old_chat_id": old_id}));
        self.cancel_in_flight();
        self.status = Status::Idle;
        self.stream = None;
        self.reply_at = None;
        self.messages.clear();
        self.chat_id = None;
        self.saved = self.fingerprint();
        self.picker = None;
        self.input.clear();
        self.sse = SseParser::default();
        self.tool_calls = ToolCallAccumulator::default();
        self.pending.clear();
        self.tool_rounds = 0;
        self.owes_reply = false;
        self.manual_head = false;
        self.job_auto_confirm = false;
        self.pin_to_bottom();
        // A watcher armed for the conversation being thrown away has nothing
        // left to compare against, and would otherwise keep billing.
        self.jobs.clear();
        true
    }

    /// Scroll toward older messages, for the client that asked. Clamped at
    /// render against the real line count, which the key handler doesn't know.
    pub(crate) fn scroll_up(&mut self, lines: usize) -> bool {
        *self.scroll.entry(self.client).or_default() += lines;
        true
    }

    pub(crate) fn scroll_down(&mut self, lines: usize) -> bool {
        let offset = self.scroll.entry(self.client).or_default();
        *offset = offset.saturating_sub(lines);
        true
    }

    pub(crate) fn log(&mut self, kind: &str, payload: serde_json::Value) {
        if self.chat_id.is_none() && !self.messages.is_empty() {
            if let Some((id, json)) = history::save(&mut self.chat_id, &self.messages) {
                self.saved = self.fingerprint();
                post_save(id, json);
            }
        }
        self.log_seq = self.log_seq.wrapping_add(1);
        post_log(self.chat_id, self.mono_ms, self.log_seq, kind, payload);
    }

    pub(crate) fn log_global(&mut self, kind: &str, payload: serde_json::Value) {
        self.log_seq = self.log_seq.wrapping_add(1);
        post_log(self.chat_id, self.mono_ms, self.log_seq, kind, payload);
    }
}
