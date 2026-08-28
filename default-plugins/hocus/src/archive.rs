use crate::message::{Message, Picker, PickerKind, PickerRow};
use crate::*;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// The archive
// ---------------------------------------------------------------------------

impl State {
    /// What autosave watches. The same trick `ui::TranscriptKey` relies on: only
    /// the last message is ever mutated in place, so a count and that message's
    /// length between them catch every change to the transcript.
    pub(crate) fn fingerprint(&self) -> (usize, usize) {
        (self.messages.len(), self.mutable_len())
    }

    /// The length of the one message that can change without the count changing:
    /// the streaming placeholder, which `append_to_reply` grows in place. The
    /// last message is the fallback — it is the placeholder whenever there is
    /// one and nothing has been pushed behind it, and when there is no stream
    /// nothing is mutated in place at all.
    ///
    /// A typed tool mid-reply is what makes the two differ: its call and result
    /// sit *after* the placeholder, so watching the last message would leave the
    /// arriving reply invisible to the cache, the archive and every other client.
    pub fn mutable_len(&self) -> usize {
        let at = self
            .reply_at
            .unwrap_or(self.messages.len().saturating_sub(1));
        self.messages.get(at).map_or(0, |m| m.content.len())
    }

    /// Write the transcript out.
    ///
    /// `force` writes mid-reply. Without it a stream saves once at the end
    /// rather than on every socket read; with it, the two places a later write
    /// is no use — `show_chats`, which reads the archive straight back, and
    /// `BeforeClose`, which has no later. A partial reply archived beats one
    /// lost.
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
        if !force && self.saved == key {
            return;
        }
        let saved_len = self.saved.0;
        self.saved = key;
        let mut id = self.chat_id.take();
        let written = history::save(
            &self.private_store(),
            &mut id,
            &self.messages,
            saved_len,
            self.reply_at,
        );
        self.chat_id = id;
        if written.is_err() {
            // Nothing to say it to, and nothing to do about it. Retried on the
            // next change, because the fingerprint moved but the rows did not —
            // which is the wrong way round, so put it back.
            self.saved = (saved_len, self.saved.1);
        }
    }

    /// Write every parked chat out.
    ///
    /// Taken out of `self` and put back rather than borrowed in place: the write
    /// needs the store, which is borrowed from `self` too.
    pub(crate) fn save_parked(&mut self) {
        let mut parked = std::mem::take(&mut self.parked);
        for chat in &mut parked {
            let mut id = chat.chat_id.take();
            let written = history::save(
                &self.private_store(),
                &mut id,
                &chat.messages,
                chat.saved.0,
                chat.reply_at,
            );
            chat.chat_id = id;
            if written.is_ok() {
                chat.saved = (chat.messages.len(), chat.saved.1);
            }
        }
        self.parked = parked;
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
        self.picker = Some(Picker {
            entries: titles,
            selected: 0,
            kind: PickerKind::Chats(ids),
        });
    }

    /// How many other conversations are working off-screen.
    pub fn parked_count(&self) -> usize {
        self.parked.len()
    }

    /// Chats with something still outstanding, foreground and parked alike.
    pub(crate) fn live_chat_ids(&self) -> Vec<u64> {
        self.parked
            .iter()
            .filter(|chat| chat.is_live())
            .filter_map(|chat| chat.chat_id)
            .chain(self.chat_id.filter(|_| self.is_in_flight()))
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
        self.picker = Some(Picker {
            entries: Vec::new(),
            selected: 0,
            kind: PickerKind::Models,
        });
    }

    /// The agent strip as the hocus-agents pane should draw it.
    pub(crate) fn current_strip(&self) -> agents::Strip {
        let rows = agents::strip_rows(
            &self.panes,
            &self.manual_kinds,
            &self.activity,
            &self.pane_projects(),
            &self.pane_branches(),
            &self.summaries,
            &self.hooks,
        );
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
        self.picker = Some(Picker {
            selected: models.iter().position(|id| *id == current).unwrap_or(0),
            entries: models.into_iter().map(PickerRow::item).collect(),
            kind: PickerKind::Models,
        });
        true
    }

    /// Every key while the picker is open. Unhandled keys are swallowed rather
    /// than falling through to the input box hidden behind it.
    pub(crate) fn picker_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Esc => self.picker = None,
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
                picker.selected = step(&picker.entries, picker.selected, key.bare_key);
            },
            _ => return false,
        }
        true
    }

    /// Put another chat on screen, and set this one aside rather than throwing
    /// it away. Three cases, in order of how little there is to do:
    ///
    /// 1. it is already the one you are looking at — nothing;
    /// 2. it is parked and still live — swap the two, and the reply that has
    ///    been arriving off-screen is there waiting;
    /// 3. otherwise load it from the archive.
    ///
    /// The chat being left is parked only if something is still owed to it. A
    /// settled one is already in the archive and is dropped, which is what keeps
    /// `parked` to the handful of conversations actually working.
    pub(crate) fn switch_to_chat(&mut self, id: u64) {
        if self.chat_id == Some(id) {
            return;
        }
        let parked = self.parked.iter().position(|chat| chat.chat_id == Some(id));
        let incoming = match parked {
            Some(index) => self.parked.remove(index),
            None => {
                let Some(messages) = history::load(&self.private_store(), id) else {
                    return;
                };
                Chat {
                    messages,
                    chat_id: Some(id),
                    ..Chat::default()
                }
            },
        };

        let outgoing = self.take_chat();
        self.put_chat(incoming);
        if parked.is_none() {
            // Loaded, not resumed: nothing has changed yet, so this must not
            // immediately rewrite the file it came from. A parked chat brings
            // its own `saved`, which may well be behind its transcript.
            self.saved = self.fingerprint();
        }
        self.pin_to_bottom();
        self.log_global("open_chat", serde_json::json!({"chat_id": id}));
        if outgoing.is_live() {
            self.parked.push(outgoing);
        }
    }
}

/// The row one arrow key lands on: the next selectable one in that direction,
/// stepping over the headers between.
///
/// Staying put when there is none is what makes the header the agent list opens
/// with harmless — there is nothing above it to reach, so `Up` on the first
/// agent is a no-op rather than a jump onto a line that means nothing.
fn step(entries: &[PickerRow], selected: usize, direction: BareKey) -> usize {
    let next = match direction {
        BareKey::Up => (0..selected).rev().find(|at| entries[*at].selectable()),
        _ => (selected.saturating_add(1)..entries.len()).find(|at| entries[*at].selectable()),
    };
    next.unwrap_or(selected)
}
