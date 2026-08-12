use crate::message::{Message, Picker, PickerKind, PickerRow, RowKind};
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

    /// Write the transcript out once it has settled.
    ///
    /// In-flight is skipped so a stream saves once at the end rather than on
    /// every socket read, and an empty transcript is skipped so a pane you only
    /// opened leaves nothing behind.
    pub(crate) fn autosave(&mut self) {
        if self.is_in_flight() || self.messages.is_empty() {
            return;
        }
        let key = self.fingerprint();
        if self.saved == key {
            return;
        }
        self.saved = key;
        if let Some((id, json)) = history::save(&mut self.chat_id, &self.messages) {
            post_save(id, json);
        }
    }

    /// Autosave, but write here and now instead of on the worker, and without
    /// waiting for the reply to settle. For the two places a queued write is not
    /// good enough: `show_chats` reads the archive right back, and `BeforeClose`
    /// has no later.
    ///
    /// Unlike `autosave` this writes mid-stream. It is called once per switch or
    /// once per close, not once per socket read, so the reason for that guard
    /// doesn't apply — and a partial reply on disk beats losing it.
    pub(crate) fn flush_save(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        self.saved = self.fingerprint();
        if let Some((id, json)) = history::save(&mut self.chat_id, &self.messages) {
            history::write(id, json.as_bytes());
        }
    }

    /// The archive list, newest first, behind `/history`. Opens mid-reply: the
    /// chat you leave keeps streaming in the background, so switching costs it
    /// nothing.
    pub(crate) fn show_chats(&mut self) {
        // So the chat you are in is in the list, and current. Synchronous —
        // list() below would race a write queued on the worker. Every parked
        // chat is written too, or one that has never been saved would have no
        // id and no row to switch back to.
        self.flush_save();
        for chat in &mut self.parked {
            if let Some((id, json)) = history::save(&mut chat.chat_id, &chat.messages) {
                history::write(id, json.as_bytes());
            }
        }
        let live: Vec<u64> = self.live_chat_ids();
        let (ids, titles): (Vec<u64>, Vec<PickerRow>) = history::list()
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
        );
        self.picker = Some(Picker {
            entries: Vec::new(),
            selected: 0,
            kind: PickerKind::Models,
        });
    }

    /// Ctrl-r's answer: the agent picker. Opens over a streaming reply for the
    /// same reason `/history` does — it drives the session, not the conversation.
    ///
    /// The branches the list groups by are the only part of a snapshot that goes
    /// stale on its own — a checkout in an agent's folder changes one and no host
    /// event says so — so opening the list asks for a fresh probe. The cached
    /// answer is drawn now and `refresh_agent_picker` redraws the list when the
    /// new one lands.
    pub(crate) fn open_agent_picker(&mut self) -> bool {
        self.autosave();
        self.show_agents();
        Self::request_branch_refresh();
        true
    }

    /// The agent list from the latest event-fed daemon snapshot.
    pub(crate) fn show_agents(&mut self) {
        let (entries, ids) = self.agent_rows();
        // The first row is a project header, which cannot be selected.
        let selected = entries.iter().position(PickerRow::selectable).unwrap_or(0);
        self.picker = Some(Picker {
            entries,
            selected,
            kind: PickerKind::Agents(ids),
        });
    }

    /// Rebuild an open agent list from the snapshot that just landed, staying on
    /// the agent it was on. Whether anything about it changed.
    ///
    /// Panes come and go and branches are re-read while the list is open, and
    /// none of that would be visible otherwise. The selection follows the pane
    /// rather than the row number, so a project appearing above it does not move
    /// what `Enter` would focus.
    pub(crate) fn refresh_agent_picker(&mut self) -> bool {
        let was = match self
            .picker
            .as_ref()
            .map(|picker| (&picker.kind, picker.selected))
        {
            Some((PickerKind::Agents(ids), selected)) => ids.get(selected).copied().flatten(),
            _ => return false,
        };
        let (entries, ids) = self.agent_rows();
        let selected = was
            .and_then(|pane_id| ids.iter().position(|id| *id == Some(pane_id)))
            // The pane it was on is gone: back to the top of the list.
            .or_else(|| entries.iter().position(PickerRow::selectable))
            .unwrap_or(0);
        let picker = Picker {
            entries,
            selected,
            kind: PickerKind::Agents(ids),
        };
        let changed = self.picker.as_ref() != Some(&picker);
        self.picker = Some(picker);
        changed
    }

    /// The agent strip as picker rows, and the pane each row names — `None` for
    /// the group headers, which are drawn and skipped over.
    fn agent_rows(&self) -> (Vec<PickerRow>, Vec<Option<u32>>) {
        agents::strip_rows(
            &self.panes,
            &self.manual_kinds,
            &self.activity,
            &self.pane_projects(),
            &self.pane_branches(),
        )
        .into_iter()
        .map(|row| match row {
            agents::StripRow::Project(text) => (
                PickerRow {
                    text,
                    kind: RowKind::Project,
                },
                None,
            ),
            agents::StripRow::Branch(text) => (
                PickerRow {
                    text,
                    kind: RowKind::Branch,
                },
                None,
            ),
            agents::StripRow::Agent { pane_id, text } => (PickerRow::item(text), Some(pane_id)),
        })
        .unzip()
    }

    /// Bring an agent's pane into view, through the tool that already does it —
    /// `focus` finds the pane's tab itself, so this works across tabs.
    pub(crate) fn focus_agent(&self, pane_id: u32) {
        tools::dispatch(&ToolCall {
            id: String::new(),
            name: "focus".to_owned(),
            arguments: serde_json::json!({"pane_id": pane_id, "is_plugin": false}).to_string(),
        });
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
                    // A header has no pane id, and movement never lands on one.
                    PickerKind::Agents(ids) => {
                        if let Some(id) = ids.get(picker.selected).copied().flatten() {
                            self.focus_agent(id);
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
                let Some(messages) = history::load(id) else {
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
