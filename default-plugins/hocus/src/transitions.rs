use crate::message::{Message, Reasoning, ReasoningKey, Role, Status, StreamState};
use crate::*;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

impl State {
    /// The data store.
    ///
    /// The shims inside a plugin; the real engine over a temporary directory
    /// everywhere else, so that a test exercises the same SQLite a session does
    /// rather than a fake. Nothing outside this pair of methods names either.
    #[cfg(target_family = "wasm")]
    pub(crate) fn store(&self) -> impl Backend + '_ {
        hocus_proto::store::Host::shared()
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn store(&self) -> impl Backend + '_ {
        &self.store
    }

    /// hocus's own namespace: the archive and the up-arrow history.
    ///
    /// Private because nothing else reads them, which also means nothing has to
    /// agree with anyone about their schema. Same database as [`State::store`],
    /// different partition of it.
    #[cfg(target_family = "wasm")]
    pub(crate) fn private_store(&self) -> impl Backend + '_ {
        hocus_proto::store::Host::private()
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn private_store(&self) -> impl Backend + '_ {
        self.store.joined("plugin:hocus", "hocus")
    }

    /// Hand the host the tab names worked out in `refresh_tab_project_labels`.
    ///
    /// Gated on the permission having come back granted: the call is refused
    /// before that, and the next poll makes it again anyway.
    #[cfg(target_family = "wasm")]
    pub(crate) fn publish_tab_projects(&self) {
        if !self.granted {
            return;
        }
        set_tab_projects(
            self.tab_project_labels
                .iter()
                .map(|(tab_id, project)| (*tab_id as u64, project.clone()))
                .collect(),
        );
    }

    /// Outside a plugin there is no host to tell, and `tab_project_labels` is
    /// what the tests read instead.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn publish_tab_projects(&self) {}

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
            spotlight: self.spotlight.take(),
            tool_rounds: std::mem::take(&mut self.tool_rounds),
            owes_reply: std::mem::take(&mut self.owes_reply),
            manual_head: std::mem::take(&mut self.manual_head),
            chat_id: self.chat_id.take(),
            saved: std::mem::take(&mut self.saved),
            view: std::mem::take(&mut self.view),
            job_auto_confirm: std::mem::take(&mut self.job_auto_confirm),
            plan: std::mem::take(&mut self.plan),
            plugin_load: self.plugin_load.take(),
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
        self.spotlight = chat.spotlight;
        self.tool_rounds = chat.tool_rounds;
        self.owes_reply = chat.owes_reply;
        self.manual_head = chat.manual_head;
        self.chat_id = chat.chat_id;
        self.saved = chat.saved;
        self.view = chat.view;
        self.job_auto_confirm = chat.job_auto_confirm;
        self.plan = chat.plan;
        self.plugin_load = chat.plugin_load;
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

    /// The call awaiting `y`/`n` as the block that describes it, if any. Rendered
    /// so the prompt names the one call it is about rather than leaving the user
    /// to infer it, and says what that call will do rather than handing over the
    /// model's own JSON to read.
    pub fn confirmation_prompt(&self) -> Option<Vec<String>> {
        // A plan waiting on an answer renders through the same tail as a tool
        // does — the wrapping, the blank line above it, and the anchor
        // suppression that keeps it from being scrolled off the bottom
        // unanswered are all things it needs for the same reasons.
        if self.awaiting_plan() {
            return Some(vec![
                "plan ready  /execute carries it out  <n> keep planning".to_owned(),
            ]);
        }
        match self.status {
            Status::ConfirmingTool => self
                .pending
                .front()
                .map(|call| self.confirmation_detail(call)),
            _ => None,
        }
    }

    /// Whether a plan is waiting to be executed or refined.
    ///
    /// Deliberately *not* folded into `is_in_flight`: nothing is arriving and no
    /// tool is running, so a question typed at the plan must go out as the next
    /// planning turn rather than pile up in `asks`.
    pub(crate) fn awaiting_plan(&self) -> bool {
        matches!(self.plan, Plan::AwaitingApproval)
    }

    /// Whether the read-only tool set is what goes out with the next request.
    ///
    /// False while a plan waits on its answer — nothing goes out at all then —
    /// which is why the UI asks `plan_mode_on` instead.
    pub(crate) fn is_planning(&self) -> bool {
        matches!(self.plan, Plan::Planning)
    }

    /// Whether planning mode is on at all, in either of its two states. What the
    /// title and the prompt ask, since neither cares how far along it is.
    pub(crate) fn plan_mode_on(&self) -> bool {
        !matches!(self.plan, Plan::Off)
    }

    /// Whether a `y`/`n` is outstanding, for the callers that only need to know
    /// that much and shouldn't pay to build the block to find out.
    pub(crate) fn is_confirming(&self) -> bool {
        matches!(self.status, Status::ConfirmingTool)
    }

    /// Whether we are waiting on something *and* nothing is arriving — the one
    /// predicate the whole waiting indicator keys off.
    pub(crate) fn is_loading(&self) -> bool {
        match self.status {
            Status::AwaitingCommand
            | Status::AwaitingSearch
            | Status::AwaitingLinear
            | Status::AwaitingExtension
            | Status::AwaitingPluginLoad => true,
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
        } else if matches!(self.status, Status::AwaitingLinear) {
            // The tool rather than the query, the way the Slack arm does it:
            // `linear_issues` often has no query worth showing.
            if let Some(call) = head {
                return Some(format!("{spinner} linear: {}", call.name));
            }
        } else if matches!(self.status, Status::AwaitingExtension) {
            if let Some(call) = head {
                return Some(format!("{spinner} extension: {}", call.name));
            }
        } else if matches!(self.status, Status::AwaitingPluginLoad) {
            return Some(format!("{spinner} loading plugin"));
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
    /// Subscribe to the daemon, and stop expecting its old numbering.
    ///
    /// Only for the moments we genuinely suspect a restarted daemon: our own
    /// load, its `AGENT_HELLO`, and the permission grant that follows. A daemon
    /// that was reloaded restarts its `seq` at one, and holding the old
    /// high-water mark would drop every snapshot it ever sent again.
    ///
    /// Anything that merely has fresh overrides to send wants `send_manual_kinds`
    /// instead. Resetting the watermark mid-stream is not free: it re-admits the
    /// snapshots already in flight, and an older one still holding a pane that
    /// has since closed will put it back on screen.
    pub(crate) fn subscribe_to_agentd(&mut self) {
        self.agent_seq = 0;
        self.send_manual_kinds();
    }

    /// Tell the daemon what the manual kinds are now. Same pipe, same reply — but
    /// no claim that the numbering restarted, because it did not.
    pub(crate) fn send_manual_kinds(&mut self) {
        let Ok(payload) = serde_json::to_string(&PollRequest {
            manual_kinds: self.manual_kinds.clone(),
            activity: self.activity.clone(),
        }) else {
            return;
        };
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE).with_payload(payload));
    }

    /// Declare the shared entities and start watching the project rows.
    ///
    /// The watch hands back the rows it matched at the moment it was registered,
    /// so the registry is in hand here rather than after a round trip — hocus
    /// used to start empty and wait for the store's first push.
    ///
    /// A failure is not fatal and not reported: it means the data store is
    /// unreachable, which for a builtin means something is very wrong, and there
    /// is no conversation on screen yet to say so in. Every read that follows
    /// simply answers empty, and every write says why it failed.
    pub(crate) fn open_store(&mut self) {
        // The archive and the up-arrow history, in hocus's own namespace: only
        // this plugin reads them, so nothing has to be coordinated. This is also
        // where the old JSON files are read in, once.
        let prompts = history::open(&self.private_store())
            .is_ok()
            .then(|| history::load_prompts(&self.private_store()));
        if let Some(prompts) = prompts {
            self.input.set_history(prompts);
        }
        // Declared here as well as in hocus-store, because either may start
        // first and a fetch against an undeclared entity is an error.
        if self.store().define_all(&store::shared_defs()).is_err() {
            return;
        }
        // `Rows` rather than `Ids`: the registry is small, and paging for the
        // rows after every change would be a second call for nothing. The limit
        // is explicit because the default hundred would silently lose the rest.
        let handle = self.store().watch(
            &FetchRequest::entity("project").limit(store::LIST_FETCH_LIMIT),
            WatchMode::Rows,
        );
        let Ok(handle) = handle else {
            return;
        };
        self.project_watch = handle.watch_id;
        self.take_projects(projects::from_rows(&handle.rows));
    }

    /// Take a new copy of the project registry.
    ///
    /// Nothing here compares versions. The store re-runs a watch on every write
    /// and reports only when the rows actually differ, so an idempotent write is
    /// silent — which is what the snapshot watermark was approximating.
    pub(crate) fn on_projects_changed(&mut self, watch_id: u64, rows: &DataRows) -> bool {
        if watch_id != self.project_watch {
            return false;
        }
        self.take_projects(projects::from_rows(rows));
        true
    }

    fn take_projects(&mut self, projects: projects::Registry) {
        self.projects = projects;
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
        self.push_strip(false);
    }

    /// Apply a batch of writes to the shared namespace.
    ///
    /// hocus writes the store itself now. There is no reply to wait for and no
    /// tool call to park: an apply is one transaction that has either happened
    /// or returned an error by the time this returns.
    pub(crate) fn write_store(&mut self, mutations: Vec<Mutation>) -> Result<ApplyReport, String> {
        self.store().apply(mutations)
    }

    pub(crate) fn on_timer(&mut self, elapsed: f64) -> bool {
        self.timer_armed = false;
        self.mono_ms = self.mono_ms.saturating_add((elapsed * 1000.0) as u64);
        // Asks ahead of jobs: someone is sitting in front of the popup waiting
        // on this, and a job that comes due on the same tick loses nothing by
        // going a second later. Once an ask starts, `is_in_flight` holds the
        // job back anyway — `tick_jobs` still runs, for its countdown.
        let asked = self.drain_asks();
        let fired = self.tick_jobs(elapsed);
        let fired = self.poll_plugin_loads() || fired;
        let fired = self.poll_extension_scan() || fired;
        let frames = self.wants_frames();
        if frames {
            self.tick += 1;
        }
        let fired = asked || fired;
        // A parked chat's plugin load keeps the chain running at frame rate even when nothing on
        // screen wants frames: nothing else will wake us, and the report is due in 750ms.
        let loads = self.plugin_load_pending();
        let extension_scan = self.extension_scan_due_ms.is_some();
        if frames || loads || extension_scan || !self.jobs.is_empty() || !self.asks.is_empty() {
            self.arm_timer(if frames || loads || extension_scan {
                FRAME_SECONDS
            } else {
                JOB_SECONDS
            });
        }
        // A quiet tick changed nothing on screen; asking for a render would
        // recomposite the whole session for nothing.
        frames || fired
    }

    fn poll_extension_scan(&mut self) -> bool {
        let Some(due) = self.extension_scan_due_ms else {
            return false;
        };
        if self.mono_ms < due {
            return false;
        }
        self.extension_scan_due_ms = None;
        for (namespace, path) in &self.extension_files {
            if self.extensions.owner_for_path(path).is_none() {
                extensions::load(path, namespace);
            }
        }
        false
    }

    /// Whether any chat, foreground or parked, is waiting on a `plugin_reload`.
    fn plugin_load_pending(&self) -> bool {
        matches!(self.status, Status::AwaitingPluginLoad)
            || self
                .parked
                .iter()
                .any(|chat| matches!(chat.status, Status::AwaitingPluginLoad))
    }

    /// Poll every chat waiting on a `plugin_reload`, the foreground one and any parked with
    /// `/history` still owed their report. Returns whether any transcript changed.
    ///
    /// Parked chats are swapped in one at a time through `with_parked`, which is how a parked
    /// chat's stream is serviced too: `poll_plugin_load` and everything it calls only ever see
    /// the chat they are for. Descending order, because a chat that settles inside `with_parked`
    /// is dropped rather than re-parked and every later index shifts down with it.
    fn poll_plugin_loads(&mut self) -> bool {
        let mut fired = self.poll_plugin_load();
        let parked: Vec<usize> = self
            .parked
            .iter()
            .enumerate()
            .filter(|(_, chat)| matches!(chat.status, Status::AwaitingPluginLoad))
            .map(|(index, _)| index)
            .collect();
        for index in parked.into_iter().rev() {
            fired = self.with_parked(index, |state| state.poll_plugin_load()) || fired;
        }
        fired
    }

    /// Look at the pane a `plugin_reload` opened, once it has had time to load. Returns whether
    /// the head was answered.
    ///
    /// Runs off the frame timer rather than a timer of its own: the chain is already running at
    /// `FRAME_SECONDS` while a load is awaited — `wants_frames` for the foreground chat, and
    /// `plugin_load_pending` for a parked one — and a second chain is exactly what `timer_armed`
    /// exists to prevent.
    ///
    /// Off wasm there is no host to ask, so the look itself lives in `tools::host` and this is
    /// only the clock around it.
    fn poll_plugin_load(&mut self) -> bool {
        if !matches!(self.status, Status::AwaitingPluginLoad) {
            return false;
        }
        let Some(watch) = self.plugin_load.clone() else {
            // Nothing to watch: answer rather than spin forever.
            self.status = Status::Idle;
            self.finish_head("the reload was requested, but nothing was recorded to watch");
            self.pump_tools();
            return true;
        };
        if self.mono_ms.saturating_sub(watch.started_ms) < PLUGIN_LOAD_MS {
            return false;
        }
        let report = if watch.extension {
            format!(
                "loaded {}, but it did not register Hocus tools; call plugin_log for its error",
                watch.target
            )
        } else {
            tools::plugin_load_report(&watch.target, tools::observe_plugin(&watch.target))
        };
        self.plugin_load = None;
        self.status = Status::Idle;
        self.finish_head(report);
        self.pump_tools();
        true
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

    /// Take a question from the `hocus-ask` popup.
    ///
    /// Every path answers the popup, including the ones that turn the question
    /// away. The popup is a floating pane the user is looking at while this
    /// runs, and it cannot tell a hocus that heard and said nothing from a
    /// session with no hocus in it at all — silence is the one reply that means
    /// something else.
    pub(crate) fn on_ask(&mut self, message: &PipeMessage) -> bool {
        let reply_to = match message.source {
            PipeSource::Plugin(id) => Some(id),
            // A hand-typed `zellij pipe` has no plugin to answer. The question
            // is still taken; there is just nobody to tell.
            _ => None,
        };
        let text = message
            .payload
            .as_deref()
            .and_then(|json| serde_json::from_str::<wire::Ask>(json).ok())
            .map(|ask| ask.text.trim().to_owned())
            .unwrap_or_default();

        if text.is_empty() {
            self.ack_ask(
                reply_to,
                wire::AskOutcome::Refused,
                "the question was empty",
            );
            return false;
        }
        if self.cfg.is_err() {
            // Queueing would be a promise nothing can keep: no configuration
            // means no request, now or on any later tick.
            self.ack_ask(
                reply_to,
                wire::AskOutcome::Refused,
                "hocus has no working API configuration",
            );
            return false;
        }
        if self.asks.len() >= MAX_QUEUED_ASKS {
            self.ack_ask(
                reply_to,
                wire::AskOutcome::Refused,
                "hocus already has a queue of questions waiting",
            );
            return false;
        }

        self.asks.push_back(text);
        let started = self.drain_asks();
        self.ack_ask(
            reply_to,
            if started {
                wire::AskOutcome::Sent
            } else {
                wire::AskOutcome::Queued
            },
            "",
        );
        // A question that could not start needs a tick to retry on, and an idle
        // hocus has no timer chain running to give it one.
        if !self.asks.is_empty() {
            self.arm_timer(FRAME_SECONDS);
        }
        started
    }

    /// Tell the popup what became of its question.
    ///
    /// Addressed by plugin id rather than broadcast, unlike everything else
    /// hocus sends: the popup is one of possibly several, and an acknowledgement
    /// delivered to the wrong one would close a box the user is still typing in.
    fn ack_ask(&self, reply_to: Option<u32>, outcome: wire::AskOutcome, detail: &str) {
        let Some(id) = reply_to else {
            return;
        };
        let ack = wire::AskAck {
            outcome,
            detail: detail.to_owned(),
        };
        if let Ok(payload) = serde_json::to_string(&ack) {
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::ASK_ACK)
                    .with_destination_plugin_id(id)
                    .with_payload(payload),
            );
        }
    }

    /// Start at most one queued question. Returns whether the transcript changed.
    ///
    /// The same guards as `tick_jobs`, for the same reasons: there is exactly
    /// one stream slot, and an open picker owns the keyboard.
    pub(crate) fn drain_asks(&mut self) -> bool {
        if self.is_in_flight() || self.picker.is_some() {
            return false;
        }
        let Ok(cfg) = self.cfg.clone() else {
            return false;
        };
        self.start_queued_question(&cfg)
    }

    /// Send the oldest accepted question. The timer calls this after a plain
    /// reply; the tool loop calls it after every call in a batch has a result.
    pub(crate) fn start_queued_question(&mut self, cfg: &Config) -> bool {
        let Some(mut text) = self.asks.pop_front() else {
            return false;
        };

        // Mentions resolve here rather than where the question was typed, the
        // same as a scheduled job's do: "@codex is stuck" should read the agent
        // as it is when the question goes out. The popup holds no snapshot of
        // its own to resolve them against anyway.
        if let Some(context) =
            agents::mention_context(&text, &self.panes, &self.manual_kinds, &self.activity)
        {
            text.push_str("\n\n");
            text.push_str(&context);
        }

        let content = text.clone();
        self.job_auto_confirm = false;
        // No `[scheduled]`-style prefix, deliberately. A job's prefix exists
        // because the model is being handed words nobody typed; an ask *is* the
        // user's words, and marking them up would change what the model reads.
        // The provenance lives in the log line instead.
        self.messages.push(Message::user(text));
        self.log(
            "user_input",
            serde_json::json!({
                "content": log::truncate(&content),
                "via": "queue",
            }),
        );
        // A fresh tool budget, same as a typed question.
        self.tool_rounds = 0;
        self.start_request(cfg);
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

    /// Grow the arriving reply's chain-of-thought.
    ///
    /// Unlike `append_to_reply` this does *not* start a fresh message when the
    /// placeholder is gone: thinking with nothing to attach it to would become
    /// a visible assistant message with no text in it, and it is only ever kept
    /// to be handed back with the tool calls it preceded.
    ///
    /// The key comes from the first fragment and is left alone after that — one
    /// endpoint does not change vocabulary mid-stream.
    pub(crate) fn append_to_reasoning(&mut self, text: &str, key: ReasoningKey) {
        if let Some(message) = self.reply() {
            message
                .reasoning
                .get_or_insert_with(|| Reasoning::new(String::new(), key))
                .text
                .push_str(text);
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
    ///
    /// The viewport's anchor is an index into the same list and goes with it,
    /// or it would slide onto whichever message took the placeholder's place.
    /// Note a reply that did arrive is *not* removed, so cancelling one part way
    /// leaves the anchor holding its first line on screen — a reply cut short is
    /// still worth reading.
    pub(crate) fn pop_empty_placeholder(&mut self) {
        if let (true, Some(at)) = (self.reply_is_empty(), self.reply_at) {
            self.messages.remove(at);
            self.view.anchor = None;
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
        // Throwing the conversation away answers its confirmation as surely as
        // an `n` does, so the pane it had singled out stops being singled out.
        self.drop_spotlight();
        self.pending.clear();
        self.tool_rounds = 0;
        self.owes_reply = false;
        self.manual_head = false;
        self.job_auto_confirm = false;
        self.plan = Plan::Off;
        self.pin_to_bottom();
        // A watcher armed for the conversation being thrown away has nothing
        // left to compare against, and would otherwise keep billing.
        self.jobs.clear();
        true
    }

    /// Scroll toward older messages, for the client that asked. Clamped at
    /// render against the real line count, which the key handler doesn't know.
    pub(crate) fn scroll_up(&mut self, lines: usize) -> bool {
        self.release();
        *self.view.scroll.entry(self.client).or_default() += lines;
        true
    }

    pub(crate) fn scroll_down(&mut self, lines: usize) -> bool {
        self.release();
        let offset = self.view.scroll.entry(self.client).or_default();
        *offset = offset.saturating_sub(lines);
        true
    }

    /// Hand the viewport to the client that just scrolled: whatever the arriving
    /// reply is doing, it stops being the thing that decides where they are
    /// looking. Until the next `pin_to_bottom`, which is the next time the
    /// conversation changes under them.
    fn release(&mut self) {
        self.view.released.insert(self.client);
    }

    pub(crate) fn log(&mut self, kind: &str, payload: serde_json::Value) {
        // A log line is filed under a chat id, so an unsaved chat needs one
        // before it can be logged against. Saving is what allocates it.
        if self.chat_id.is_none() && !self.messages.is_empty() {
            self.save_now(true);
        }
        self.log_seq = self.log_seq.wrapping_add(1);
        post_log(self.chat_id, self.mono_ms, self.log_seq, kind, payload);
    }

    pub(crate) fn log_global(&mut self, kind: &str, payload: serde_json::Value) {
        self.log_seq = self.log_seq.wrapping_add(1);
        post_log(self.chat_id, self.mono_ms, self.log_seq, kind, payload);
    }
}
