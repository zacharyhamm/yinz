use crate::message::{
    Ask, Job, Message, Reasoning, ReasoningKey, Retry, Role, ScheduledRun, Status, UserOrigin,
};
use crate::*;
use hocus_proto::jobs;
use hocus_proto::llm::{api_error_message, retry_delay, streamed_retry_delay};
use std::collections::BTreeMap;
use yinz_tile::prelude::DataWatchId;
use yinz_tile::prelude::*;

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

/// Which of the two things is waiting on an answer.
///
/// They share a renderer but not a body: a tool confirmation carries its own
/// lines, while a plan's body is the assistant message the box is drawn around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Plan,
    Tool,
}

/// Everything the pane needs to draw the box that is waiting on an answer.
///
/// A struct rather than the lines themselves, because the renderer decides how
/// the parts are painted — the title on the top border, the keys along the
/// bottom one, and only the flagged detail line in the theme's error colour.
#[derive(Debug, Clone)]
pub struct Confirmation {
    pub kind: PromptKind,
    /// What the box is called: the tool's name, or `PLAN`.
    pub title: String,
    /// The body, one line each, flagged `true` for the line that says what the
    /// call will do. Empty for a plan.
    pub detail: Vec<(String, bool)>,
    /// `(key, what it does)`, drawn along the bottom border.
    pub keys: Vec<(&'static str, &'static str)>,
}

/// What `tick_jobs` says when a job fires: whether it asked to be seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fired {
    pub(crate) show: bool,
}

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

    /// Who this hocus writes rows as: its machine, and the session it is in.
    ///
    /// The session name arrives with the daemon's first snapshot, so this is not `is_known`
    /// until then — which is why the per-pane reads and writes hold back until it is.
    pub(crate) fn owner(&self) -> agents::rows::Owner {
        agents::rows::Owner::new(self.host_id.clone(), self.yinz_session_name.clone())
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

    pub(crate) fn active_stream_id(&self) -> Option<String> {
        self.stream.as_ref().map(|stream| stream.id.clone())
    }

    /// Lift the foreground conversation out, leaving an empty one behind.
    ///
    /// The engine's state only: the input box is not touched, and the render
    /// caches go with the chat rather than being dropped. `Chat::draft` comes
    /// back empty — `swap_foreground` is what fills it, for the switches a
    /// person can see.
    pub(crate) fn take_chat(&mut self) -> Chat {
        Chat {
            transcript_cache: self.transcript_cache.take(),
            plan_cache: self.plan_cache.take(),
            usage: self.usage.take(),
            last_request_input: self.last_request_input.take(),
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
            chat_revision: self.chat_revision.take(),
            saved: std::mem::take(&mut self.saved),
            view: std::mem::take(&mut self.view),
            scheduled_run: self.scheduled_run.take(),
            retry: self.retry.take(),
            plan: std::mem::take(&mut self.plan),
            plugin_load: self.plugin_load.take(),
            extension_call_started_ms: self.extension_call_started_ms.take(),
            workflow_call: self.workflow_call.take(),
            draft: (String::new(), 0),
            jobs: std::mem::take(&mut self.jobs),
            asks: std::mem::take(&mut self.asks),
        }
    }

    /// Put one in the foreground. Whatever was there is overwritten, so this is
    /// always paired with a `take_chat`. The chat's `draft` is ignored here for
    /// the reason `take_chat` leaves it empty.
    pub(crate) fn put_chat(&mut self, chat: Chat) {
        self.transcript_cache = chat.transcript_cache;
        self.plan_cache = chat.plan_cache;
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
        self.chat_revision = chat.chat_revision;
        self.saved = chat.saved;
        self.view = chat.view;
        self.scheduled_run = chat.scheduled_run;
        self.retry = chat.retry;
        self.plan = chat.plan;
        self.usage = chat.usage;
        self.last_request_input = chat.last_request_input;
        self.plugin_load = chat.plugin_load;
        self.extension_call_started_ms = chat.extension_call_started_ms;
        self.workflow_call = chat.workflow_call;
        self.jobs = chat.jobs;
        self.asks = chat.asks;
    }

    /// Put `incoming` on screen and hand back what was there, drafts included:
    /// the outgoing chat keeps what was typed at it, and the incoming one's
    /// draft goes back into the box. The viewport is pinned to the tail, since
    /// the conversation changed for everyone looking.
    ///
    /// This is the swap for a switch a person asked for; `with_parked` is the
    /// one for servicing a chat off-screen, and it leaves the box alone.
    pub(crate) fn swap_foreground(&mut self, mut incoming: Chat) -> Chat {
        let mut outgoing = self.take_chat();
        outgoing.draft = (
            self.input.get_text().to_owned(),
            self.input.get_cursor_position(),
        );
        let (text, cursor) = std::mem::take(&mut incoming.draft);
        self.put_chat(incoming);
        self.input.replace_text(text, cursor);
        self.pin_to_bottom();
        outgoing
    }

    /// Run `f` against a parked chat by swapping it into the foreground for the
    /// duration and swapping the real one back afterwards.
    ///
    /// Doing it this way is what keeps `on_chunk`, `pump_tools` and the rest
    /// single-conversation code: they only ever see the chat they are for, in
    /// the one place they know to look for it. The chat goes back to the same
    /// index whatever happened to it, which is what keeps the tab bar's order
    /// — `parked` with the foreground spliced in at `chat_tab` — standing.
    ///
    /// Nothing a person is looking at moves: the input box, its undo stack and
    /// the foreground's render caches all come through untouched, so a tick
    /// that services another tab costs the one on screen nothing.
    pub(crate) fn with_parked<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let foreground = self.take_chat();
        let parked = self.parked.remove(index);
        self.put_chat(parked);

        let out = f(self);
        self.autosave();

        let background = self.take_chat();
        self.put_chat(foreground);
        self.parked.insert(index, background);
        out
    }

    /// How many chats the tab bar shows: every parked one plus the foreground.
    pub(crate) fn chat_tab_count(&self) -> usize {
        self.parked.len() + 1
    }

    /// The parked index behind a bar position, or `None` for the foreground.
    pub(crate) fn parked_at_tab(&self, tab: usize) -> Option<usize> {
        use std::cmp::Ordering;
        match tab.cmp(&self.chat_tab) {
            Ordering::Less => Some(tab),
            Ordering::Equal => None,
            Ordering::Greater => Some(tab - 1),
        }
    }

    /// The bar position of a parked chat.
    pub(crate) fn tab_of_parked(&self, index: usize) -> usize {
        if index < self.chat_tab {
            index
        } else {
            index + 1
        }
    }

    /// Whether any chat, on screen or parked, has a job counting down.
    pub(crate) fn any_jobs(&self) -> bool {
        self.job_count() > 0
    }

    /// Jobs across every open chat, the foreground's first. Held jobs are not
    /// counted: nothing is counting them down.
    pub(crate) fn job_count(&self) -> usize {
        self.jobs.len()
            + self
                .parked
                .iter()
                .map(|chat| chat.jobs.len())
                .sum::<usize>()
    }

    /// Every job hocus knows of: the open chats' and the held ones'.
    pub(crate) fn total_job_count(&self) -> usize {
        self.job_count() + self.held_jobs.values().map(Vec::len).sum::<usize>()
    }

    /// Whether `id` is a job in any chat, open or held - what stops a restore
    /// from reading the same row in twice.
    fn knows_job(&self, id: u64) -> bool {
        self.jobs.iter().any(|job| job.id == id)
            || self
                .parked
                .iter()
                .any(|chat| chat.jobs.iter().any(|job| job.id == id))
            || self
                .held_jobs
                .values()
                .any(|jobs| jobs.iter().any(|job| job.id == id))
    }

    /// Every chat's job list, the foreground's first, then the held ones, for
    /// the operations that reach across tabs: `unschedule` and `/stop_jobs`.
    pub(crate) fn all_jobs_mut(&mut self) -> impl Iterator<Item = &mut Vec<Job>> {
        std::iter::once(&mut self.jobs)
            .chain(self.parked.iter_mut().map(|chat| &mut chat.jobs))
            .chain(self.held_jobs.values_mut())
    }

    /// The jobs held for `chat_id`, for the chat that is being reopened; the
    /// empty list when none are.
    pub(crate) fn take_held_jobs(&mut self, chat_id: u64) -> Vec<Job> {
        self.held_jobs.remove(&chat_id).unwrap_or_default()
    }

    /// Whether any chat has a question waiting for its turn.
    fn any_asks(&self) -> bool {
        !self.asks.is_empty() || self.parked.iter().any(|chat| !chat.asks.is_empty())
    }

    /// Which parked chat a response belongs to, by whichever id its context
    /// carries. `None` means the foreground's — including when it is nobody's,
    /// which the handlers already turn away on their own.
    pub(crate) fn owner_of(&self, context: &BTreeMap<String, String>) -> Option<usize> {
        if let Some(id) = context.get(crate::decisions::REVIEW_CONTEXT) {
            return self.parked.iter().position(|chat| {
                matches!(&chat.status, Status::ReviewingTool { request_id, .. } if request_id == id)
            });
        }
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

    /// Route an asynchronous test report to the chat and call that requested it.
    pub(crate) fn on_plugin_test_result(&mut self, id: &str, payload: &str) -> bool {
        if let Some(index) = self.parked_with_call(id) {
            return self.with_parked(index, |state| state.on_plugin_test_result(id, payload));
        }
        if self.status != Status::AwaitingPluginTest
            || !self.pending.front().is_some_and(|call| call.id == id)
        {
            return false;
        }
        let answer = match serde_json::from_str::<
            Result<yinz_tile::plugin_authoring::PluginTestReport, String>,
        >(payload)
        {
            Ok(Ok(report)) => serde_json::to_string(&report).unwrap_or_default(),
            Ok(Err(error)) => error,
            Err(error) => format!("Invalid plugin test response: {error}"),
        };
        self.finish_head(answer);
        self.pump_tools();
        true
    }

    /// Which parked chat is waiting on this tool call. Only its queue head can be.
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

    /// The foreground chat's answer to [`Chat::is_live`].
    ///
    /// Spelled again over `State`'s flat fields rather than shared, because the foreground chat
    /// *is* those fields - it is only a `Chat` once it is parked. Wider than `is_in_flight`:
    /// that is about a request the endpoint owes us, and this is about anything a person would
    /// be sorry to lose, plan mode and armed jobs included.
    pub(crate) fn foreground_is_live(&self) -> bool {
        self.stream.is_some()
            || !matches!(self.status, Status::Idle)
            || !matches!(self.plan, Plan::Off)
            || !self.jobs.is_empty()
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
    pub fn confirmation_prompt(&self) -> Option<Confirmation> {
        // A plan waiting on an answer renders through the same tail as a tool
        // does — the blank line above it and the anchor suppression that keeps
        // it from being scrolled off the bottom unanswered are things it needs
        // for the same reasons. What differs is where the body comes from: a
        // tool describes itself here, while a plan's body is the reply already
        // in the transcript, which the renderer lifts into the box.
        if self.awaiting_plan() {
            return Some(Confirmation {
                kind: PromptKind::Plan,
                title: "PLAN".to_owned(),
                detail: Vec::new(),
                // Short labels on purpose: the hints are written along the
                // box's bottom border, and the pane is a third of the screen
                // wide. Anything longer drops onto a line of its own.
                // `/execute --clear` is left to the help line below: the
                // border cannot fit a fourth key without dropping them all
                // onto a line of their own.
                keys: vec![("/execute", "run"), ("n", "refine"), ("Esc", "hide")],
            });
        }
        match self.status {
            Status::ConfirmingTool | Status::ReviewingTool { .. } => {
                self.pending.front().map(|call| Confirmation {
                    kind: PromptKind::Tool,
                    title: if matches!(self.status, Status::ReviewingTool { .. }) {
                        format!("{} — checking with Jev", call.name)
                    } else {
                        call.name.clone()
                    },
                    detail: self.confirmation_body(call),
                    keys: vec![("y", "run"), ("n", "skip"), ("Esc", "hide")],
                })
            },
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

    /// Where the plan is in the transcript: the last reply that was prose
    /// rather than a round of tool calls. What the frame lifts into its box
    /// and what `/execute --clear` carries into a fresh chat.
    pub(crate) fn plan_index(&self) -> Option<usize> {
        self.messages
            .iter()
            .rposition(|message| message.role == Role::Assistant && message.tool_calls.is_empty())
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
        matches!(
            self.status,
            Status::ConfirmingTool | Status::ReviewingTool { .. }
        )
    }

    /// Whether we are waiting on something — the one predicate the whole waiting
    /// indicator keys off.
    pub(crate) fn is_loading(&self) -> bool {
        match self.status {
            Status::AwaitingCommand
            | Status::AwaitingSearch
            | Status::AwaitingDecisions
            | Status::AwaitingSwamp { .. }
            | Status::ReviewingTool { .. }
            | Status::AwaitingExtension
            | Status::AwaitingPluginLoad
            | Status::AwaitingPluginTest
            | Status::AwaitingRetry => true,
            // A confirmation prompt is waiting on the user rather than on us,
            // so the only thing left to be waiting on is the reply — for as
            // long as the stream is open, not just until its first token.
            // Arriving text used to be treated as its own proof of life, but
            // it only proves the model was alive at the last delta: a reply
            // that has gone quiet mid-sentence, a long tool-call body
            // accumulating invisibly, and a tool typed mid-reply that pushed
            // the placeholder off the end all left a silent wait on screen.
            // `finishing` has already recorded its outcome and waits on
            // nothing, which is what `is_receiving` excludes.
            Status::Idle => self.is_receiving(),
            Status::ConfirmingTool => false,
        }
    }

    /// Whether the timer chain should keep running. Wider than `is_loading` in
    /// two ways: the wand splash animates with nothing outstanding at all, and a
    /// stream still arriving behind a confirmation prompt needs frames to
    /// repaint even though `is_loading` calls that a wait on the user.
    pub(crate) fn wants_frames(&self) -> bool {
        // `wand_drawing` is set by the render that drew it, so the splash keeps its own clock
        // running and nothing else has to know the rules for when it is on screen.
        self.is_loading() || self.is_receiving() || (self.wand_drawing && !self.hidden)
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
        if matches!(
            self.status,
            Status::AwaitingCommand | Status::AwaitingDecisions | Status::AwaitingSwamp { .. }
        ) {
            if let Some(call) = head {
                return Some(format!("{spinner} running: {}", call.name));
            }
        } else if matches!(self.status, Status::ReviewingTool { .. }) {
            return Some(format!("{spinner} checking tool safety with Jev"));
        } else if matches!(self.status, Status::AwaitingSearch) {
            let args = head.and_then(|call| tools::Args::parse(&call.arguments).ok());
            if let Some(query) = args.and_then(|args| args.query) {
                return Some(format!("{spinner} searching: {query}"));
            }
        } else if matches!(self.status, Status::AwaitingExtension) {
            if let Some(call) = head {
                return Some(format!("{spinner} extension: {}", call.name));
            }
        } else if matches!(self.status, Status::AwaitingPluginLoad) {
            return Some(format!("{spinner} loading plugin"));
        } else if let Some((due_ms, attempt)) = self
            .retry
            .and_then(|retry| Some((retry.due_ms?, retry.attempt)))
        {
            // The one loader phrase that counts down. Nobody watching a rate limit wants a
            // joke; they want to know that something is still going to happen and roughly
            // when, so they can decide whether to wait or to press Esc.
            //
            // Only while the wait is the thing being waited on: once the resend is out the
            // record survives to carry the attempt count, and a countdown drawn over an
            // in-flight request would sit at "retrying in 1s" for as long as it took.
            let left = due_ms.saturating_sub(self.mono_ms).div_ceil(1000).max(1);
            return Some(format!(
                "{spinner} retrying in {left}s ({}/{MAX_RETRIES})",
                attempt + 1
            ));
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
    /// outstanding can show its spinner up to `JOB_SECONDS` late — Yinz has
    /// no cancel_timeout, and a second chain would defeat `timer_armed`.
    pub(crate) fn arm_timer(&mut self, interval: f64) {
        if !self.timer_armed {
            set_timeout(yinz_tile::prelude::FractionalSeconds::new(interval));
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
    /// Anything that merely has fresh overrides to send wants `send_overrides`
    /// instead. Resetting the watermark mid-stream is not free: it re-admits the
    /// snapshots already in flight, and an older one still holding a pane that
    /// has since closed will put it back on screen.
    pub(crate) fn subscribe_to_agentd(&mut self) {
        self.agent_seq = 0;
        self.send_overrides();
    }

    /// Request a fresh daemon snapshot without resetting its sequence watermark.
    pub(crate) fn send_overrides(&mut self) {
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_SUBSCRIBE));
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
        let prompts = history::open(&self.private_store(), &self.host_id)
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
        self.restore_persisted();
        // A no-op at a cold start, where the session name has not arrived yet and the first
        // snapshot will do this instead. It is not a no-op for a reload: the state that
        // carries the name across one is the same state this is called on.
        self.restore_pane_rows();
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
        self.take_projects(
            projects::from_rows(&handle.rows),
            projects::row_ids(&handle.rows),
        );
    }

    /// Take a new copy of the project registry.
    ///
    /// Nothing here compares versions. The store re-runs a watch on every write
    /// and reports only when the rows actually differ, so an idempotent write is
    /// silent — which is what the snapshot watermark was approximating.
    pub(crate) fn on_projects_changed(&mut self, watch_id: DataWatchId, rows: &DataRows) -> bool {
        if watch_id != self.project_watch {
            return false;
        }
        self.take_projects(projects::from_rows(rows), projects::row_ids(rows));
        true
    }

    pub(crate) fn take_projects(
        &mut self,
        projects: projects::Registry,
        ids: BTreeMap<String, yinz_tile::prelude::DataRowId>,
    ) {
        // Compare row identities too: a forgotten name may already have been recreated.
        // ponytail: scan replacement histories; index IDs if large registries slow watches.
        let replacements: BTreeMap<_, _> = self
            .projects
            .keys()
            .filter(|name| {
                !projects.contains_key(*name)
                    || self
                        .project_ids
                        .get(*name)
                        .is_some_and(|id| ids.get(*name) != Some(id))
            })
            .map(|name| {
                let replacement = self.project_ids.get(name).and_then(|id| {
                    let mut matches = projects
                        .values()
                        .filter(|project| project.replaced_ids.contains(id));
                    let project = matches.next()?;
                    matches.next().is_none().then(|| project.name.clone())
                });
                (name.clone(), replacement)
            })
            .collect();
        for assignments in [&mut self.tab_projects, &mut self.auto_tab_projects] {
            assignments.retain(|_, name| match replacements.get(name) {
                Some(Some(replacement)) => {
                    name.clone_from(replacement);
                    true
                },
                Some(None) => false,
                None => projects.contains_key(name),
            });
        }
        if let Some(replacement) = self
            .active_project
            .as_ref()
            .and_then(|name| replacements.get(name))
        {
            self.active_project = replacement.clone();
        }
        self.projects = projects;
        self.project_ids = ids;
        self.refresh_tab_projects();
        self.push_strip(false);
    }

    /// Apply a batch of writes to the shared namespace.
    ///
    /// hocus writes the store itself now. There is no reply to wait for and no
    /// tool call to park: an apply is one transaction that has either happened
    /// or returned an error by the time this returns.
    /// The error is typed: a conflict means somebody wrote a guarded row first and the batch
    /// was rolled back, which a caller can answer by reading again rather than by giving up.
    pub(crate) fn write_store(
        &mut self,
        mutations: Vec<Mutation>,
    ) -> Result<ApplyReport, WriteError> {
        self.store().apply(mutations)
    }

    /// A write whose failure is worth a log line and nothing more: the state
    /// it mirrors is still in memory, and the next write will try again.
    fn write_store_quietly(&mut self, what: &str, mutations: Vec<Mutation>) {
        if mutations.is_empty() {
            return;
        }
        if let Err(error) = self.write_store(mutations) {
            self.log_global(
                "store_error",
                serde_json::json!({"what": what, "error": log::truncate(error.message())}),
            );
        }
    }

    /// Take back what the last hocus wrote: its scheduled jobs, the kinds the
    /// user set by hand, and the daemon's last summaries. Read once, at load;
    /// hocus is the only writer of these rows, so nothing watches them.
    ///
    /// A job follows the chat that armed it. A restart opens no chat, so a job
    /// whose row names one is *held* under that id until `/history` reopens
    /// it - see `held_jobs` - and only a job armed in a chat that was never
    /// saved, which no archive can bring back, lands in the foreground. Its
    /// countdown resumes from where the wall clock says it is.
    pub(crate) fn restore_persisted(&mut self) {
        let now = now_secs();
        // Scoped to this machine, unlike the reads below it. A job is a prompt that will be
        // typed into a session, so restoring another machine's would fire its work here. The
        // session name has not arrived yet at load, which is why this is the machine and not
        // the full owner — see `jobs::job_uid`.
        let fetched = self
            .store()
            .fetch(&jobs::owned_request(&self.owner()).limit(store::LIST_FETCH_LIMIT))
            .ok();
        if let Some(rows) = fetched {
            let mut restored = 0;
            let mut held = 0;
            for row in jobs::from_rows(&rows) {
                if self.knows_job(row.job_id) {
                    continue;
                }
                self.job_seq = self.job_seq.max(row.job_id);
                let job = Job {
                    id: row.job_id,
                    every: row.every,
                    due_in: (row.next_due_at - now).max(0) as f64,
                    prompt: row.prompt,
                    show: row.show,
                    auto_confirm: row.auto_confirm,
                };
                match row.chat_id {
                    Some(chat_id) if self.chat_id != Some(chat_id) => {
                        if let Some(index) = self
                            .parked
                            .iter()
                            .position(|chat| chat.chat_id == Some(chat_id))
                        {
                            self.parked[index].jobs.push(job);
                            restored += 1;
                        } else {
                            self.held_jobs.entry(chat_id).or_default().push(job);
                            held += 1;
                        }
                    },
                    _ => {
                        self.jobs.push(job);
                        restored += 1;
                    },
                }
            }
            if restored > 0 {
                self.arm_timer(JOB_SECONDS);
                self.messages.push(Message::notice(format!(
                    "restored {restored} scheduled job(s)"
                )));
            }
            if held > 0 {
                self.messages.push(Message::notice(format!(
                    "{held} scheduled job(s) wait in archived chats: /history reopens one with its jobs"
                )));
            }
        }
        self.load_more_agent_runs();
    }

    /// The other half of the restore: the rows that are about *panes*.
    ///
    /// Split off and deferred because they cannot be read at load. Every session on the
    /// machine shares one store file, so a latch or a summary is keyed by session as well as
    /// pane id - and hocus learns its session name from the daemon's first snapshot, which has
    /// not arrived yet when `restore_persisted` runs. Reading them unscoped would take another
    /// session's rows.
    ///
    /// Runs once, on that first snapshot. Same rule as the rows themselves: hocus is the only
    /// writer, so nothing watches them and one read is enough.
    pub(crate) fn restore_pane_rows(&mut self) {
        let owner = self.owner();
        if !owner.is_known() || self.restored_pane_rows {
            return;
        }
        self.restored_pane_rows = true;
        let fetch = |entity: &str| {
            agents::rows::owned_rows_request(entity, &owner).limit(store::LIST_FETCH_LIMIT)
        };
        let fetched = self
            .store()
            .fetch(&fetch(agents::rows::SUMMARY_ENTITY))
            .ok();
        if let Some(rows) = fetched {
            let summaries = agents::rows::summaries_from_rows(&rows);
            if !summaries.is_empty() {
                self.summaries = summaries;
            }
        }
        // The rows the pane-id-only version of these entities left behind. Unreachable by any
        // predicate a current row matches, so they would otherwise sit there for good.
        self.write_store_quietly(
            "agent_rows_migration",
            agents::rows::stale_unowned_delete_mutations(),
        );
    }

    pub(crate) fn load_more_agent_runs(&mut self) {
        const PAGE: u32 = 50;
        let Ok(rows) = self.store().fetch(
            &FetchRequest::entity(agents::rows::RUN_ENTITY)
                .filter(Pred::eq("active", false))
                .filter(Pred::not_null("header"))
                .sort_by("started_at_ms", SortDir::Desc)
                .sort_by("uid", SortDir::Asc)
                .select(["header"])
                .limit(PAGE)
                .offset(self.archive_offset),
        ) else {
            return;
        };
        let count = rows.len() as u32;
        let headers = agents::rows::run_headers_from_rows(&rows);
        if let Some(last) = headers.last() {
            self.archive_loaded_through = Some((last.started_at_ms, last.id.clone()));
        }
        for header in headers {
            if !self.archived_runs.iter().any(|run| run.id == header.id) {
                self.archived_runs.push(header);
            }
        }
        self.archive_offset = self.archive_offset.saturating_add(count);
        self.archive_more = rows.truncated;
    }

    pub(crate) fn send_run_detail(
        &self,
        plugin_id: yinz_tile::prelude::PluginId,
        run_id: &str,
        revision: Option<u64>,
    ) {
        pipe_message_to_plugin(self.run_detail_message(plugin_id, run_id, revision));
    }

    /// The answer to the panel asking for a run's history: the run as the payload, or - with no
    /// payload - an `error` arg saying why there is none, beside a `run_id` arg naming the run
    /// asked for. `revision` is echoed either way, as the panel matches replies on it.
    ///
    /// Answered either way for the reason `resume_run` is: a request that is never answered
    /// leaves the history half on its placeholder for good, and the user cannot tell a run still
    /// loading from one that never will. A failure is sent again on each snapshot, like the
    /// request that produced it, so a store that recovers replaces it with the run.
    pub(crate) fn run_detail_message(
        &self,
        plugin_id: yinz_tile::prelude::PluginId,
        run_id: &str,
        revision: Option<u64>,
    ) -> MessageToPlugin {
        let mut args: BTreeMap<String, String> = revision
            .map(|revision| ("revision".to_owned(), revision.to_string()))
            .into_iter()
            .collect();
        let message =
            MessageToPlugin::new(wire::AGENT_RUN_DETAIL).with_destination_plugin_id(plugin_id);
        match self.run_detail(run_id, revision) {
            Ok(payload) => message.with_args(args).with_payload(payload),
            Err(error) => {
                args.insert("run_id".to_owned(), run_id.to_owned());
                args.insert("error".to_owned(), error);
                message.with_args(args)
            },
        }
    }

    /// The selected run, serialised with the entries the panel shows.
    fn run_detail(&self, run_id: &str, revision: Option<u64>) -> Result<String, String> {
        let rows = self
            .store()
            .fetch(&agents::rows::run_by_id_request(run_id))
            .map_err(|_| "the run's record could not be read".to_owned())?;
        let mut run = agents::rows::runs_from_rows(&rows)
            .into_iter()
            .next()
            .ok_or_else(|| "the run is no longer in the archive".to_owned())?;
        // Search details keep the full match with context; ordinary details load the bounded
        // newest entries. A run stored before the split still carries its entries in its
        // payload, and keeps them when hydrating the newest entries.
        if let Some(revision) = revision {
            let page = hocus_proto::run_history::search_detail(&self.store(), run_id, revision)?;
            // Counted the way `hydrate_newest_entries` counts what it leaves out, as a lower
            // bound: the page starts a little before the match, and without the count the
            // oldest event it holds reads as the start of the run.
            run.omitted_entries = usize::from(page.earlier);
            run.entries = page.entries;
        } else {
            agents::rows::hydrate_newest_entries(&self.store(), &mut run);
        }
        serde_json::to_string(&run).map_err(|error| error.to_string())
    }

    /// Start an archived run again, resuming the session it recorded.
    ///
    /// hocus rather than the daemon, and hocus rather than the panel, for the reason every
    /// other row action is hocus's: the run's kind, session id, folder and machine are all
    /// facts this plugin already holds — `archived_runs` is the very list the rows were
    /// built from — and the panel carries nothing but the run id it was drawn with.
    ///
    /// Answered either way (`wire::AGENT_RESUME_RESULT`). The panel put a confirmation in
    /// front of this, so the user has already said yes to something; a refusal that never
    /// arrives would leave that yes looking like a key that does nothing.
    pub(crate) fn resume_run(&mut self, plugin_id: yinz_tile::prelude::PluginId, run_id: &str) {
        let answer = |detail: String| {
            let Ok(payload) = serde_json::to_string(&wire::ResumeResult {
                run_id: run_id.to_owned(),
                detail,
            }) else {
                return;
            };
            pipe_message_to_plugin(
                MessageToPlugin::new(wire::AGENT_RESUME_RESULT)
                    .with_destination_plugin_id(plugin_id)
                    .with_payload(payload),
            );
        };
        let command = match self.resume_plan(run_id) {
            Ok(command) => command,
            Err(error) => {
                answer(error);
                return;
            },
        };
        // `agent_pane_context` by hand: the host sets the marker itself only on the
        // git-checkout call, and this route has no checkout to make. Without it the pane is
        // one the host cannot tell from any other command pane, and `agent_exit` would never
        // fire for the agent in it.
        match open_command_pane_near_plugin(command, launch::agent_pane_context()) {
            Some(pane_id) => {
                // Walking to it is the point: a resumed conversation is one you carry on.
                focus_pane_with_id(pane_id, false, false);
                answer(String::new());
            },
            None => answer("the pane could not be opened".to_owned()),
        }
    }

    /// What resuming the run the panel named would run, or why it cannot be.
    ///
    /// Split from `resume_run` for the reason `launch::plan` is split from `launch::open`:
    /// everything decided here is decidable without a host, and everything after it is a
    /// host call and nothing else.
    pub(crate) fn resume_plan(&self, run_id: &str) -> Result<CommandToRun, String> {
        // A run that has gone live since the strip was drawn — someone resumed it elsewhere,
        // or a warp adopted its pane into this session. The archive row the user pressed `r`
        // on is stale by a beat, and starting a second process on the session id would be two
        // agents writing one conversation, neither aware of the other. `Enter` on the live
        // row is what they want, and it is already there.
        if let Some((pane_id, _)) = self
            .active_runs
            .iter()
            .find(|(_, header)| header.id == run_id)
        {
            return Err(format!("that run is live again in pane {pane_id}"));
        }
        let header = self.stored_run_header(run_id)?;
        // The same staleness one yinz session further out: another session sharing the
        // store resumed this run, or warped its pane in, and `adopt_stored_run` cleared the
        // record's end. `active_runs` above cannot see that - it is this session's live runs
        // - and the stored record is the only thing that can. `ended_at_ms` rather than
        // `outcome`, because `ended_at_ms.is_none()` is exactly what the `active` column the
        // archive is paged by is written from: a row with an end and a stale `Active`
        // outcome is shown in the archive, and has to stay resumable.
        if header.ended_at_ms.is_none() {
            return Err("that run is live again in another session".to_owned());
        }
        launch::resume_command(
            &header.kind,
            &header.session_id,
            header.cwd.as_deref(),
            header.host.as_deref(),
        )
    }

    /// The stored record for a run the panel named, as the store holds it now.
    ///
    /// The store rather than `archived_runs`, which is where the row itself came from: that
    /// list is a snapshot taken when the page was loaded, and a run adopted since - by
    /// another yinz session sharing this store, or by a warp - still looks ended in it.
    /// Reading it back is also what covers the panel pointing at a row this instance never
    /// loaded, which the paged archive makes real rather than defensive.
    pub(crate) fn stored_run_header(&self, run_id: &str) -> Result<agents::AgentRunHeader, String> {
        let rows = self
            .store()
            .fetch(&agents::rows::run_by_id_request(run_id))
            .map_err(|_| "that run's record could not be read".to_owned())?;
        agents::rows::runs_from_rows(&rows)
            .into_iter()
            .next()
            .map(|run| run.header())
            .ok_or_else(|| "that run is no longer in the archive".to_owned())
    }

    pub(crate) fn persist_job(&mut self, row: jobs::JobRow) {
        let owner = self.owner();
        self.write_store_quietly("job", vec![jobs::to_mutation(&owner, &row)]);
    }

    pub(crate) fn forget_jobs(&mut self, job_ids: Vec<u64>) {
        if job_ids.is_empty() {
            return;
        }
        let owner = self.owner();
        self.write_store_quietly("job", vec![jobs::delete_mutation(&owner, job_ids)]);
    }

    /// Mirror the daemon's summaries, writing only what changed.
    pub(crate) fn persist_summaries(&mut self, before: &agents::Summaries) {
        let owner = self.owner();
        if !owner.is_known() {
            return;
        }
        let mutations = agents::rows::summary_diff_mutations(&owner, before, &self.summaries);
        self.write_store_quietly("agent_summary", mutations);
    }

    pub(crate) fn on_timer(&mut self, elapsed: f64) -> bool {
        self.timer_armed = false;
        self.mono_ms = self.mono_ms.saturating_add((elapsed * 1000.0) as u64);
        // Asks ahead of jobs: someone is waiting on this one, and a job that
        // comes due on the same tick loses nothing by
        // going a second later. Once an ask starts, `is_in_flight` holds the
        // job back anyway — `tick_jobs` still runs, for its countdown.
        // The resend goes first. `Status::AwaitingRetry` holds both of the others back on its
        // own — `drain_asks` and `tick_jobs` bail on `is_in_flight` — so this only decides
        // which tick they resume on, and losing a tick to a question that was queued behind
        // this very request would be the wrong way round.
        let retried = self.poll_retry();
        let asked = self.drain_asks();
        let mut fired = match self.tick_jobs(elapsed) {
            Some(Fired { show }) => {
                if show {
                    // The panel, not this pane: `show_self` alone leaves hocus believing it is
                    // still hidden - see `State::show_panel`. And only if it is down: one that is
                    // up keeps the keyboard where the user has it.
                    self.show_panel_for_job();
                }
                true
            },
            None => false,
        };
        // The same pass for every parked chat with something to tick: its
        // queue and its jobs are its own, and it is as able to start a request
        // off-screen as it is to receive one. The others are left alone — a
        // settled tab has nothing to do, and swapping it in costs.
        let ticking: Vec<usize> = self
            .parked
            .iter()
            .enumerate()
            .filter(|(_, chat)| {
                !chat.jobs.is_empty() || !chat.asks.is_empty() || chat.retry.is_some()
            })
            .map(|(index, _)| index)
            .collect();
        // The bar position of the first chat whose job asked to be seen. A bar
        // position rather than a parked index: the switch below reorders
        // `parked`, and bar order is the one thing that never moves.
        let mut show_tab = None;
        for index in ticking {
            let (job, asked, retried) = self.with_parked(index, |state| {
                let asked = state.drain_asks();
                // Ahead of the job, and for the same reason the foreground's is: a chat still
                // owed the answer to a request it already sent is not free to start another.
                let retried = state.poll_retry();
                (state.tick_jobs(elapsed), asked, retried)
            });
            if matches!(job, Some(Fired { show: true })) && show_tab.is_none() {
                show_tab = Some(self.tab_of_parked(index));
            }
            fired = fired || asked || retried || job.is_some();
        }
        // A job that asked to be seen fired into a tab nobody is looking at.
        // Popping the pane open would show the wrong chat - the one on screen,
        // not the one the job is writing into - so the tab is switched to first,
        // and then the panel shown if it is down. Nothing here is waiting on the
        // keyboard: a scheduled run declines or auto-confirms instead of asking
        // (see `pump_tools`), so a panel that is up keeps the keys where they are.
        //
        // Except from someone typing into the box, whose chat stays on screen: a
        // switch would swap their draft out from under the next keystroke, and the
        // words after it would land in the job's chat. The job runs in its own tab
        // all the same, which the bar marks as live.
        if let Some(tab) = show_tab {
            let typing = self.own_is_focused == Some(true) && !self.input.get_text().is_empty();
            if !typing {
                self.switch_to_tab(tab);
            }
            self.show_panel_for_job();
        }
        let fired = self.poll_plugin_loads() || fired;
        let fired = self.poll_extension_calls() || fired;
        let fired = self.poll_tool_reviews() || fired;
        let fired = self.poll_extension_scan() || fired;
        let frames = self.wants_frames();
        if frames {
            self.tick += 1;
            self.wand_frame = self.wand_frame.wrapping_add(1);
        }
        let fired = asked || retried || fired;
        // A parked chat's plugin load keeps the chain running at frame rate even when nothing on
        // screen wants frames: nothing else will wake us, and the report is due in 750ms.
        // A parked chat's extension call likewise: its timeout is the only thing that will
        // answer a call nobody else does.
        let loads = self.plugin_load_pending()
            || self.extension_call_pending()
            || self.tool_review_pending();
        let extension_scan = self.extension_scan_due_ms.is_some();
        // A retry wants frames, not job-rate ticks: the countdown beside the spinner is drawn
        // every frame, and a parked chat's retry has nothing else to wake it.
        let retrying = self.retry_pending();
        if frames || loads || extension_scan || retrying || self.any_jobs() || self.any_asks() {
            self.arm_timer(if frames || loads || extension_scan || retrying {
                FRAME_SECONDS
            } else {
                JOB_SECONDS
            });
        }
        // A quiet tick changed nothing on screen; asking for a render would
        // recomposite the whole session for nothing.
        frames || fired
    }

    /// Arm a resend of the request the endpoint just turned away, or answer `false` when this
    /// one is not worth resending.
    ///
    /// `false` is the caller's signal to do what it always did: show the error and end the
    /// turn. That covers a status a resend would only repeat, a provider still saying no after
    /// `MAX_RETRIES`, and a configuration that has gone missing under us — in each case the
    /// person is better served by being told than by a spinner.
    ///
    /// Nothing is appended to the transcript. A rate limit is a statement about the size of
    /// the payload, so answering it by adding to that payload is the one thing that cannot
    /// help; the countdown lives in `loader`, which is repainted rather than stored. The only
    /// thing a retry does to the transcript is take back the placeholder the refused request
    /// was streaming into — empty here, and, on the streamed path, whatever had arrived in it.
    pub(crate) fn arm_retry(&mut self, status: u16, body: &[u8]) -> bool {
        let attempt = self.next_attempt();
        let wait = attempt.and_then(|attempt| retry_delay(status, body, attempt));
        self.arm_retry_after(attempt, wait, &api_error_message(status, body))
    }

    /// The same, for an error that arrived inside the stream rather than ahead of it. See
    /// `streamed_retry_delay` for why the bar is higher there.
    pub(crate) fn arm_streamed_retry(&mut self, message: &str) -> bool {
        let attempt = self.next_attempt();
        let wait = attempt.and_then(|attempt| streamed_retry_delay(message, attempt));
        self.arm_retry_after(attempt, wait, message)
    }

    /// Which resend this would be, or `None` when there is not going to be one: the cap is
    /// spent, or there is no configuration left to send with.
    fn next_attempt(&self) -> Option<u32> {
        let attempt = self.retry.map_or(0, |retry| retry.attempt + 1);
        (attempt < MAX_RETRIES && self.cfg.is_ok()).then_some(attempt)
    }

    fn arm_retry_after(&mut self, attempt: Option<u32>, wait: Option<f64>, detail: &str) -> bool {
        let (Some(attempt), Some(wait)) = (attempt, wait) else {
            self.retry = None;
            return false;
        };
        self.retry = Some(Retry {
            due_ms: Some(self.mono_ms.saturating_add((wait * 1000.0) as u64)),
            attempt,
        });
        // `scheduled_run` is deliberately left alone. The job that started this turn still
        // owns it, and a run that loses its authority to a rate limit comes back unable to
        // call the tools it was armed for.
        //
        // Nor is a status something else owns. A stream and a tool run can both be
        // outstanding, so a refusal can land while the queue is `AwaitingCommand` - and
        // `on_command_result` answers to that status alone, so overwriting it would drop the
        // result and leave the call unanswered. The tool is also the better thing for the
        // loader to name while both are true.
        if matches!(self.status, Status::Idle) {
            self.status = Status::AwaitingRetry;
        }
        self.log(
            "api_retry",
            serde_json::json!({
                "attempt": attempt + 1,
                "of": MAX_RETRIES,
                "wait_seconds": wait,
                "error": log::truncate(detail),
            }),
        );
        self.arm_timer(FRAME_SECONDS);
        true
    }

    /// Send the rate-limited request again once its wait is up.
    ///
    /// `retry` is left set across `start_request`, so a second refusal reaches `arm_retry`
    /// with the attempt count intact and the backoff keeps doubling. A completed successful
    /// response clears it, which is what makes the count per-outage rather than per-conversation.
    ///
    /// What is *not* left set is the due time: this runs on every frame, and a `due_ms` still
    /// in the past would send the request again 10 times a second for as long as the endpoint
    /// took to answer - each one bumping `stream_seq` and orphaning the last. Clearing it is
    /// what makes the resend happen once.
    fn poll_retry(&mut self) -> bool {
        let Some(retry) = self.retry else {
            return false;
        };
        let Some(due_ms) = retry.due_ms else {
            // Already resent; the record is only carrying the attempt count now.
            return false;
        };
        if self.mono_ms < due_ms {
            // Still counting down, but the countdown is drawn, so the frame is not wasted.
            return false;
        }
        let Ok(cfg) = self.cfg.clone() else {
            // Nothing to resend with. `arm_retry` checks the same thing, so this is only
            // reachable if the configuration was reloaded out from under a waiting retry.
            self.retry = None;
            self.status = Status::Idle;
            self.messages.push(Message::error(
                "gave up retrying: the endpoint configuration is no longer valid",
            ));
            return true;
        };
        self.retry = Some(Retry {
            due_ms: None,
            attempt: retry.attempt,
        });
        // Only the status this armed. Anything else - a tool run the refusal landed on top of
        // - still owns it and is still owed its result; `arm_retry_after` declined to take it
        // for the same reason.
        if matches!(self.status, Status::AwaitingRetry) {
            self.status = Status::Idle;
        }
        self.start_request(&cfg);
        true
    }

    /// Whether this chat's resend is still waiting on the clock, as opposed to already in
    /// flight. What the countdown, the interrupt and the frame timer all mean by "retrying".
    pub(crate) fn retry_armed(&self) -> bool {
        matches!(self.retry, Some(retry) if retry.due_ms.is_some())
    }

    /// Whether any chat, foreground or parked, is waiting out a rate limit.
    fn retry_pending(&self) -> bool {
        // A resend that is already out needs no timer of its own: it is a stream like any
        // other, and `wants_frames` keeps the chain running for it.
        self.retry_armed()
            || self
                .parked
                .iter()
                .any(|chat| matches!(chat.retry, Some(retry) if retry.due_ms.is_some()))
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

    /// Whether any chat, foreground or parked, is waiting on an extension call.
    fn extension_call_pending(&self) -> bool {
        matches!(self.status, Status::AwaitingExtension)
            || self
                .parked
                .iter()
                .any(|chat| matches!(chat.status, Status::AwaitingExtension))
    }

    /// Poll every chat waiting on an extension call, the foreground one and any parked, each
    /// against its own clock. Returns whether any was given up on. The swap is
    /// `poll_plugin_loads`'s, for the same reason.
    fn poll_extension_calls(&mut self) -> bool {
        let mut fired = self.poll_extension_call();
        let parked: Vec<usize> = self
            .parked
            .iter()
            .enumerate()
            .filter(|(_, chat)| matches!(chat.status, Status::AwaitingExtension))
            .map(|(index, _)| index)
            .collect();
        for index in parked {
            fired = self.with_parked(index, |state| state.poll_extension_call()) || fired;
        }
        fired
    }

    /// Poll every chat waiting on a `plugin_reload`, the foreground one and any parked with
    /// `/history` still owed their report. Returns whether any transcript changed.
    ///
    /// Parked chats are swapped in one at a time through `with_parked`, which is how a parked
    /// chat's stream is serviced too: `poll_plugin_load` and everything it calls only ever see
    /// the chat they are for. Indices are stable under the swap, so the order is immaterial.
    fn poll_plugin_loads(&mut self) -> bool {
        let mut fired = self.poll_plugin_load();
        let parked: Vec<usize> = self
            .parked
            .iter()
            .enumerate()
            .filter(|(_, chat)| matches!(chat.status, Status::AwaitingPluginLoad))
            .map(|(index, _)| index)
            .collect();
        for index in parked {
            fired = self.with_parked(index, |state| state.poll_plugin_load()) || fired;
        }
        fired
    }

    /// Give up on an extension call that has gone unanswered for `CALL_TIMEOUT_MS`: the head is
    /// answered with the reason and the model carries on. Returns whether it was.
    fn poll_extension_call(&mut self) -> bool {
        if !matches!(self.status, Status::AwaitingExtension) {
            return false;
        }
        let Some(started) = self.extension_call_started_ms else {
            return false;
        };
        let timeout = if self
            .pending
            .front()
            .is_some_and(|call| crate::operations::TOOLS.contains(&call.name.as_str()))
        {
            30_000
        } else if self.workflow_call.is_some() {
            120_000
        } else {
            extensions::CALL_TIMEOUT_MS
        };
        if self.mono_ms.saturating_sub(started) < timeout {
            return false;
        }
        let name = self
            .pending
            .front()
            .map(|head| head.name.clone())
            .unwrap_or_default();
        self.status = Status::Idle;
        self.extension_call_started_ms = None;
        if let Some(pending) = self.workflow_call.take() {
            self.finish_head(crate::workflows::uncertain_result(
                self.pending.front(),
                &pending.rpc_id,
                "Background service response was lost",
            ));
            self.pump_tools();
            return true;
        }
        self.finish_head(format!(
            "extension tool {name} did not answer in {}s; plugin_log its plugin for the error",
            extensions::CALL_TIMEOUT_MS / 1000
        ));
        self.pump_tools();
        true
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
        } else if watch.headless {
            tools::plugin_headless_report(
                &watch.name,
                &watch.target,
                tools::plugin_log_tail(&watch.name, 40),
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

    /// Count the jobs down and start at most one due run. Says which, if one
    /// fired — the transcript changed exactly then.
    ///
    /// Only fires when `Idle`: there is exactly one stream slot, and a job that
    /// comes due mid-reply waits for a later tick rather than stacking runs.
    ///
    /// Showing the pane is the caller's job, not this one's: a job in a parked
    /// chat has to be switched to before it is shown, and only `on_timer`
    /// knows which chat this is running against.
    ///
    /// A scheduled run that calls a confirmation-gated tool does not park on
    /// its y/n prompt - `pump_tools` declines it unless the job was armed
    /// with `auto_confirm` - so one such run cannot hold every other job.
    pub(crate) fn tick_jobs(&mut self, elapsed: f64) -> Option<Fired> {
        for job in &mut self.jobs {
            job.due_in -= elapsed;
        }
        if self.is_in_flight() || self.picker.is_some() {
            return None;
        }
        let Ok(cfg) = self.cfg.clone() else {
            return None;
        };
        let chat_id = self.chat_id;
        let now = now_secs();
        let job = self.jobs.iter_mut().find(|job| job.due_in <= 0.0)?;

        job.due_in = job.every;
        let prompt = job.prompt.clone();
        let job_id = job.id;
        let show = job.show;
        let auto_confirm = job.auto_confirm;
        let row = job_row(job, chat_id, now);
        self.scheduled_run = Some(ScheduledRun {
            job_id,
            auto_confirm,
        });
        // The next due time moved, and that is the one moment a countdown
        // reaches the store.
        self.persist_job(row);
        // Mentions resolve at fire time, so "check @codex every minute" reads the
        // agent as it is now, not as it was when the job was armed.
        self.push_user_message(format!("[scheduled] {prompt}"), None, UserOrigin::Scheduled);
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
        Some(Fired { show })
    }

    /// Take a question from the `hocus_ask` pipe.
    ///
    /// Every path answers a plugin caller, including the ones that turn the
    /// question away. Such a caller cannot tell a hocus that heard and said
    /// nothing from a session with no hocus in it at all — silence is the one
    /// reply that means something else.
    pub(crate) fn on_ask(&mut self, message: &PipeMessage) -> bool {
        let reply_to = match message.source {
            PipeSource::Plugin(id) => Some(id),
            // A hand-typed `yinz pipe` has no plugin to answer. The question
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

        self.asks.push_back(Ask::text(text, UserOrigin::Pipe));
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

    /// Tell the asking plugin what became of its question.
    ///
    /// Addressed by plugin id rather than broadcast, unlike everything else
    /// hocus sends: the asker is one of possibly several, and an acknowledgement
    /// delivered to the wrong one would answer a question it never put.
    fn ack_ask(
        &self,
        reply_to: Option<yinz_tile::prelude::PluginId>,
        outcome: wire::AskOutcome,
        detail: &str,
    ) {
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

    /// Enrich at dispatch time while keeping the original text and its provenance.
    pub(crate) fn push_user_message(
        &mut self,
        mut text: String,
        display: Option<String>,
        user_origin: UserOrigin,
    ) -> String {
        let user_content = text.clone();
        if let Some(context) = agents::mention_context(
            &text,
            &self.panes,
            &self.manual_kinds,
            &self.activity,
            &self.hooks,
        ) {
            text.push_str("\n\n");
            text.push_str(&context);
        }
        let mut message = Message::user_shown(text.clone(), display);
        message.user_origin = user_origin;
        message.user_content = Some(user_content);
        self.messages.push(message);
        text
    }

    /// Send the oldest accepted question after a reply or complete tool batch.
    pub(crate) fn start_queued_question(&mut self, cfg: &Config) -> bool {
        let Some(Ask {
            content: text,
            display,
            user_origin,
        }) = self.asks.pop_front()
        else {
            return false;
        };

        // Mentions resolve here rather than where the question was typed, the
        // same as a scheduled job's do: "@codex is stuck" should read the agent
        // as it is when the question goes out. The popup holds no snapshot of
        // its own to resolve them against anyway.
        self.scheduled_run = None;
        // Preserve provider text; provenance travels separately on the message.
        let typed = display.clone();
        let content = self.push_user_message(text, display, user_origin);
        self.log(
            "user_input",
            serde_json::json!({
                "content": log::truncate(&content),
                "display": typed,
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
        if self.reply_is_empty() {
            self.pop_placeholder();
        }
        self.reply_at = None;
    }

    /// Drop the streaming placeholder whatever is in it.
    ///
    /// Only for a resend of the same request. Partial text is worth reading when it is the
    /// last word on the matter, which is why cancelling keeps it — but a retry is about to
    /// fetch the whole answer again, and a half-sentence left in the transcript goes back out
    /// as an assistant turn the provider never finished, for the model to continue from.
    pub(crate) fn pop_placeholder(&mut self) {
        if let Some(at) = self.reply_at.filter(|at| *at < self.messages.len()) {
            self.messages.remove(at);
            // See `pop_empty_placeholder`: the anchor is an index into the list that just
            // moved, and would otherwise slide onto whatever took the placeholder's place.
            self.view.anchor = None;
        }
        self.reply_at = None;
    }

    pub(crate) fn cancel_stream(&mut self) -> bool {
        // Whichever half of a retry this interrupt found, it ends here: a wait has nothing
        // left to count down to, and a resend already out is the stream cancelled below.
        let waiting = self
            .retry
            .take()
            .is_some_and(|retry| retry.due_ms.is_some());
        // A retry waiting on the clock has no live stream to cancel - the request that failed
        // is already `finishing`, whether or not the host has delivered the terminator it owes
        // - so without this branch the interrupt would find nothing to do and the countdown
        // would go on running under it. A resend that is already out is a stream like any
        // other and takes the path below, which is what makes the notice true.
        if waiting && !self.is_receiving() {
            self.status = Status::Idle;
            self.messages.push(Message::notice("cancelled"));
            self.log("notice", serde_json::json!({"content": "cancelled"}));
            self.log("cancel", serde_json::json!({"reason": "retry"}));
            self.scheduled_run = None;
            self.pin_to_bottom();
            return true;
        }
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
        self.scheduled_run = None;
        // Not dropped: the host still owes exactly one `done: true` chunk, and
        // `finishing` is what keeps it from being recorded as a second outcome.
        if let Some(stream) = self.stream.as_mut() {
            stream.finishing = true;
        }
        true
    }

    /// Unloading a plugin does not cancel its in-flight streams host-side, so
    /// the shutdown path has to.
    pub(crate) fn cancel_in_flight(&mut self) {
        if self.status == Status::AwaitingPluginTest {
            if let Some(call) = self.pending.front() {
                yinz_tile::shim::cancel_plugin_test(&call.id);
            }
        }
        if let Some(stream_id) = self.active_stream_id() {
            cancel_web_request(stream_id);
        }
    }

    /// Start over, in a new tab. The chat being left stays open — parked where
    /// it was in the bar, still owed its reply — and the new one goes on the
    /// end. A chat with nothing in it is not worth a tab: it is reset in place.
    pub(crate) fn new_chat(&mut self) -> bool {
        let old_id = self.chat_id;
        self.log_global("new_chat", serde_json::json!({"old_chat_id": old_id}));
        self.picker = None;
        // A new chat is a new box, and a new box types: both ring stops within this half reset
        // with it, as they do for every other arrival at the conversation.
        self.reset_ring_stop();
        self.open_tab(Chat::default());
        true
    }

    /// Throw the foreground conversation away and leave an empty one. Whatever
    /// it was owed is cancelled; whatever it had said is already written, or
    /// was never worth writing.
    pub(crate) fn reset_chat(&mut self) {
        self.cancel_in_flight();
        self.status = Status::Idle;
        self.retry = None;
        self.stream = None;
        self.reply_at = None;
        self.messages.clear();
        self.plan_cache = None;
        self.usage = None;
        self.last_request_input = None;
        self.chat_id = None;
        self.chat_revision = None;
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
        self.scheduled_run = None;
        self.plan = Plan::Off;
        self.pin_to_bottom();
        // A watcher armed for the conversation being thrown away has nothing
        // left to compare against, and would otherwise keep billing.
        let gone: Vec<u64> = self.jobs.iter().map(|job| job.id).collect();
        self.jobs.clear();
        self.forget_jobs(gone);
        self.asks.clear();
    }

    /// Scroll toward older messages, for the client that asked. Clamped here
    /// against what the last render measured, and again at render against the
    /// real line count; says whether anything on screen changes, so a wheel
    /// spinning against the top does not cost a render per notch.
    ///
    /// The measurement is only trusted while the transcript is what the render
    /// saw. A chunk that lands in the same batch as the notch grows the
    /// transcript before anything renders, and clamping against the old
    /// maximum would eat the notch that reaches the new lines.
    pub fn scroll_up(&mut self, lines: usize) -> bool {
        let released = self.release();
        let max_offset = self.view.max_offset.filter(|_| self.transcript_measured());
        let offset = self.view.scroll.entry(self.client).or_default();
        let target = match max_offset {
            // never below where we already are: the render may have put us
            // past a maximum it has since stopped believing in
            Some(max) => (*offset + lines).min(max.max(*offset)),
            None => *offset + lines,
        };
        let moved = target > *offset;
        *offset = target;
        released || moved
    }

    /// Scroll toward the tail. At the bottom already, an already-released
    /// client has nothing to show for it, and says so.
    pub fn scroll_down(&mut self, lines: usize) -> bool {
        let released = self.release();
        let offset = self.view.scroll.entry(self.client).or_default();
        let moved = *offset > 0;
        *offset = offset.saturating_sub(lines);
        released || moved
    }

    /// Hand the viewport to the client that just scrolled: whatever the arriving
    /// reply is doing, it stops being the thing that decides where they are
    /// looking. Until the next `pin_to_bottom`, which is the next time the
    /// conversation changes under them. True when this changed anything — a
    /// release drops the anchor for that client, which is visible.
    fn release(&mut self) -> bool {
        self.view.released.insert(self.client)
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

/// Unix seconds now, or zero where there is no clock worth the name.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// A job as the store holds it, due `job.due_in` seconds from `now`.
pub(crate) fn job_row(job: &Job, chat_id: Option<u64>, now: i64) -> jobs::JobRow {
    jobs::JobRow {
        job_id: job.id,
        chat_id,
        every: job.every,
        next_due_at: now + job.due_in.ceil() as i64,
        prompt: job.prompt.clone(),
        show: job.show,
        auto_confirm: job.auto_confirm,
    }
}
