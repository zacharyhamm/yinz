use crate::message::{Ask, Message, Picker, PickerKind, PickerRow};
use crate::*;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// The archive
// ---------------------------------------------------------------------------

/// What autosave watches. Only one message is ever mutated in place, so a
/// count and that message's length between them catch every transcript change;
/// the usage pair catches an accounting-only change.
///
/// The one that can change without the count changing is the streaming
/// placeholder at `reply_at`, which `append_to_reply` grows in place. The last
/// message is the fallback — it is the placeholder whenever there is one and
/// nothing has been pushed behind it, and when there is no stream nothing is
/// mutated in place at all.
///
/// A typed tool mid-reply is what makes the two differ: its call and result
/// sit *after* the placeholder, so watching the last message would leave the
/// arriving reply invisible to the cache, the archive and every other client.
///
/// A free function because a parked `Chat` and the flat foreground fields both
/// need it, and the rule must not be able to drift between the two.
pub(crate) fn fingerprint(
    messages: &[Message],
    reply_at: Option<usize>,
    usage: Option<TokenUsage>,
) -> (usize, usize, Option<TokenUsage>) {
    (messages.len(), mutable_len(messages, reply_at), usage)
}

/// The length of the message `fingerprint` watches. See there.
pub(crate) fn mutable_len(messages: &[Message], reply_at: Option<usize>) -> usize {
    let at = reply_at.unwrap_or(messages.len().saturating_sub(1));
    messages.get(at).map_or(0, |m| m.content.len())
}

impl State {
    /// `fingerprint` for the foreground chat.
    pub(crate) fn fingerprint(&self) -> (usize, usize, Option<TokenUsage>) {
        fingerprint(&self.messages, self.reply_at, self.usage)
    }

    /// `mutable_len` for the foreground chat.
    pub fn mutable_len(&self) -> usize {
        mutable_len(&self.messages, self.reply_at)
    }

    /// Write the transcript out, if it changed since the last write.
    ///
    /// `force` writes mid-reply. Without it a stream saves once at the end
    /// rather than on every socket read; with it, the places a later write is
    /// no use — `show_chats`, which reads the archive straight back, a tab
    /// being closed, and `BeforeClose`, which has no later. A partial reply
    /// archived beats one lost. Forced or not, a chat whose fingerprint has
    /// not moved is not rewritten: every row written is a function of its
    /// messages and usage, so there is nothing new to say.
    ///
    /// An empty transcript is never written, so a pane you only opened leaves
    /// nothing behind.
    ///
    /// This used to be two functions, one queueing the write on the io worker
    /// and one writing the file inline. There is one now because a store write
    /// is a host call rather than a blocking file write, and because the worker
    /// could not do it anyway: the id of a new chat comes back from the apply,
    /// and the worker's mailbox has no replies.
    pub(crate) fn save_now(&mut self, force: bool) {
        if self.messages.is_empty() || (!force && self.is_in_flight()) {
            return;
        }
        let key = self.fingerprint();
        if self.saved == key {
            return;
        }
        let previous = self.saved;
        let saved_len = previous.0;
        self.saved = key;
        let mut id = self.chat_id.take();
        let written = history::save(
            &self.private_store(),
            &mut id,
            &self.messages,
            saved_len,
            self.reply_at,
            self.usage,
        );
        self.chat_id = id;
        if written.is_err() {
            // Nothing to say it to, and nothing to do about it. Retried on the
            // next change, because the fingerprint moved but the rows did not —
            // which is the wrong way round, so put it back.
            self.saved = previous;
        }
    }

    /// Write every parked chat out, mid-reply and all. Each goes through
    /// `flush_save` in turn, so the gates — a tab that was only opened leaves
    /// no row, an idle one that has not changed is not rewritten every time
    /// the list opens — are `save_now`'s and not a second copy of them.
    pub(crate) fn save_parked(&mut self) {
        for index in 0..self.parked.len() {
            self.with_parked(index, |state| state.flush_save());
        }
    }

    /// Write the transcript out once it has settled.
    pub(crate) fn autosave(&mut self) {
        self.save_now(false);
    }

    /// Write it now, mid-reply and all.
    pub(crate) fn flush_save(&mut self) {
        self.save_now(true);
    }

    /// The archive list, newest first, behind `/history`. Opens mid-reply: the
    /// chat you leave keeps streaming in the background, so switching costs it
    /// nothing.
    pub(crate) fn show_chats(&mut self) {
        // So the chat you are in is in the list, and current. Every parked chat
        // is written too, or one that has never been saved would have no id and
        // no row to switch back to.
        //
        // There is no race to avoid here any more: a write used to be queued on
        // the io worker, and `list` would read the directory before it landed.
        self.flush_save();
        self.save_parked();
        self.reset_ring_stop();
        let live: Vec<u64> = self.live_chat_ids();
        let (ids, titles): (Vec<u64>, Vec<PickerRow>) = history::list(&self.private_store())
            .into_iter()
            .map(|entry| {
                // A dot marks the ones still working, so a background reply is
                // visible from the list rather than only on arrival.
                let title = match live.contains(&entry.id) {
                    true => format!("● {}", entry.title),
                    false => entry.title,
                };
                (entry.id, PickerRow::item(title))
            })
            .unzip();
        self.picker = Some(Picker::new(titles, 0, PickerKind::Chats(ids)));
    }

    /// How many other conversations are working off-screen — receiving a
    /// reply, waiting on an answer, or counting a job down. Open tabs that
    /// have settled are not counted: the note this feeds says what is
    /// happening, and the bar already says what is open.
    pub fn parked_count(&self) -> usize {
        self.parked.iter().filter(|chat| chat.is_live()).count()
    }

    /// Tell the host what hocus has to say about a quit, and whether it should stand in the way
    /// of one.
    ///
    /// A quit takes the whole session with it, and a reply in flight or a tool waiting on a `y`
    /// is exactly the thing a person did not mean to throw away by typing `exit` in their last
    /// shell. So a working hocus guards: closing the session's last pane raises the quit
    /// confirmation instead of ending the session, and the confirmation says what is going on.
    ///
    /// A *settled* hocus does not guard. It still says how much is open, because the same
    /// confirmation goes up for `Ctrl-q` and knowing three conversations are about to go is
    /// worth a line - but nothing is outstanding, and a quit the user asked for is theirs to
    /// make.
    ///
    /// Memoised: this runs on every update, and the call becomes a host instruction.
    pub(crate) fn publish_quit_notice(&mut self) {
        // Counted here rather than taken from `live_chat_ids`, which is about *rows in the
        // archive*: it filters on `chat_id`, so a conversation that has never been saved - the
        // first exchange of a fresh session, exactly when a stray `exit` costs most - is not in
        // it. This is about conversations, saved or not.
        let working = self.parked.iter().filter(|chat| chat.is_live()).count()
            + usize::from(self.foreground_is_live());
        // Every conversation with something in it, on screen or set aside. An empty hocus - the
        // splash, nothing typed - has nothing to say and says nothing.
        let open = self
            .parked
            .iter()
            .filter(|chat| !chat.messages.is_empty())
            .count()
            + usize::from(!self.messages.is_empty());
        let notice = match (working, open) {
            (0, 0) => None,
            (0, 1) => Some("hocus: one conversation open, settled".to_owned()),
            (0, open) => Some(format!("hocus: {open} conversations open, all settled")),
            (1, _) => Some("hocus is still working on a conversation".to_owned()),
            (working, _) => Some(format!("hocus is still working - {working} conversations")),
        };
        let guard = working > 0;
        if self.last_quit_notice.as_ref() == Some(&(notice.clone(), guard)) {
            return;
        }
        self.last_quit_notice = Some((notice.clone(), guard));
        #[cfg(test)]
        {
            self.quit_notice_publishes += 1;
        }
        set_quit_notice(notice, guard);
    }

    /// Withdraw whatever was last said about a quit.
    ///
    /// The server drops a plugin's notice when its pane closes, but a guard is the one piece of
    /// state whose staleness a person would feel - a session that will not close its last pane
    /// and cannot say why - so hocus lets go of it itself on the way out rather than relying on
    /// that ordering.
    pub(crate) fn withdraw_quit_notice(&mut self) {
        self.last_quit_notice = Some((None, false));
        #[cfg(test)]
        {
            self.quit_notice_publishes += 1;
        }
        set_quit_notice(None, false);
    }

    /// Chats with something still outstanding: foreground and parked alike,
    /// and the archived ones whose jobs are held for their reopening.
    pub(crate) fn live_chat_ids(&self) -> Vec<u64> {
        self.parked
            .iter()
            .filter(|chat| chat.is_live())
            .filter_map(|chat| chat.chat_id)
            .chain(self.chat_id.filter(|_| self.is_in_flight()))
            .chain(self.held_jobs.keys().copied())
            .collect()
    }

    /// Ask the endpoint what it serves, and show the list as it arrives. Behind
    /// `/models`.
    ///
    /// No `Status` variant and no spinner: the picker is modal, so this is the
    /// only thing *it* is waiting on and the empty list is itself the "fetching"
    /// state. A reply may well be arriving behind it; the `models` context on the
    /// request is what keeps the two responses apart.
    ///
    /// ponytail: like `web_search`, a plain `web_request` has no stream id and so
    /// nothing to cancel — `Esc` stops waiting and the late reply is dropped
    /// because the picker it was for is gone.
    pub(crate) fn show_models(&mut self) {
        // Nothing to ask, and the banner behind the picker already says why.
        let Ok(cfg) = &self.cfg else {
            return;
        };
        web_request(
            api::models_url(&cfg.base_url),
            HttpVerb::Get,
            api::models_headers(cfg),
            Vec::new(),
            // Round-trips through the host: this is what tells the response
            // apart from the one that leads a chat stream.
            BTreeMap::from([("models".to_owned(), "1".to_owned())]),
            None,
        );
        self.picker = Some(Picker::new(Vec::new(), 0, PickerKind::Models));
    }

    /// The agent strip as the hocus-agents pane should draw it.
    pub(crate) fn current_strip(&self) -> agents::Strip {
        let mut rows = agents::strip_rows(
            &self.panes,
            &self.manual_kinds,
            &self.activity,
            &self.pane_projects(),
            &self.pane_branches(),
            &self.summaries,
            &self.hooks,
        );
        if !self.archived_runs.is_empty() || self.archive_more {
            rows.push(agents::StripRow::Project("archive".to_owned()));
            rows.extend(self.archived_runs.iter().map(|run| agents::StripRow::Run {
                run_id: run.id.clone(),
                text: format!("{} · {}", run.kind, run.title),
                kind: run.kind.clone(),
            }));
            if self.archive_more {
                rows.push(agents::StripRow::MoreRuns);
            }
        }
        // Only the summaries with a row to sit under. A summary outlives the pane it is about -
        // that is deliberate, it is an account of a moment - but the box is drawn *for the
        // selected row*, and a summary no row names can never be selected.
        let summaries = rows
            .iter()
            .filter_map(|row| match row {
                agents::StripRow::Agent { pane_id, .. } => Some(*pane_id),
                _ => None,
            })
            .filter_map(|pane_id| {
                let summary = self.summaries.get(&pane_id)?;
                Some((pane_id, summary.clone()))
            })
            .collect();
        agents::Strip {
            rows,
            // the raw focus, not `last_focused_agent`: the pane puts its cursor on
            // the agent you moved to and dims it when you move off, and a sticky
            // value could never say the second thing
            focused: self.focused_pane,
            summaries,
        }
    }

    /// Send the agent strip to the hocus-agents pane, unless nothing changed
    /// since the last push. `force` sends regardless: the pane asking with
    /// `AGENT_STRIP_HELLO` starts empty whatever was last sent.
    pub(crate) fn push_strip(&mut self, force: bool) {
        let strip = self.current_strip();
        // Focus is part of the comparison, so walking from one agent to another
        // still pushes even though every row reads the same.
        if !force && strip == self.last_strip {
            return;
        }
        if let Ok(payload) = serde_json::to_string(&strip) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP).with_payload(payload));
        }
        self.last_strip = strip;
    }

    /// The deferred half of `show_models`. Returns whether this response was one:
    /// a `WebRequestResult` with no `models` context is the head of a stream.
    ///
    /// Anything other than a list to show closes the picker and says why in the
    /// transcript, rather than leaving "fetching models…" on screen forever.
    pub(crate) fn finish_models(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if !context.contains_key("models") {
            return false;
        }
        // A fetch we already abandoned with Esc, or one already answered: an
        // empty model list is exactly the still-waiting state.
        match self.picker.as_ref() {
            Some(picker)
                if matches!(picker.kind, PickerKind::Models) && picker.entries.is_empty() => {},
            _ => return false,
        }

        if !(200..300).contains(&status) {
            self.picker = None;
            let err = api::api_error_message(status, body);
            self.messages.push(Message::error(err.clone()));
            self.log(
                "error",
                serde_json::json!({"error": log::truncate(&err), "status": status}),
            );
            self.pin_to_bottom();
            return true;
        }

        let models = api::model_ids(body);
        if models.is_empty() {
            self.picker = None;
            self.messages
                .push(Message::notice("the endpoint listed no models"));
            self.log(
                "notice",
                serde_json::json!({"content": "the endpoint listed no models"}),
            );
            self.pin_to_bottom();
            return true;
        }

        // Open on the model in use, so the list says where you are as well as
        // where you can go.
        let current = self
            .cfg
            .as_ref()
            .map(|cfg| cfg.model.clone())
            .unwrap_or_default();
        self.picker = Some(Picker::new(
            models.iter().cloned().map(PickerRow::item).collect(),
            models.iter().position(|id| *id == current).unwrap_or(0),
            PickerKind::Models,
        ));
        true
    }

    /// Every key while the picker is open. Unhandled keys are swallowed rather
    /// than falling through to the input box hidden behind it.
    ///
    /// Printable keys type into the filter rather than doing nothing: a long
    /// model list is reached by name, not by scrolling. The filter narrows what
    /// is drawn; `selected` still indexes the whole list, which is what keeps
    /// the ids `PickerKind::Chats` carries in step with it.
    pub(crate) fn picker_key(&mut self, key: KeyWithModifier) -> bool {
        // The filter edits, taken first because they are typing: everything
        // below is navigation.
        if let Some(picker) = self.picker.as_mut() {
            let typed = matches!(key.bare_key, BareKey::Char(_))
                && (key.has_no_modifiers() || key.has_modifiers(&[KeyModifier::Shift]));
            match key.bare_key {
                BareKey::Char(c) if typed => {
                    picker.filter.push(c);
                    picker.resettle();
                    return true;
                },
                BareKey::Backspace => {
                    picker.filter.pop();
                    picker.resettle();
                    return true;
                },
                // The input box's line kill, for the same reason.
                BareKey::Char('u') if key.has_modifiers(&[KeyModifier::Ctrl]) => {
                    picker.filter.clear();
                    picker.resettle();
                    return true;
                },
                BareKey::Tab if key.has_no_modifiers() => {
                    if let Some(rest) = picker.completion() {
                        picker.filter.push_str(&rest);
                        picker.resettle();
                    }
                    return true;
                },
                _ => {},
            }
        }
        match key.bare_key {
            // Esc undoes the narrowing before it closes the list: having typed
            // your way into a corner, the way out is back to the full list.
            BareKey::Esc => match self.picker.as_mut() {
                Some(picker) if !picker.filter.is_empty() => {
                    picker.filter.clear();
                    picker.resettle();
                },
                _ => self.picker = None,
            },
            BareKey::Enter => {
                let Some(picker) = self.picker.take() else {
                    return false;
                };
                match &picker.kind {
                    PickerKind::Chats(ids) => {
                        if let Some(id) = ids.get(picker.selected).copied() {
                            self.switch_to_chat(id);
                        }
                    },
                    // Held only for this session: config is re-resolved from the
                    // KDL block and the environment on every load.
                    PickerKind::Models => {
                        if let (Some(model), Ok(cfg)) =
                            (picker.entries.get(picker.selected), &mut self.cfg)
                        {
                            let m = model.text.clone();
                            cfg.model = m.clone();
                            self.log_global("model_switch", serde_json::json!({"model": m}));
                        }
                    },
                }
            },
            BareKey::Up | BareKey::Down => {
                let Some(picker) = self.picker.as_mut() else {
                    return false;
                };
                picker.selected = step(&picker.visible(), picker.selected, key.bare_key);
            },
            _ => return false,
        }
        true
    }

    /// Put another chat on screen, and set this one aside rather than throwing
    /// it away. Three cases, in order of how little there is to do:
    ///
    /// 1. it is already the one you are looking at — nothing;
    /// 2. it is open in another tab — switch to that tab, and the reply that
    ///    has been arriving off-screen is there waiting;
    /// 3. otherwise load it from the archive into a new tab on the end.
    pub(crate) fn switch_to_chat(&mut self, id: u64) {
        if self.chat_id == Some(id) {
            return;
        }
        if let Some(index) = self.parked.iter().position(|chat| chat.chat_id == Some(id)) {
            self.switch_to_tab(self.tab_of_parked(index));
            return;
        }
        let Some(mut messages) = history::load(&self.private_store(), id) else {
            return;
        };
        // A chat that cannot be read is not opened, as above: opened with no
        // totals, its next save would write that absence over the real ones.
        let Ok(usage) = history::load_usage(&self.private_store(), id) else {
            return;
        };
        // The jobs the chat was armed with when hocus last ran come back with
        // it, and start counting down again from here.
        let jobs = self.take_held_jobs(id);
        if !jobs.is_empty() {
            messages.push(Message::notice(format!(
                "restored {} scheduled job(s)",
                jobs.len()
            )));
        }
        let arm = !jobs.is_empty();
        self.open_tab(Chat {
            messages,
            chat_id: Some(id),
            usage,
            jobs,
            ..Chat::default()
        });
        if arm {
            self.arm_timer(JOB_SECONDS);
        }
        self.log_global("open_chat", serde_json::json!({"chat_id": id}));
    }

    /// Put `incoming` on screen in a tab of its own, on the end of the bar.
    /// The chat being left stays open where it was — parked, still owed its
    /// reply — unless there is nothing to it, in which case it is simply let
    /// go rather than left as a blank tab nothing will ever reap.
    pub(crate) fn open_tab(&mut self, incoming: Chat) {
        self.chat_tab_view.remove(&self.client);
        let worth_a_tab = self.worth_a_tab();
        let outgoing = self.swap_foreground(incoming);
        if worth_a_tab {
            self.parked.insert(self.chat_tab, outgoing);
            self.chat_tab = self.parked.len();
        }
        // Opened, not resumed: nothing has changed yet, so this must not
        // immediately rewrite the row it came from. A chat brings its own
        // `saved`, which may well be behind its transcript, and a fresh one has
        // nothing to write.
        self.saved = self.fingerprint();
    }

    /// Whether the foreground chat would be missed: anything said, anything
    /// saved, a question waiting its turn, a watcher armed, or a reply on its
    /// way. A chat that only looks empty is kept; one that is empty is not.
    fn worth_a_tab(&self) -> bool {
        !self.messages.is_empty()
            || self.chat_id.is_some()
            || !self.jobs.is_empty()
            || !self.asks.is_empty()
            || self.is_in_flight()
    }

    /// Put the chat at bar position `tab` on screen. The one being left goes
    /// back into `parked` where the bar had it, so the order you see never
    /// changes under a switch.
    pub(crate) fn switch_to_tab(&mut self, tab: usize) {
        let Some(index) = self.parked_at_tab(tab) else {
            return;
        };
        if index >= self.parked.len() {
            return;
        }
        let incoming = self.parked.remove(index);
        let outgoing = self.swap_foreground(incoming);
        // Once the incoming chat is out, the outgoing one's slot is where it
        // was in the bar, less one if the incoming chat sat before it.
        let insert_at = if self.chat_tab < tab {
            self.chat_tab
        } else {
            self.chat_tab - 1
        };
        self.parked.insert(insert_at, outgoing);
        self.chat_tab = tab;
        self.chat_tab_view.remove(&self.client);
        self.log_global("open_chat", serde_json::json!({"chat_id": self.chat_id}));
    }

    /// Close the tab at bar position `tab`. Whatever it was doing is stopped —
    /// a stream cancelled, a tool run aborted — and what it has is written
    /// out, so `/history` can bring it back. Closing the one on screen moves to
    /// its right-hand neighbour, or the left one from the end; closing the
    /// last tab leaves a fresh empty chat rather than no chat at all.
    pub(crate) fn close_tab(&mut self, tab: usize) {
        if tab >= self.chat_tab_count() {
            return;
        }
        self.chat_tab_view.remove(&self.client);
        let closing_id = match self.parked_at_tab(tab) {
            None => self.chat_id,
            Some(index) => self.parked[index].chat_id,
        };
        self.log_global("close_chat", serde_json::json!({"chat_id": closing_id}));
        if self.parked_at_tab(tab).is_none() {
            if self.parked.is_empty() {
                // The only tab: stopped and written out, then reset in place.
                // Not `new_chat`, which would park a chat still mid-reply.
                self.stop_chat();
                self.reset_chat();
                return;
            }
            let neighbour = if tab + 1 < self.chat_tab_count() {
                tab + 1
            } else {
                tab - 1
            };
            self.switch_to_tab(neighbour);
        }
        // Now certainly parked. Its position may have moved by the switch.
        let Some(index) = self.parked_at_tab(tab) else {
            return;
        };
        self.with_parked(index, |state| state.stop_chat());
        let closed = self.parked.remove(index);
        // A closed chat's jobs stop for good, as `/clear`'s do: nothing ticks
        // them once the chat is gone, and a row left behind would come back at
        // the next start as a job nobody remembers arming.
        let gone: Vec<u64> = closed.jobs.iter().map(|job| job.id).collect();
        self.forget_jobs(gone);
        if index < self.chat_tab {
            self.chat_tab -= 1;
        }
    }

    /// Stop whatever the foreground chat is doing and write it out, ahead of
    /// it being thrown away. Every way a chat is closed goes through here, so
    /// the order below is decided once:
    ///
    /// 1. the tool run and the stream are stopped;
    /// 2. questions still queued become notices — their askers were told they
    ///    had been taken, and a notice survives into the archive without ever
    ///    reaching the model;
    /// 3. a placeholder nothing arrived into is dropped, so a tab closed before
    ///    its first token archives no empty reply;
    /// 4. the transcript is written *with `reply_at` still set*, which is what
    ///    lets `history::save` rewrite a reply that grew in place behind a
    ///    typed tool's rows.
    pub(crate) fn stop_chat(&mut self) {
        if self.tools_busy() {
            self.abort_tools();
        }
        self.cancel_in_flight();
        self.stream = None;
        self.drop_spotlight();
        unsent_asks_to_notices(&mut self.messages, &mut self.asks);
        if self.reply_is_empty() {
            self.pop_empty_placeholder();
        }
        self.flush_save();
        self.reply_at = None;
    }
}

/// Turn every queued question into an `unsent question` notice. Written as a
/// notice rather than dropped because the asker was told the question had been
/// queued; written as a notice rather than a user message so that reopening
/// the chat does not answer it by surprise. Over the flat foreground fields or
/// a parked `Chat`'s alike, which is why it is not a method.
pub(crate) fn unsent_asks_to_notices(
    messages: &mut Vec<Message>,
    asks: &mut std::collections::VecDeque<Ask>,
) {
    for ask in std::mem::take(asks) {
        messages.push(Message::notice(format!("unsent question: {}", ask.shown())));
    }
}

/// The row one arrow key lands on: the next selectable one in that direction,
/// stepping over the headers between.
///
/// Staying put when there is none is what makes the header the agent list opens
/// with harmless — there is nothing above it to reach, so `Up` on the first
/// agent is a no-op rather than a jump onto a line that means nothing.
fn step(visible: &[usize], selected: usize, direction: BareKey) -> usize {
    let next = match direction {
        BareKey::Up => visible.iter().rev().find(|at| **at < selected),
        _ => visible.iter().find(|at| **at > selected),
    };
    next.copied().unwrap_or(selected)
}
