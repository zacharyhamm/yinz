use super::*;
use hocus_proto::vms::{self, Identity, Operation, Request, ResultMessage, Snapshot};
use std::ops::Range;

/// How long the tab waits on a reply before it stops holding its one request slot for it.
///
/// A safety net for replies hocus may never send - a hocus that reloaded, a companion
/// re-verified - and not a bound on the work: the provider times out on inactivity, so a
/// healthy sync can run past this. Which is why a request given up on is still answered
/// (`timed_out_operations`) and why the words for it say "no reply yet", not "failed".
const REQUEST_TIMEOUT_MS: i64 = 60_000;

/// What the tab says while there is no one hocus to ask: none open as the panel's
/// conversation, or two, which is as unverifiable as none. Not "hocus is not running" - two
/// overlays are this too - and it names the key that fixes the usual case.
const NO_OWNER: &str = "No single hocus panel to ask · Ctrl e c opens the conversation";

/// [`NO_OWNER`] as the divider says it: the key that fixes it first, because `divider_line`
/// cuts the label from the tail and a narrow pane would keep only the complaint. The
/// placeholder keeps [`NO_OWNER`], which it wraps whole.
const NO_OWNER_DIVIDER: &str = "Ctrl e c opens the conversation · no single hocus panel to ask";

/// What becomes of a request left out when the owner changes: the reply could only come from
/// the hocus it was sent to, which is no longer the one this tab listens to.
const OWNER_CHANGED: &str = "The hocus panel changed before answering, so no result will arrive \
                             here. It may have run; check the VM's state before retrying.";

#[derive(Default)]
pub(super) struct View {
    owner: Option<PluginId>,
    pub visible: bool,
    snapshot: Snapshot,
    /// Whether a listing has ever arrived. Until one has, nothing an error says has made
    /// anything stale: a provider that was never configured is unavailable, not out of date,
    /// and not worth the tab's `!` for the rest of the session.
    loaded: bool,
    selected: usize,
    scroll: usize,
    /// Where the wheel left the list's window, or `None` to keep it centred on the selection.
    /// A key or a click on the list hands it back to the selection, as on Agents.
    list_scroll: Option<usize>,
    pub details: bool,
    pane: Option<usize>,
    reveal_pane: bool,
    pending: Option<Request>,
    pending_started_ms: i64,
    pending_deadline_ms: i64,
    /// The user's own request, waiting for a background listing to answer first - or for hocus
    /// to finish one it is still [`busy`](Self::busy) with.
    ///
    /// hocus runs one request at a time, so sending it alongside the listing would only be
    /// refused - and dropping it, which is what `c`, `s` and `x` used to do whenever the
    /// 2-second poll happened to be out, threw away a key the divider was advertising. It
    /// goes out as soon as a listing answers without being refused, and only if its lease is
    /// still listed.
    queued: Option<(Operation, Identity)>,
    /// Requests given up on that hocus may still answer, each with when it was sent.
    timed_out_operations: Vec<(Request, i64)>,
    /// hocus answered the last listing with [`vms::BUSY`] while one of those was still out:
    /// proof the request is alive, only slow, so the tab says it is still running rather than
    /// calling the list stale - and holds `c`, `s` and `x`, which hocus would refuse the same
    /// way. Cleared by the late reply, by the next listing that is not refused, and by an owner
    /// change, so it cannot hold them for good.
    busy: bool,
    /// The last listing went unanswered past the deadline, and hocus has answered none since
    /// except to refuse it. Its [`vms::BUSY`] is then most likely that listing still running,
    /// so the refusal keeps the expiry's words rather than blaming "another VM request" that
    /// nobody sent.
    list_expired: bool,
    sequence: u64,
    confirming: Option<(Operation, Identity)>,
    results: Vec<Outcome>,
    /// One of the user's requests failed while they were not looking at this tab: the one
    /// failure nothing else would mark, since `error` is the listing's. Cleared when the tab
    /// is on screen again, and by the next request.
    failed: bool,
    error: String,
    last_list_rows: usize,
    pub last_details_rows: usize,
    window: usize,
    max_scroll: usize,
    /// The wrapped detail rows each of the selected lease's pane lines took this frame. What
    /// a click on a pane and the pane cursor's reveal go by, so neither can land on anything
    /// else - host output that happens to start with `> ` included.
    pane_rows: Vec<Range<usize>>,
}

/// What became of one of the user's requests: kept per lease, and drawn at the top of its
/// details, so the next thing read after pressing a key is the answer to it.
struct Outcome {
    identity: Identity,
    operation: Operation,
    failed: bool,
    at_ms: i64,
    text: String,
}

/// The selected row's details, one entry per line before wrapping, and which of those
/// entries are the lease's pane lines.
struct Details {
    lines: Vec<String>,
    panes: Range<usize>,
}

impl View {
    pub(super) fn clear_dimensions(&mut self) {
        self.last_list_rows = 0;
        self.last_details_rows = 0;
        self.window = 0;
        self.max_scroll = 0;
        self.pane_rows.clear();
    }

    /// Whether the tab is worth its `!`: a listing gone stale since one last arrived, or a
    /// request of the user's that failed while another tab was showing.
    pub(super) fn has_error(&self) -> bool {
        (self.loaded && !self.error.is_empty()) || self.failed
    }

    /// The listing's error as one row can carry it: see [`reason_line`]. It goes on the
    /// one-row divider, after a prefix that already says the listing is stale or unavailable.
    fn error_line(&self) -> &str {
        reason_line(&self.error)
    }

    fn error_prefix(&self) -> &'static str {
        match self.loaded {
            true => "STALE",
            false => "Unavailable",
        }
    }

    fn selected_lease(&self) -> Option<&vms::Lease> {
        self.snapshot.vms.get(self.selected)
    }

    /// Whether one of the user's own requests is out or waiting to go - including one given up
    /// on that hocus has since answered [`busy`](Self::busy) for, which is still running there
    /// and would get the next one refused. That, and not a background listing, is what holds
    /// `c`, `s` and `x`: one at a time, and the divider says which one.
    fn user_request_out(&self) -> bool {
        self.busy
            || self.queued.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.operation != Operation::List)
    }

    /// Rows the list draws: the leases, then the retained disks under their header.
    fn list_len(&self) -> usize {
        let disks = self.snapshot.recoveries.len();
        self.snapshot.vms.len() + if disks == 0 { 0 } else { 1 + disks }
    }
}

impl State {
    pub(super) fn vm_sessions(&mut self, sessions: &[SessionInfo]) -> bool {
        let owner = sessions
            .iter()
            .find(|session| session.is_current_session)
            .and_then(vms::panel_owner);
        if self.vms.owner == owner {
            return false;
        }
        // A reply is taken only from the owner it was sent to, so whatever was out is forgotten
        // below. Said where each one's answer would have gone, rather than leaving "the result
        // will replace this line" for nothing to replace. Not failures, whose `!` would go up
        // for closing the conversation: the request may well have run.
        let unanswered: Vec<_> = self
            .vms
            .timed_out_operations
            .iter()
            .map(|(request, _)| request)
            .chain(
                self.vms
                    .pending
                    .iter()
                    .filter(|pending| pending.operation != Operation::List),
            )
            .filter_map(|request| Some((request.operation, request.target.clone()?)))
            .collect();
        for (operation, identity) in unanswered {
            self.record_outcome(operation, identity, false, OWNER_CHANGED.into());
        }
        if let Some((operation, identity)) = self.vms.queued.take() {
            self.record_outcome(
                operation,
                identity,
                false,
                "Not sent: the hocus panel changed while this waited for the list.".into(),
            );
        }
        self.vms.owner = owner;
        self.vms.visible = false;
        self.vms.pending = None;
        self.vms.timed_out_operations.clear();
        self.vms.busy = false;
        self.vms.list_expired = false;
        self.vms.confirming = None;
        if owner.is_some() && !self.native_tabs {
            // Request the owner's visibility after verifying it, even if its first update
            // arrived before this manifest.
            self.send_selection();
        }
        true
    }

    /// The 2-second tick's part in this tab: expire a request hocus has not answered, list
    /// again if the tab is showing and nothing is out, and say whether that is worth a redraw.
    pub(super) fn poll_vms(&mut self) -> bool {
        let expired = self.expire_vm_request(now_ms());
        let showing = self.surface == Surface::Vms && self.vms.visible;
        if showing {
            // Seen: the `!` is for a failure that arrived while the user was elsewhere.
            self.vms.failed = false;
        }
        // A request the user confirmed goes out from the listing's reply, which checks its
        // lease is still listed - so once hocus is no longer busy it gets that listing even
        // with the tab out of sight. Left for the next visit, a close confirmed before the user
        // went elsewhere went out hours later, the moment the tab was looked at again.
        let drain = self.vms.queued.is_some() && !self.vms.busy;
        // Not held while a close or sync question is armed: the question names the panes and
        // the sharing it is about, and a list frozen under it - for as long as the keys are in
        // the conversation - showed them as they were minutes ago, with nothing saying so. A
        // listing that moves the selection off the lease drops the question, and a `y` pressed
        // while one is out waits for it (`send_or_queue`).
        if (showing || drain) && self.vms.pending.is_none() {
            self.vm_request(Operation::List, None);
        }
        // A request of the user's own counts its seconds on the divider, so a tick that
        // brings no listing still has something to redraw.
        let counting = showing
            && (self.vms.busy
                || self
                    .vms
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.operation != Operation::List));
        expired || counting
    }
    fn expire_vm_request(&mut self, now: i64) -> bool {
        if now < self.vms.pending_deadline_ms {
            return false;
        }
        let Some(request) = self.vms.pending.clone() else {
            return false;
        };
        let seconds = REQUEST_TIMEOUT_MS / 1000;
        if request.operation == Operation::List {
            // Through the reply path, for the words it leaves on the divider. Not for a request
            // queued behind the listing: hocus is most likely still running the listing, and
            // would refuse it as busy - which used to be recorded as the request failing, and
            // lose it. It waits for a listing hocus answers, as `send_or_queue` makes a new one
            // wait while this one is still out.
            let queued = self.vms.queued.take();
            self.accept_vm_result(ResultMessage {
                request_id: request.request_id,
                operation: request.operation,
                target: request.target.clone(),
                snapshot: None,
                output: String::new(),
                error: format!("VM listing did not answer in {seconds} s; retrying."),
            });
            self.vms.queued = queued;
            // After the reply path, which clears it for any answer that is not a refusal.
            self.vms.list_expired = true;
            return true;
        }
        // Not a failure: the provider times out on inactivity, not at this deadline, so the
        // request may well be running still - and its reply is still taken when it comes.
        self.vms.pending = None;
        if let Some(identity) = request.target.clone() {
            self.record_outcome(
                request.operation,
                identity,
                false,
                format!(
                    "No reply after {seconds} s; hocus may still be running it. The result will \
                     replace this line when it arrives."
                ),
            );
        }
        let started = self.vms.pending_started_ms;
        self.vms.timed_out_operations.push((request, started));
        self.poll_vms();
        true
    }
    /// Send `operation` to the owner, or say it could not go: there is no single hocus to
    /// ask, or a request is already out.
    fn vm_request(&mut self, operation: Operation, target: Option<Identity>) -> bool {
        let Some(owner) = self.vms.owner else {
            return false;
        };
        if self.vms.pending.is_some() {
            return false;
        }
        let now = now_ms();
        self.vms.sequence = self.vms.sequence.max(now.max(0) as u64) + 1;
        let request = Request {
            request_id: self.vms.sequence,
            operation,
            target,
        };
        let Ok(payload) = serde_json::to_string(&request) else {
            return false;
        };
        pipe_message_to_plugin(
            MessageToPlugin::new(vms::REQUEST)
                .with_destination_plugin_id(owner)
                .with_payload(payload),
        );
        self.vms.pending = Some(request);
        self.vms.pending_started_ms = now;
        self.vms.pending_deadline_ms = now.saturating_add(REQUEST_TIMEOUT_MS);
        if operation != Operation::List {
            // A new attempt: the old failure's `!` has said what it had to.
            self.vms.failed = false;
        }
        true
    }
    /// The user's own request: out now, or queued behind a background listing, which answers
    /// within the tick, or behind a request hocus is still [`busy`](View::busy) with - a `y`
    /// armed just before hocus said so, which sent now would only be refused. So too behind a
    /// listing given up on ([`list_expired`](View::list_expired)), which hocus is most likely
    /// still running. False only where it cannot go at all.
    fn send_or_queue(&mut self, operation: Operation, identity: Identity) -> bool {
        match &self.vms.pending {
            None if !self.vms.busy && !self.vms.list_expired => {
                self.vm_request(operation, Some(identity))
            },
            Some(pending) if pending.operation != Operation::List => false,
            _ => {
                self.vms.queued = Some((operation, identity));
                true
            },
        }
    }
    /// Keep what became of a request of the user's, in place of the last one for its lease.
    fn record_outcome(
        &mut self,
        operation: Operation,
        identity: Identity,
        failed: bool,
        text: String,
    ) {
        if self
            .vms
            .selected_lease()
            .is_some_and(|lease| lease.identity() == identity)
        {
            // It is drawn at the top of the details, so that is where they go, however far
            // down them the user had scrolled while waiting.
            self.vms.scroll = 0;
        }
        if failed && !(self.surface == Surface::Vms && self.vms.visible) {
            self.vms.failed = true;
        }
        self.vms
            .results
            .retain(|outcome| outcome.identity != identity);
        self.vms.results.push(Outcome {
            identity,
            operation,
            failed,
            at_ms: now_ms(),
            text,
        });
    }
    pub(super) fn vm_pipe(&mut self, message: &PipeMessage) -> Option<bool> {
        if matches!(message.name.as_str(), vms::VISIBILITY | vms::RESULT)
            && !self
                .vms
                .owner
                .is_some_and(|owner| message.source == PipeSource::Plugin(owner))
        {
            return Some(false);
        }
        match message.name.as_str() {
            vms::VISIBILITY => {
                let visible = message.payload.as_deref() == Some("true");
                let opened = visible && !self.vms.visible;
                self.vms.visible = visible;
                if opened {
                    self.poll_vms();
                }
                // hocus putting the panel away is the panel going away, whichever half asked.
                if !visible {
                    self.end_visit();
                }
                // A shared overlay moved to another tab can reopen without Visible(true).
                // The verified owner's signal covers the whole panel, including Swamp.
                let swamp_changed = self.swamp_panel_visibility(visible);
                Some(opened || swamp_changed)
            },
            vms::RESULT => {
                let Some(result) = message
                    .payload
                    .as_deref()
                    .and_then(|payload| serde_json::from_str::<ResultMessage>(payload).ok())
                else {
                    return Some(false);
                };
                Some(self.accept_vm_result(result))
            },
            _ => None,
        }
    }
    fn accept_vm_result(&mut self, result: ResultMessage) -> bool {
        let matches = |pending: &Request| {
            pending.request_id == result.request_id
                && pending.operation == result.operation
                && pending.target == result.target
        };
        if self.vms.pending.as_ref().is_some_and(matches) {
            self.vms.pending = None;
        } else if let Some(index) = self
            .vms
            .timed_out_operations
            .iter()
            .position(|(request, _)| matches(request))
        {
            self.vms.timed_out_operations.remove(index);
            if self.vms.timed_out_operations.is_empty() {
                self.vms.busy = false;
            }
        } else {
            return false;
        }
        if result.operation == Operation::List {
            // hocus answers one request at a time, so "busy" while one we stopped waiting on
            // is still out says that request is alive - not that the listing failed. With none
            // out but a listing given up on, the one hocus is busy with is most likely that
            // listing: the list is still stale, and the expiry already says why without
            // blaming another request.
            let refused = result.error == vms::BUSY;
            self.vms.busy = refused && !self.vms.timed_out_operations.is_empty();
            let own_listing = refused && self.vms.list_expired;
            if !refused {
                self.vms.list_expired = false;
            }
            if !self.vms.busy && !own_listing {
                self.vms.error = result.error;
            }
            if let Some(snapshot) = result.snapshot {
                self.vms.loaded = true;
                let selected = self.vms.selected_lease().map(|lease| lease.identity());
                let previous_selection = selected.clone();
                let pane_id = self
                    .vms
                    .selected_lease()
                    .and_then(|lease| self.vms.pane.and_then(|index| lease.panes.get(index)))
                    .map(|pane| pane.id);
                let recovery = self
                    .vms
                    .selected
                    .checked_sub(self.vms.snapshot.vms.len())
                    .and_then(|index| self.vms.snapshot.recoveries.get(index))
                    .map(|disk| disk.path.clone());
                self.vms.selected = selected
                    .and_then(|identity| {
                        snapshot
                            .vms
                            .iter()
                            .position(|lease| lease.identity() == identity)
                    })
                    .or_else(|| {
                        recovery
                            .and_then(|path| {
                                snapshot
                                    .recoveries
                                    .iter()
                                    .position(|disk| disk.path == path)
                            })
                            .map(|index| index + snapshot.vms.len())
                    })
                    .unwrap_or(self.vms.selected)
                    .min((snapshot.vms.len() + snapshot.recoveries.len()).saturating_sub(1));
                if snapshot
                    .vms
                    .get(self.vms.selected)
                    .map(|lease| lease.identity())
                    != previous_selection
                {
                    self.vms.scroll = 0;
                    self.vms.pane = None;
                    self.vms.confirming = None;
                } else if self.vms.pane.is_some() {
                    self.vms.pane = snapshot.vms.get(self.vms.selected).and_then(|lease| {
                        lease.panes.iter().position(|pane| Some(pane.id) == pane_id)
                    });
                }
                self.vms.snapshot = snapshot;
            }
            // The listing the user's request was waiting on has answered. It goes out only to
            // a lease still listed: hocus would refuse a changed one anyway, and saying so
            // here names the lease rather than leaving it to a later refusal. Not after a
            // listing hocus refused as busy - with a request given up on, or with a listing
            // given up on, which it is most likely still running - since it would refuse this
            // too: it waits for a listing that is not refused.
            if let Some((operation, identity)) = self.vms.queued.take_if(|_| !refused) {
                if self
                    .vms
                    .snapshot
                    .vms
                    .iter()
                    .any(|lease| lease.identity() == identity)
                {
                    self.vm_request(operation, Some(identity));
                } else {
                    self.record_outcome(
                        operation,
                        identity,
                        true,
                        "The lease changed while this waited for the list; select it and try \
                         again."
                            .into(),
                    );
                }
            }
        } else {
            if let Some(identity) = result.target {
                if result.error == vms::BUSY {
                    // Not run: hocus was still busy with another request. That is proof the
                    // other one is alive, not that this one failed - recorded as a failure, the
                    // user's confirmed close or sync was lost. It goes back in the queue, to go
                    // out after a listing hocus answers.
                    if self.vms.queued.is_none() {
                        self.vms.queued = Some((result.operation, identity));
                    }
                } else {
                    let failed = !result.error.is_empty();
                    let text = match failed {
                        true => result.error,
                        false => result.output,
                    };
                    self.record_outcome(result.operation, identity, failed, text);
                }
            }
            self.poll_vms();
        }
        true
    }
    pub(super) fn vm_key(&mut self, key: KeyWithModifier) -> bool {
        if self.vms.confirming.is_some() && key.has_no_modifiers() {
            let delta = match key.bare_key {
                BareKey::Up | BareKey::Char('k') => -1,
                BareKey::Down | BareKey::Char('j') => 1,
                BareKey::PageUp => -(self.vms.last_details_rows.max(1) as isize),
                BareKey::PageDown => self.vms.last_details_rows.max(1) as isize,
                _ => 0,
            };
            if delta != 0 {
                // The question stays on the divider, so reading down the panes it would close
                // cannot scroll it out of view.
                self.vms.scroll = self
                    .vms
                    .scroll
                    .saturating_add_signed(delta)
                    .min(self.vms.max_scroll);
                return true;
            }
        }
        if let Some((operation, identity)) = self.vms.confirming.take() {
            if key.bare_key == BareKey::Char('y')
                && key.has_no_modifiers()
                && self
                    .vms
                    .selected_lease()
                    .is_some_and(|lease| lease.identity() == identity)
            {
                self.send_or_queue(operation, identity);
            }
            return true;
        }
        if key.bare_key == BareKey::Tab {
            let back = key.has_modifiers(&[KeyModifier::Shift]);
            match (self.vms.details, back) {
                (false, true) => self.tabs_focused = true,
                (false, false) if self.vms.last_details_rows > 0 => self.vms.details = true,
                (true, true) => self.vms.details = false,
                _ => self.yield_to_chat(wire::STOP_TABS),
            }
            return true;
        }
        // Ctrl-o never gets this far: `on_key` takes it ahead of every surface.
        if key.has_modifiers(&[KeyModifier::Ctrl]) {
            if key.bare_key == BareKey::Char('r') && self.vms.pending.is_none() {
                // Asked for, so not held to `poll_vms`'s conditions: a key reaching this pane
                // means the tab is on screen, whatever hocus last said about the panel. One
                // request at a time still - one already out is what the divider shows.
                self.vm_request(Operation::List, None);
            }
            return true;
        }
        if !key.has_no_modifiers() {
            return false;
        }
        match key.bare_key {
            // By which half has the keys, which the divider shows, rather than by whether a
            // pane cursor happens to be kept: Esc in the details goes back to the list, and
            // Esc in the list puts the panel away.
            BareKey::Esc if self.vms.details => {
                self.vms.pane = None;
                self.vms.details = false;
            },
            BareKey::Esc => self.hide_the_panel(),
            BareKey::Up | BareKey::Char('k') => self.vm_step(-1),
            BareKey::Down | BareKey::Char('j') => self.vm_step(1),
            BareKey::PageUp => {
                self.vms.scroll = self
                    .vms
                    .scroll
                    .saturating_sub(self.vms.last_details_rows.max(1))
            },
            BareKey::PageDown => {
                self.vms.scroll =
                    (self.vms.scroll + self.vms.last_details_rows.max(1)).min(self.vms.max_scroll)
            },
            BareKey::Enter => {
                if self.vms.details && self.vm_jump() {
                    return true;
                }
                // A pane too short to split has no details to open: Enter would leave a pane
                // cursor nobody can see, for the next Esc to clear instead of hiding.
                if self.vms.last_details_rows == 0 {
                    return false;
                }
                let Some(lease) = self.vms.selected_lease() else {
                    return false;
                };
                self.vms.pane = (!lease.panes.is_empty()).then_some(0);
                self.vms.details = true;
                self.vms.reveal_pane = true;
                self.vms.scroll = 0;
            },
            BareKey::Char(c @ ('c' | 's' | 'x')) => {
                // The divider already says what is running or queued; the redraw keeps it
                // in view.
                if self.vms.user_request_out() {
                    return true;
                }
                let Some(lease) = self.vms.selected_lease() else {
                    return false;
                };
                let identity = lease.identity();
                // No y/n that could not be honoured: the divider says why nothing happens.
                if self.vms.owner.is_none() {
                    return true;
                }
                if c != 'x' && lease.entry.worktree.is_none() {
                    self.record_outcome(
                        if c == 's' { Operation::Sync } else { Operation::Changes },
                        identity,
                        false,
                        "No host worktree is linked to this VM. Copy needed files out before closing it."
                            .into(),
                    );
                    self.vms.details = true;
                    self.vms.scroll = 0;
                    return true;
                }
                match c {
                    'c' => {
                        self.send_or_queue(Operation::Changes, identity);
                    },
                    _ => {
                        let operation = match c {
                            's' => Operation::Sync,
                            _ => Operation::Close,
                        };
                        self.vms.confirming = Some((operation, identity));
                        self.vms.details = true;
                        self.vms.scroll = 0;
                    },
                }
            },
            _ => return false,
        }
        true
    }
    /// Drop an armed close or sync question. For the moments nothing else would answer it -
    /// the panel going away, the surface switching, a click moving the selection - where it
    /// would otherwise take the first key of the next visit, or a key meant for another row.
    pub(super) fn cancel_vm_confirmation(&mut self) {
        self.vms.confirming = None;
    }

    /// The keys this tab answers right now, as `(keys, what they do)` for the panel's `?`
    /// sheet, the most useful first - and only the ones that would do something: a retained
    /// disk has no lease to act on, a pane too short to split has no details to go into, and
    /// with no one hocus to ask nothing is sent.
    ///
    /// An armed question is all this tab lists, under which the panel's own keys follow as
    /// ever: every key but its answer and a scroll cancels it, so nothing else the tab answers
    /// is what the next key does.
    pub(super) fn vm_key_sheet(&self) -> Vec<(String, String)> {
        let row = |keys: &str, what: &str| (keys.to_owned(), what.to_owned());
        let lease = self.vms.selected_lease();
        if let Some((operation, target)) = &self.vms.confirming {
            // `y` is honoured only for the lease the question names (`vm_key`).
            if lease.is_some_and(|lease| lease.identity() == *target) {
                let answer = match operation {
                    Operation::Close => format!("close {} and its panes", target.name),
                    _ => format!("sync {} into the host worktree", target.name),
                };
                return vec![
                    row("y", &answer),
                    row("↑↓ j k PgUp PgDn", "scroll the details; the question stays"),
                    row("n / Esc", "cancel"),
                    row("any other key", "cancel"),
                ];
            }
        }
        let split = self.vms.last_details_rows > 0;
        let owner = self.vms.owner.is_some();
        let panes = lease.is_some_and(|lease| !lease.panes.is_empty());
        let pane = self.vms.details && self.vms.pane.is_some();
        let mut sheet = vec![];
        match (self.vms.details, pane) {
            (true, true) => sheet.extend([
                row("Enter", "go to the chosen pane"),
                row("↑↓ j k", "choose a pane"),
                row("Esc", "back to the list"),
            ]),
            (true, false) => {
                sheet.push(row("↑↓ j k", "scroll the details"));
                if panes {
                    sheet.push(row("Enter", "choose one of the VM's panes"));
                }
                sheet.push(row("Esc", "back to the list"));
            },
            _ => {
                sheet.push(row("↑↓ j k", "choose a VM or retained disk"));
                // A pane too short to split has no details for Enter to open (`vm_key`).
                if lease.is_some() && split {
                    sheet.push(row(
                        "Enter",
                        match panes {
                            true => "open the VM's details and panes",
                            false => "open the VM's details",
                        },
                    ));
                }
                sheet.push(row("Esc", "hide the panel"));
            },
        }
        if lease.is_some() && owner {
            sheet.push(row("x", "close lease and panes (asks first)"));
            if lease.is_some_and(|lease| lease.entry.worktree.is_some()) {
                sheet.extend([
                    row("s", "sync guest → host (asks first)"),
                    row("c", "count guest paths changed since the last sync"),
                ]);
            }
        }
        if !owner {
            // Named here as on the divider: it is what makes the rest of this tab's keys work.
            sheet.push(row(
                "Ctrl e c",
                "open the conversation, which answers x, s, c and Ctrl-r here",
            ));
        }
        if split {
            sheet.push(row("PgUp PgDn", "scroll the details"));
        }
        if owner {
            sheet.push(row("Ctrl-r", "refresh the list now"));
        }
        // As `vm_key` walks the ring: the details are a stop only where they are drawn.
        sheet.extend(match (self.vms.details, split) {
            (true, _) => [
                row("Tab", "on to the conversation"),
                row("Shift+Tab", "back to the list"),
            ],
            (false, true) => [
                row("Tab", "into the details"),
                row("Shift+Tab", "to the panel's tabs"),
            ],
            (false, false) => [
                row("Tab", "on to the conversation"),
                row("Shift+Tab", "to the panel's tabs"),
            ],
        });
        sheet.extend([
            row(
                "click",
                "select a row; a pane line picks the pane, a second click goes to it",
            ),
            row("wheel", "scroll the half under the pointer"),
        ]);
        sheet
    }

    /// Go to the chosen pane of the selected VM, putting the panel away: Enter on it in the
    /// details, or a second click on it. Whether there was one to go to.
    fn vm_jump(&mut self) -> bool {
        let Some(lease) = self.vms.selected_lease() else {
            return false;
        };
        let Some(pane) = self.vms.pane.and_then(|index| lease.panes.get(index)) else {
            return false;
        };
        let session = SessionName::from(lease.entry.session.as_str());
        let id = TerminalId::new(pane.id);
        let current = self.vms.snapshot.current_session.as_deref() == Some(session.as_str());
        self.hide_the_panel();
        if current {
            focus_terminal_pane(id, false, false);
        } else {
            switch_session_with_focus(&session, None, Some(PaneId::Terminal(id)));
        }
        true
    }

    fn vm_step(&mut self, delta: isize) {
        if let Some(index) = self.vms.pane.filter(|_| self.vms.details) {
            let count = self
                .vms
                .selected_lease()
                .map(|lease| lease.panes.len())
                .unwrap_or_default();
            self.vms.pane = Some(
                index
                    .saturating_add_signed(delta)
                    .min(count.saturating_sub(1)),
            );
            self.vms.reveal_pane = true;
        } else if self.vms.details {
            self.vms.scroll = self
                .vms
                .scroll
                .saturating_add_signed(delta)
                .min(self.vms.max_scroll);
        } else {
            let previous = self.vms.selected;
            self.vms.selected = self.vms.selected.saturating_add_signed(delta).min(
                (self.vms.snapshot.vms.len() + self.vms.snapshot.recoveries.len())
                    .saturating_sub(1),
            );
            if self.vms.selected != previous {
                self.vms.pane = None;
            }
            self.vms.scroll = 0;
            self.vms.list_scroll = None;
        }
    }
    pub(super) fn vm_mouse(&mut self, event: Mouse, keys_were_here: bool) -> bool {
        match event {
            Mouse::LeftClick(line, _) if line >= 0 => {
                let line = line as usize;
                if line < self.vms.last_list_rows {
                    self.vm_list_click(line);
                    return true;
                }
                // The status row of a pane too short to split is not a half to focus.
                if self.vms.last_details_rows == 0 {
                    return false;
                }
                let was_here = std::mem::replace(&mut self.vms.details, true) && keys_were_here;
                // The divider only takes the keys; a line under it may be one of the panes.
                let Some(row) = (line - self.vms.last_list_rows)
                    .checked_sub(1)
                    .map(|line| self.vms.scroll + line)
                else {
                    return true;
                };
                if let Some(index) = self
                    .vms
                    .pane_rows
                    .iter()
                    .position(|rows| rows.contains(&row))
                {
                    // The first click points and the second acts, as a second Enter does: a
                    // click that went straight to a terminal would walk the user out of the
                    // panel on what may have been a click meant to give the details the keys.
                    if was_here && self.vms.pane == Some(index) && self.vm_jump() {
                        return true;
                    }
                    self.vms.pane = Some(index);
                }
            },
            Mouse::ScrollUp(lines) => return self.vm_wheel(-(lines.max(1) as isize)),
            Mouse::ScrollDown(lines) => return self.vm_wheel(lines.max(1) as isize),
            _ => return false,
        }
        true
    }
    /// A click on list row `line`: the row it lands on is selected, and the keys go to the
    /// list.
    fn vm_list_click(&mut self, line: usize) {
        self.vms.details = false;
        let displayed = self.vms.window + line;
        let count = self.vms.snapshot.vms.len();
        let disks = !self.vms.snapshot.recoveries.is_empty();
        // The retained-disk group header is display-only, and so is the space below the list.
        if displayed >= self.vms.list_len() || (disks && displayed == count) {
            return;
        }
        let index = displayed - usize::from(disks && displayed > count);
        if index != self.vms.selected {
            self.vms.selected = index;
            self.vms.scroll = 0;
            self.vms.pane = None;
            // The question was about the row just left, and is drawn only there: kept, it
            // would be answered by the next key meant for this one.
            self.cancel_vm_confirmation();
        }
        self.vms.list_scroll = None;
    }
    /// The wheel scrolls the half under the pointer, as on Agents: the list's window over the
    /// list, the details over the divider or the details.
    fn vm_wheel(&mut self, delta: isize) -> bool {
        // on_mouse has checked the bounds and excluded the tab row above the content.
        let over_list = match self
            .mouse_position
            .map(|(line, _)| (line as usize).saturating_sub(1))
        {
            Some(line) if line < self.vms.last_list_rows => true,
            Some(line)
                if self.vms.last_details_rows > 0
                    && line <= self.vms.last_list_rows + self.vms.last_details_rows =>
            {
                false
            },
            Some(_) => return false,
            None => !self.vms.details,
        };
        if over_list {
            let max = self.vms.list_len().saturating_sub(self.vms.last_list_rows);
            let start = self.vms.list_scroll.unwrap_or(self.vms.window).min(max);
            let next = start.saturating_add_signed(delta).min(max);
            if next == start {
                return false;
            }
            self.vms.list_scroll = Some(next);
            return true;
        }
        let next = self
            .vms
            .scroll
            .saturating_add_signed(delta)
            .min(self.vms.max_scroll);
        let moved = next != self.vms.scroll;
        self.vms.scroll = next;
        moved
    }

    /// What is happening, or last happened, to the lease `identity` at the user's request:
    /// running, queued, or the latest outcome under a header naming it.
    fn vm_outcome_lines(&self, identity: &Identity, now: i64) -> Vec<String> {
        if let Some(pending) = self.vms.pending.as_ref().filter(|pending| {
            pending.operation != Operation::List && pending.target.as_ref() == Some(identity)
        }) {
            // In place of the last outcome: it was the answer to an earlier request, and
            // reading it under this one's name is how an old "sync completed" passed for
            // a new one.
            return vec![format!("{} running…", capitalized(noun(pending.operation)))];
        }
        if let Some((operation, _)) = self
            .vms
            .queued
            .as_ref()
            .filter(|(_, target)| target == identity)
        {
            return vec![match self.vms.busy {
                true => format!(
                    "{} queued until hocus finishes the request still running…",
                    capitalized(noun(*operation))
                ),
                false => format!(
                    "{} queued behind the list refresh…",
                    capitalized(noun(*operation))
                ),
            }];
        }
        let Some(outcome) = self
            .vms
            .results
            .iter()
            .find(|outcome| outcome.identity == *identity)
        else {
            return vec![];
        };
        let mut lines = vec![format!("{}:", outcome_title(outcome, now, false))];
        lines.extend(outcome.text.lines().map(str::to_owned));
        lines
    }

    fn vm_details(&self) -> Details {
        let now = now_ms();
        let mut lines = vec![];
        let mut panes = 0..0;
        let selected = self.vms.selected_lease();
        // The latest outcome for some other lease. Up top only when that lease is gone - a
        // close, whose answer has nowhere else to be - and at the foot otherwise, where it
        // stays out of the way of the lease that is selected; selecting its own lease brings
        // it to the top.
        let other =
            self.vms.results.last().filter(|outcome| {
                selected.is_none_or(|lease| lease.identity() != outcome.identity)
            });
        let other_lines = other.map(|outcome| {
            let mut block = vec![format!("{}:", outcome_title(outcome, now, true))];
            block.extend(outcome.text.lines().map(str::to_owned));
            block
        });
        let other_gone = other.is_some_and(|outcome| {
            !self
                .vms
                .snapshot
                .vms
                .iter()
                .any(|lease| lease.identity() == outcome.identity)
        });
        let (mut top, foot) = match other_gone {
            true => (other_lines, None),
            false => (None, other_lines),
        };
        if let Some(lease) = selected {
            let identity = lease.identity();
            let sharing: Vec<_> = self
                .vms
                .snapshot
                .vms
                .iter()
                .filter(|other| other.entry.session != lease.entry.session && shared(lease, other))
                .map(|other| other.entry.session.as_str())
                .collect();
            if let Some((op, target)) = &self.vms.confirming {
                if *target == identity {
                    lines.extend(vm_question(*op, lease, &sharing));
                }
            }
            // Before everything that does not change when a key is pressed, so the answer to
            // the key is what the details open on.
            lines.extend(self.vm_outcome_lines(&identity, now));
            lines.extend(top.take().into_iter().flatten());
            // The first of the lines that stay put names the lease in full, which its list row
            // may have had room for only the end of.
            lines.push(format!(
                "VM: {} · session {}",
                lease.entry.name, lease.entry.session
            ));
            if let Some(error) = &lease.panes_error {
                lines.push(error.clone());
            }
            let start = lines.len();
            for (index, pane) in lease.panes.iter().enumerate() {
                lines.push(format!(
                    "{} {} {}: {}",
                    if self.vms.pane == Some(index) {
                        ">"
                    } else {
                        " "
                    },
                    if pane.agent { "agent" } else { "terminal" },
                    pane.id,
                    pane.title
                ));
            }
            panes = start..lines.len();
            lines.extend([
                format!(
                    "Image: {}   Branch: {}",
                    lease.entry.image.as_deref().unwrap_or("—"),
                    lease.branch.as_deref().unwrap_or("—")
                ),
                format!(
                    "Worktree: {}",
                    lease
                        .entry
                        .worktree
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "—".into())
                ),
                format!(
                    "Shared with: {}",
                    if sharing.is_empty() {
                        "—".into()
                    } else {
                        sharing.join(", ")
                    }
                ),
            ]);
            let metrics = lease.entry.metrics.as_ref();
            lines.push(format!(
                "Host disk: {} allocated · {} virtual",
                bytes(metrics.and_then(|m| m.disk_allocated_bytes)),
                bytes(metrics.and_then(|m| m.disk_logical_bytes))
            ));
            lines.push(format!(
                "{}: {}",
                metrics
                    .and_then(|m| m.memory_source.as_deref())
                    .unwrap_or("Memory"),
                bytes(metrics.and_then(|m| m.memory_bytes))
            ));
            lines.push(format!(
                "Host CPU: {} (100% = one core{})",
                percent(lease.cpu_percent),
                if sharing.is_empty() {
                    ""
                } else {
                    "; shared measurement"
                }
            ));
        } else if let Some(disk) = self
            .vms
            .selected
            .checked_sub(self.vms.snapshot.vms.len())
            .and_then(|index| self.vms.snapshot.recoveries.get(index))
        {
            lines.extend(top.take().into_iter().flatten());
            // The provider deletes a retained disk on its own schedule, and this panel's
            // own listing is one of the calls that prunes it: the date is the one thing here
            // the user has to act on, so it is said, from the provider's own number.
            let days = vms::RECOVERY_RETENTION_SECS / 86_400;
            lines.extend([
                format!("Reason: {}", disk.reason),
                format!("Retained at: {}", utc(disk.retained_at)),
                format!(
                    "Worktree: {}",
                    disk.worktree
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "—".into())
                ),
                format!("Recovery path: {}", disk.path.display()),
                format!(
                    "Deleted automatically on {} ({days} days after retention). Copy what you \
                     need out of the recovery path before then; this panel has no action for \
                     it.",
                    utc(disk
                        .retained_at
                        .saturating_add(vms::RECOVERY_RETENTION_SECS))
                ),
            ]);
        }
        lines.extend(top.into_iter().flatten());
        lines.extend(foot.into_iter().flatten());
        Details { lines, panes }
    }

    /// The details as drawn at `cols`: wrapped rows, and the rows each pane line took.
    fn vm_detail_rows(&self, cols: usize) -> (Vec<String>, Vec<Range<usize>>) {
        let Details {
            mut lines,
            mut panes,
        } = self.vm_details();
        if !self.vms.error.is_empty() {
            lines.insert(
                0,
                format!("{}: {}", self.vms.error_prefix(), self.vms.error.trim()),
            );
            panes = panes.start + 1..panes.end + 1;
        }
        // Wrapped a line at a time, so each line's first row is known: that is how a pane line
        // is found again, by where it is rather than by what it happens to start with.
        let mut wrapped = vec![];
        let mut starts = Vec::with_capacity(lines.len() + 1);
        for line in &lines {
            starts.push(wrapped.len());
            wrapped.extend(hard_wrap(line, cols));
        }
        starts.push(wrapped.len());
        let pane_rows = panes
            .map(|index| starts[index]..starts[index + 1])
            .collect();
        (wrapped, pane_rows)
    }

    /// What the list says with nothing to list, which is where the tab says what state it is
    /// in: a listing that failed, one not yet asked for or not yet answered, no hocus to ask,
    /// or genuinely no VMs - with how one is made.
    fn vm_placeholder(&self) -> &str {
        if !self.vms.error.is_empty() {
            return self.vms.error_line();
        }
        if self.vms.owner.is_none() {
            return NO_OWNER;
        }
        if !self.vms.loaded {
            return match self.vms.visible {
                false => "Waiting for hocus…",
                true => "Loading VMs…",
            };
        }
        "No Firecracker VMs. Start one from New agent (Ctrl e n) with Run in Firecracker."
    }

    /// The divider's label: the question being asked, then whatever is keeping the tab from
    /// doing what a key would, then the keys.
    ///
    /// The user's own request comes before a stale listing, because the request is what they
    /// are waiting on and the staleness is still said in the details and by the tab's `!`.
    /// `short` is the status row of a pane too short to split, which has no details to show
    /// an outcome in and so carries the latest one itself.
    ///
    /// Every label leads with what a narrow row cannot do without and names the lease last:
    /// `divider_line` cuts from the tail, and a generated VM name alone is 16-30 columns - as
    /// much as a pane at its default half has for the whole label.
    fn vm_divider_label(&self, cols: usize, short: bool) -> String {
        let (above, below) = match short {
            true => (0, 0),
            false => (
                self.vms.scroll,
                self.vms.max_scroll.saturating_sub(self.vms.scroll),
            ),
        };
        // What `divider_line` leaves the label: the rule's ends, the scroll readout, and the
        // focus mark `render_divider` puts in front of the label.
        let room = cols
            .saturating_sub(4 + FOCUS_MARK_COLS)
            .saturating_sub(scroll_readout(above, below).width());
        if let Some((operation, target)) = &self.vms.confirming {
            if self
                .vms
                .selected_lease()
                .is_some_and(|lease| lease.identity() == *target)
            {
                // The answer keys first, since they are what a narrow row keeps, and the
                // question after them - the details under the divider say it in full. Any other
                // key cancels, of which n is the one to name.
                return match operation {
                    Operation::Close => format!("y close · n cancel · Close {}?", target.name),
                    _ => format!("y sync · n cancel · Sync {} to host?", target.name),
                };
            }
        }
        // While the tab bar has the keys the row names the bar's keys, as it does in place of
        // the list's below, rather than a missing owner's complaint: that is no answer the user
        // is waiting on, the bar's keys work without an owner, and the row says it again once
        // the keys are back on the list - an empty list's placeholder says it all along.
        let bar = self.bar_has_keys() && !self.box_ahead_of_the_bar();
        if self.vms.owner.is_none() && !bar {
            return NO_OWNER_DIVIDER.to_owned();
        }
        let now = now_ms();
        let empty = self.vms.list_len() == 0;
        let status = if let Some((operation, target)) = &self.vms.queued {
            Some(format!(
                "Queued: {} · {} / {}",
                noun(*operation),
                target.session,
                target.name
            ))
        } else if let Some(pending) = self
            .vms
            .pending
            .as_ref()
            .filter(|pending| pending.operation != Operation::List)
        {
            Some(running(pending, self.vms.pending_started_ms, now, false))
        } else if let Some((request, started)) = self
            .vms
            .timed_out_operations
            .last()
            .filter(|_| self.vms.busy)
        {
            Some(running(request, *started, now, true))
        } else if !self.vms.error.is_empty() {
            // An empty list names the reason on its own row already.
            Some(match empty {
                true => format!("{} · Ctrl-r retry", self.vms.error_prefix()),
                false => format!("{}: {}", self.vms.error_prefix(), self.vms.error_line()),
            })
        } else {
            None
        };
        if let Some(status) = status {
            // The status leads, and while the bar has the keys the bar's follow it, as they
            // follow a problem on the Todos divider: this row is the only one on the tab that
            // names keys, so a stale list or a sync still running would otherwise leave the
            // bar's keys named nowhere. Cut from the end, the bar's keys first.
            if !bar {
                return status;
            }
            let segments: Vec<&str> = std::iter::once(status.as_str())
                .chain(tabs::BAR_HINT.split(" · "))
                .collect();
            return agents::fit_hints(&segments, room);
        }
        let mut segments: Vec<String> = vec![];
        if short {
            // The selected lease's latest outcome, or - named, since it is not the selected
            // one - that of a lease no longer listed, which is what a close leaves.
            let own = self.vms.selected_lease().and_then(|lease| {
                let identity = lease.identity();
                self.vms
                    .results
                    .iter()
                    .find(|outcome| outcome.identity == identity)
            });
            let gone = || {
                self.vms.results.last().filter(|outcome| {
                    !self
                        .vms
                        .snapshot
                        .vms
                        .iter()
                        .any(|lease| lease.identity() == outcome.identity)
                })
            };
            let outcome = own.map(|outcome| (outcome, String::new())).or_else(|| {
                gone().map(|outcome| {
                    let lease = &outcome.identity;
                    (outcome, format!(" of {} / {}", lease.session, lease.name))
                })
            });
            if let Some((outcome, of)) = outcome {
                // Shorter than the details' header: the row is all there is, and the answer is
                // the part worth its width - for a failure, the reason rather than the CLI's
                // `VM command failed (exit N):`, which says only what "failed" already does.
                segments.push(format!(
                    "{}{of}{}: {}",
                    capitalized(noun(outcome.operation)),
                    if outcome.failed { " failed" } else { "" },
                    match outcome.failed {
                        true => reason_line(&outcome.text),
                        false => first_line(&outcome.text),
                    }
                ));
            }
        }
        let status = segments.len();
        if bar {
            // The next key is the bar's, so the row that names keys names the bar's - here,
            // rather than on a footer row that would take one from the list or the details.
            segments.extend(tabs::BAR_HINT.split(" · ").map(str::to_owned));
        } else {
            segments.extend(self.vm_hints(short).into_iter().map(str::to_owned));
        }
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        // With no status ahead of them, a key too wide for the row is left out rather than
        // cut: `Enter fo` names no key, and the next one may fit whole. A status is cut
        // if it must be, since part of an answer is still worth the row.
        let first = match status {
            0 => segments
                .iter()
                .position(|segment| segment.width() <= room)
                .unwrap_or(segments.len()),
            _ => 0,
        };
        // Room made for `? keys` by giving up trailing keys, but never the status or the key
        // after it: the sheet `?` opens lists the rest.
        agents::fit_hints_marked(&segments[first..], room, status + 1)
    }

    /// The keys worth naming for what is selected and which half has the keys, the selection's
    /// own verb first.
    fn vm_hints(&self, short: bool) -> Vec<&'static str> {
        let Some(lease) = self.vms.selected_lease() else {
            // A retained disk, or nothing: none of a lease's keys do anything here.
            return match self.vms.details {
                true => vec!["Esc list", "↑↓ scroll", "Ctrl-r refresh"],
                false => vec!["Ctrl-r refresh"],
            };
        };
        let mut hints = vec![];
        match (self.vms.details, self.vms.pane.is_some()) {
            // Agents' word for going to a pane, short enough for a narrow divider; the sheet
            // says it in full.
            (true, true) => hints.extend(["Enter focus", "Esc list", "↑↓ pane"]),
            (true, false) => {
                if !lease.panes.is_empty() {
                    hints.push("Enter panes");
                }
                hints.extend(["Esc list", "↑↓ scroll"]);
            },
            (false, _) if short => {},
            (false, _) if lease.panes.is_empty() => hints.push("Enter details"),
            (false, _) => hints.push("Enter panes"),
        }
        hints.push("x close");
        if lease.entry.worktree.is_some() {
            hints.extend(["s sync", "c changes"]);
        }
        if self.vms.details {
            hints.push("PgUp/PgDn scroll");
        }
        hints.push("Ctrl-r refresh");
        if short {
            // Why Enter does nothing here.
            hints.push("more rows show details");
        }
        hints
    }

    pub(super) fn render_vms(&mut self, rows: usize, cols: usize) {
        let empty = self.vms.list_len() == 0;
        // Nothing to split for: an empty list with nothing to say under it has no details, and
        // no ring stop for them either. An error or an outcome keeps the split, since the
        // details are where those are told - after closing the last VM, the only place.
        let quiet = empty && self.vms.error.is_empty() && self.vms.results.is_empty();
        let (mut list_rows, mut detail_rows) = match quiet {
            true => (rows, 0),
            false => split(rows),
        };
        if self.vms.confirming.is_some() && detail_rows == 0 && rows > 1 {
            list_rows = 0;
            detail_rows = rows - 1;
        }
        // A pane too short to split still keeps a row for the divider's label: it is the only
        // place on this tab that an error, a running request, an outcome or a key is told. A
        // quiet list gets one too while the panel's tab bar has the keys, the row that says
        // what they do there - taken from the placeholder, which has rows to spare, as the
        // panel's footer took one for it before this row said it (`footer_hint`).
        let bar = self.bar_has_keys() && !self.box_ahead_of_the_bar();
        let status_row = detail_rows == 0 && (!quiet || bar) && rows >= 2;
        if status_row {
            list_rows -= 1;
        }
        self.vms.last_list_rows = list_rows;
        self.vms.last_details_rows = detail_rows;
        self.vms.pane_rows.clear();
        if detail_rows == 0 {
            self.vms.details = false;
            self.vms.max_scroll = 0;
        }
        match empty {
            true => self.render_vm_placeholder(list_rows, cols),
            false => self.render_vm_list(list_rows, cols),
        }
        if status_row {
            let label = self.vm_divider_label(cols, true);
            render_divider(&label, 0, 0, false, list_rows, cols);
            return;
        }
        if detail_rows == 0 {
            return;
        }
        let (wrapped, pane_rows) = self.vm_detail_rows(cols);
        self.vms.max_scroll = wrapped.len().saturating_sub(detail_rows);
        if std::mem::take(&mut self.vms.reveal_pane) {
            if let Some(row) = self.vms.pane.and_then(|index| pane_rows.get(index)) {
                // As little as shows the pane's first row, so what is above it - the question,
                // the outcome - stays in view where there is room for both.
                if row.start < self.vms.scroll {
                    self.vms.scroll = row.start;
                } else if row.end > self.vms.scroll + detail_rows {
                    self.vms.scroll = (row.end - detail_rows).min(row.start);
                }
            }
        }
        self.vms.pane_rows = pane_rows;
        self.vms.scroll = self.vms.scroll.min(self.vms.max_scroll);
        // Worked out after the details, whose length is what the readout at its end counts.
        let label = self.vm_divider_label(cols, false);
        render_divider(
            &label,
            self.vms.scroll,
            self.vms.max_scroll.saturating_sub(self.vms.scroll),
            self.vm_details_have_the_keys(),
            list_rows,
            cols,
        );
        for (index, line) in wrapped
            .iter()
            .skip(self.vms.scroll)
            .take(detail_rows)
            .enumerate()
        {
            paint(Text::new(line), 0, list_rows + 1 + index, Some(cols), None);
        }
    }

    /// Whether a key pressed now reaches the details under the divider, which is what the
    /// divider's `▸` and colour claim: the details' stop, and the keys really here - not on the
    /// panel's tab bar, and not in another pane.
    fn vm_details_have_the_keys(&self) -> bool {
        self.vms.details && self.keys_here()
    }

    fn render_vm_placeholder(&mut self, list_rows: usize, cols: usize) {
        self.vms.window = 0;
        let error = !self.vms.error.is_empty();
        // No marker and no band: it is not a row anything can be done to. Wrapped by word,
        // since it is the whole of what the tab has to say.
        let chars: Vec<_> = self
            .vm_placeholder()
            .chars()
            .map(|ch| markdown::StyledChar {
                ch,
                style: markdown::MdStyle::default(),
            })
            .collect();
        for (index, line) in markdown::word_wrap_styled(&chars, cols.max(1))
            .into_iter()
            .take(list_rows)
            .enumerate()
        {
            let text = Text::new(line.into_iter().map(|ch| ch.ch).collect::<String>());
            let text = match error {
                true => text.error_color_range(..),
                false => text.dim_range(..),
            };
            paint(text, 0, index, Some(cols), None);
        }
    }

    /// The list's lease rows as drawn in `cols`, less the marker column `render_vm_list` puts
    /// in front of each.
    ///
    /// The VM's name leads, then its session: the name is what tells one lease from another,
    /// and a list of one session's VMs whose rows each began with that session showed the same
    /// words on every row and cut off the only part that differed. Where the pair does not fit,
    /// the name alone does; where not even that, its tail, since generated names differ only
    /// at the end. The state always shows, and CPU and memory where there is room.
    fn vm_rows(&self, cols: usize) -> Vec<String> {
        self.vms
            .snapshot
            .vms
            .iter()
            .map(|lease| {
                let shared_mark =
                    if self.vms.snapshot.vms.iter().any(|other| {
                        other.entry.session != lease.entry.session && shared(lease, other)
                    }) {
                        " shared"
                    } else {
                        ""
                    };
                let name = &lease.entry.name;
                let identity = format!("{name} / {}", lease.entry.session);
                // The marker and its space, and the gap before the state.
                let fixed = lease.entry.state.width() + shared_mark.width() + 4;
                let cpu = format!("  {}", percent(lease.cpu_percent));
                let memory = format!(
                    "  {}",
                    bytes(lease.entry.metrics.as_ref().and_then(|m| m.memory_bytes))
                );
                let room = cols.saturating_sub(fixed + identity.width());
                let extra = if cpu.width() + memory.width() <= room {
                    format!("{cpu}{memory}")
                } else if cpu.width() <= room {
                    cpu
                } else {
                    String::new()
                };
                let budget = cols.saturating_sub(fixed + extra.width());
                let label = if identity.width() <= budget {
                    identity.as_str()
                } else if name.width() <= budget {
                    name.as_str()
                } else {
                    tail_to_width(name, budget)
                };
                format!("{label}  {}{shared_mark}{extra}", lease.entry.state)
            })
            .collect()
    }

    fn render_vm_list(&mut self, list_rows: usize, cols: usize) {
        let mut lines = self.vm_rows(cols);
        let header = (!self.vms.snapshot.recoveries.is_empty()).then_some(lines.len());
        if header.is_some() {
            // Says the deadline where every disk is listed, not only once one is selected.
            let days = vms::RECOVERY_RETENTION_SECS / 86_400;
            let full = format!("Retained disks · deleted after {days} days");
            lines.push(match full.width() <= cols {
                true => full,
                false => "Retained disks".to_owned(),
            });
            lines.extend(self.vms.snapshot.recoveries.iter().map(|disk| {
                format!(
                    "{} / {}: {}",
                    disk.session.as_deref().unwrap_or("—"),
                    disk.name.as_deref().unwrap_or("—"),
                    disk.reason
                )
            }));
        }
        let selected = self.vms.selected
            + usize::from(header.is_some_and(|header| self.vms.selected >= header));
        // Keyboard navigation centres the selection; the wheel moves only the window.
        let max = lines.len().saturating_sub(list_rows);
        let window = self
            .vms
            .list_scroll
            .unwrap_or_else(|| selected.saturating_sub(list_rows / 2))
            .min(max);
        if self.vms.list_scroll.is_some() {
            self.vms.list_scroll = Some(window);
        }
        self.vms.window = window;
        for (index, line) in lines.iter().enumerate().skip(window).take(list_rows) {
            let text = if Some(index) == header {
                // A header takes no marker column, as the Agents project headers do: nothing
                // can land on it.
                Text::new(line).color_range(0, ..)
            } else {
                let text = Text::new(format!(
                    "{} {line}",
                    if index == selected { ">" } else { " " }
                ));
                match index == selected {
                    true => text.selected(),
                    false => text,
                }
            };
            paint(text, 0, index - window, Some(cols), None);
        }
    }
}

/// The close or sync question, saying what `y` will do - and, for a close, what it takes with
/// it. `sharing` is the other sessions holding the same VM.
fn vm_question(operation: Operation, lease: &vms::Lease, sharing: &[&str]) -> Vec<String> {
    let name = &lease.entry.name;
    let session = &lease.entry.session;
    if operation != Operation::Close {
        let Some(worktree) = lease.entry.worktree.as_ref() else {
            return vec!["No host worktree is linked to this VM; no files will be copied.".into()];
        };
        let worktree = worktree.display();
        return vec![
            format!(
                "Merge {name}'s guest changes into host worktree {worktree} (session \
                 {session})? y/n"
            ),
            "Host copy wins conflicts (guest copy saved as .guest); nothing is closed.".into(),
        ];
    }
    let panes = if lease.panes_error.is_some() {
        "Pane list unavailable; matching panes will still be closed.".to_owned()
    } else if lease.panes.is_empty() {
        "No panes to close.".to_owned()
    } else {
        let agents = lease.panes.iter().filter(|pane| pane.agent).count();
        format!(
            "Closes {}{} listed below, then releases this lease.",
            plural(lease.panes.len(), "pane"),
            match agents {
                0 => String::new(),
                agents => format!(" ({})", plural(agents, "agent")),
            }
        )
    };
    let after = match (sharing.is_empty(), lease.entry.worktree.is_some()) {
        (true, true) => {
            "No other session holds this VM: it stops and merges its worktree back.".into()
        },
        (true, false) => "No other session holds this VM: it stops and deletes its guest disk. \
                         No host worktree is linked; copy needed files out before closing."
            .into(),
        (false, _) => format!("VM keeps running for: {}", sharing.join(", ")),
    };
    vec![
        format!("Close {name} in session {session}? y/n"),
        panes,
        after,
    ]
}

/// An operation as a sentence names it.
fn noun(operation: Operation) -> &'static str {
    match operation {
        Operation::List => "list",
        Operation::Changes => "changes check",
        Operation::Sync => "sync",
        Operation::Close => "close",
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A request still out, as the divider names it: what, for how long - and, for `still`, one
/// given up on that hocus says it is still running - then to which lease. The lease last,
/// because `divider_line` cuts from the tail and a generated name can fill a narrow row on its
/// own, where the seconds are the part that says the request is alive.
fn running(request: &Request, started_ms: i64, now: i64, still: bool) -> String {
    let doing = match request.operation {
        Operation::List => "Listing VMs",
        Operation::Changes => "Checking changes",
        Operation::Sync => "Syncing",
        Operation::Close => "Closing",
    };
    let seconds = now.saturating_sub(started_ms).max(0) / 1000;
    let elapsed = match seconds {
        s if s < 60 => format!("{s}s"),
        s => format!("{}m {:02}s", s / 60, s % 60),
    };
    let still = if still { ", still running" } else { "" };
    let target = request
        .target
        .as_ref()
        .map(|target| format!(" · {} / {}", target.session, target.name))
        .unwrap_or_default();
    format!("{doing} {elapsed}{still}{target}")
}

/// The header over an outcome: which operation, whether it failed, and how long ago - and, for
/// a lease other than the selected one, which lease.
fn outcome_title(outcome: &Outcome, now: i64, name_it: bool) -> String {
    let of = match name_it {
        true => format!(
            " of {} / {}",
            outcome.identity.session, outcome.identity.name
        ),
        false => String::new(),
    };
    let age = match agents::short_age(now, outcome.at_ms).as_str() {
        "now" => "just now".to_owned(),
        age => format!("{age} ago"),
    };
    let failed = if outcome.failed { "failed, " } else { "" };
    format!("Latest {}{of} ({failed}{age})", noun(outcome.operation))
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        count => format!("{count} {noun}s"),
    }
}

/// Unix seconds as a UTC minute. UTC and saying so, since a plugin has no zone of its own to
/// read and a date a few hours out is worse than one that names its zone.
fn utc(secs: u64) -> String {
    i64::try_from(secs)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("{secs} (Unix seconds)"))
}

// Host output is literal text. Wrapping must preserve punctuation in recovery paths.
fn hard_wrap(text: &str, cols: usize) -> Vec<String> {
    let chars: Vec<_> = text
        .chars()
        .map(|ch| markdown::StyledChar {
            ch,
            style: markdown::MdStyle::default(),
        })
        .collect();
    markdown::hard_wrap_styled(&chars, cols.max(1))
        .into_iter()
        .map(|line| line.into_iter().map(|ch| ch.ch).collect())
        .collect()
}

/// A command's failure as one row can carry it: the provider's own words where the command
/// printed them (the CLI's `Error: …`), rather than the `VM command failed (exit 1):` hocus
/// puts in front of them, which says only what the row's own "failed" or prefix already does;
/// else the first line there is. Never a line break - it goes on the one-row divider.
fn reason_line(text: &str) -> &str {
    text.lines()
        .find_map(|line| line.split_once("Error: ").map(|(_, reason)| reason.trim()))
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| first_line(text))
}

/// The first line of `text` with anything on it, trimmed.
fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// The longest tail of `text` that fits `width` columns: what is kept of a generated VM name,
/// whose neighbours share everything but its end. Nothing is put in place of what was cut, as
/// with `cut_to_width`: the columns are all a list row has.
fn tail_to_width(text: &str, width: usize) -> &str {
    text.char_indices()
        .map(|(index, _)| &text[index..])
        .find(|tail| tail.width() <= width)
        .unwrap_or_default()
}

fn shared(a: &vms::Lease, b: &vms::Lease) -> bool {
    match (&a.entry.vm_id, &b.entry.vm_id) {
        (Some(a), Some(b)) => a == b,
        _ => a.entry.worktree.is_some() && a.entry.worktree == b.entry.worktree,
    }
}
fn bytes(value: Option<u64>) -> String {
    value
        .map(|n| format!("{:.1} GiB", n as f64 / 1_073_741_824.0))
        .unwrap_or_else(|| "—".into())
}
fn percent(value: Option<f64>) -> String {
    value
        .map(|n| format!("{n:.0}%"))
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        let mut state = State {
            surface: Surface::Vms,
            ..Default::default()
        };
        state.vms.owner = Some(PluginId::new(1));
        state.vms.visible = true;
        state.vms.snapshot = serde_json::from_str(r#"{"vms":[{"session":"one","name":"api","state":"running","vm_id":"physical","panes":[{"id":12,"title":"shell","agent":false}]},{"session":"two","name":"api","state":"running","vm_id":"physical"},{"session":"three","name":"api","state":"running","vm_id":"different"}],"recoveries":[{"path":"/keep/disk","reason":"merge failed","retained_at":123}]}"#).unwrap();
        for lease in &mut state.vms.snapshot.vms {
            lease.entry.worktree = Some("/repo".into());
        }
        state.vms.loaded = true;
        state
    }
    fn outcome(identity: Identity, operation: Operation, text: &str) -> Outcome {
        Outcome {
            identity,
            operation,
            failed: false,
            at_ms: now_ms(),
            text: text.into(),
        }
    }
    /// hocus's answer to whatever is pending.
    fn reply(
        state: &State,
        snapshot: Option<Snapshot>,
        output: &str,
        error: &str,
    ) -> ResultMessage {
        let request = state.vms.pending.clone().expect("a request out");
        ResultMessage {
            request_id: request.request_id,
            operation: request.operation,
            target: request.target,
            snapshot,
            output: output.into(),
            error: error.into(),
        }
    }
    fn details(state: &State) -> String {
        state.vm_details().lines.join("\n")
    }
    fn key(bare: BareKey) -> KeyWithModifier {
        KeyWithModifier::new(bare)
    }
    #[test]
    fn vm_visibility_and_results_belong_only_to_the_manifest_overlay() {
        let mut state = state();
        state.vms.owner = None;
        state.vms.visible = false;
        let visibility = |sender| {
            PipeMessage::new(
                PipeSource::Plugin(PluginId::new(sender)),
                vms::VISIBILITY,
                &Some("true".into()),
                &None,
                true,
            )
        };
        state.vm_pipe(&visibility(1));
        assert!(
            !state.vms.visible,
            "messages before verification grant nothing"
        );
        let mut session = SessionInfo {
            is_current_session: true,
            plugins: BTreeMap::from([
                (
                    PluginId::new(1),
                    PluginInfo {
                        location: "zellij:hocus".into(),
                        configuration: launch::overlay_configuration(),
                    },
                ),
                (
                    PluginId::new(2),
                    PluginInfo {
                        location: "zellij:hocus".into(),
                        ..Default::default()
                    },
                ),
            ]),
            panes: PaneManifest {
                panes: std::collections::HashMap::from([(
                    Default::default(),
                    vec![
                        PaneInfo {
                            id: PaneId::Plugin(PluginId::new(1)),
                            is_floating: true,
                            is_suppressed: true,
                            ..Default::default()
                        },
                        PaneInfo {
                            id: PaneId::Plugin(PluginId::new(2)),
                            ..Default::default()
                        },
                    ],
                )]),
            },
            ..Default::default()
        };
        state.vm_sessions(&[session.clone()]);
        assert_eq!(state.vms.owner, Some(PluginId::new(1)));
        state.vm_pipe(&visibility(2));
        state.poll_vms();
        assert!(
            !state.vms.visible,
            "the tiled hocus cannot reveal the hidden overlay"
        );
        assert!(state.vms.pending.is_none());
        state.vm_pipe(&visibility(1));
        assert!(state.vms.visible);
        let request = state.vms.pending.clone().unwrap();
        let result = ResultMessage {
            request_id: request.request_id,
            operation: request.operation,
            target: request.target,
            snapshot: Some(Snapshot::default()),
            output: String::new(),
            error: String::new(),
        };
        let mut message = PipeMessage::new(
            PipeSource::Plugin(PluginId::new(2)),
            vms::RESULT,
            &Some(serde_json::to_string(&result).unwrap()),
            &None,
            true,
        );
        assert_eq!(state.vm_pipe(&message), Some(false));
        assert_eq!(state.vms.snapshot.vms.len(), 3);
        assert!(state.vms.pending.is_some());
        message.source = PipeSource::Plugin(PluginId::new(1));
        assert_eq!(state.vm_pipe(&message), Some(true));
        assert!(state.vms.snapshot.vms.is_empty());

        state.poll_vms();
        session.plugins.remove(&PluginId::new(1));
        state.vm_sessions(&[session.clone()]);
        assert!(state.vms.owner.is_none());
        assert!(!state.vms.visible);
        assert!(state.vms.pending.is_none());
        state.vm_pipe(&visibility(1));
        assert!(!state.vms.visible);
        // The same ID at another location is not the old owner.
        session.plugins.insert(
            PluginId::new(1),
            PluginInfo {
                location: "file:/tmp/hocus.wasm".into(),
                configuration: launch::overlay_configuration(),
            },
        );
        state.vm_sessions(&[session.clone()]);
        assert!(state.vms.owner.is_none());
        session.plugins.get_mut(&PluginId::new(1)).unwrap().location = "zellij:hocus".into();
        state.vm_sessions(&[session.clone()]);
        state.vm_pipe(&visibility(1));
        assert!(
            state.vms.pending.is_some(),
            "the verified owner can resume polling after reload"
        );
        // A second overlay makes ownership ambiguous, even when only one is visible.
        session
            .plugins
            .get_mut(&PluginId::new(2))
            .unwrap()
            .configuration = launch::overlay_configuration();
        session
            .panes
            .panes
            .values_mut()
            .flatten()
            .find(|pane| pane.id == PaneId::Plugin(PluginId::new(2)))
            .unwrap()
            .is_floating = true;
        state.vm_sessions(&[session]);
        assert!(state.vms.owner.is_none());
        assert!(state.vms.pending.is_none());
        state.vm_pipe(&visibility(1));
        assert!(!state.vms.visible);
    }

    #[test]
    fn vm_polling_is_single_flight_and_pauses_when_hidden_or_on_agents() {
        let mut state = state();
        state.poll_vms();
        let id = state.vms.pending.as_ref().unwrap().request_id;
        state.poll_vms();
        assert_eq!(state.vms.pending.as_ref().unwrap().request_id, id);
        state.vms.pending = None;
        state.vms.visible = false;
        state.poll_vms();
        assert!(state.vms.pending.is_none());
        state.vms.visible = true;
        state.surface = Surface::Agents;
        state.poll_vms();
        assert!(state.vms.pending.is_none());
        state.switch_surface(Surface::Vms);
        assert!(state.vms.pending.is_some());
    }
    #[test]
    fn vm_reply_identity_and_request_id_protect_the_last_successful_snapshot() {
        let mut state = state();
        state.poll_vms();
        let request = state.vms.pending.clone().unwrap();
        let mut reply = ResultMessage {
            request_id: request.request_id + 1,
            operation: Operation::List,
            target: None,
            snapshot: Some(Snapshot::default()),
            output: String::new(),
            error: String::new(),
        };
        assert!(!state.accept_vm_result(reply.clone()));
        assert_eq!(state.vms.snapshot.vms.len(), 3);
        reply.request_id = request.request_id;
        reply.snapshot = None;
        reply.error = "provider denied".into();
        assert!(state.accept_vm_result(reply));
        assert_eq!(state.vms.snapshot.vms.len(), 3);
        assert_eq!(state.vms.error, "provider denied");
    }
    #[test]
    fn vm_request_timeout_recovers_hidden_views_without_repeating_operations() {
        for operation in [
            Operation::List,
            Operation::Changes,
            Operation::Sync,
            Operation::Close,
        ] {
            let mut state = state();
            let target =
                (operation != Operation::List).then(|| state.vms.snapshot.vms[0].identity());
            state.vm_request(operation, target.clone());
            let request = state.vms.pending.clone().unwrap();
            let deadline = state.vms.pending_deadline_ms;
            assert!(!state.expire_vm_request(deadline - 1));
            assert_eq!(
                state.vms.pending.as_ref().unwrap().request_id,
                request.request_id
            );
            state.vms.visible = false;
            state.surface = Surface::Agents;
            assert!(state.expire_vm_request(deadline));
            assert!(state.vms.pending.is_none());
            assert!(!state.expire_vm_request(deadline));
            assert_eq!(state.vms.snapshot.vms.len(), 3);
            if let Some(identity) = target {
                // Given up on, not failed: the provider times out on inactivity, and the
                // request may still be running.
                assert_eq!(state.vms.results[0].identity, identity);
                assert!(state.vms.results[0].text.contains("No reply after 60 s"));
                assert!(!state.vms.results[0].failed);
                assert!(!state.vms.has_error());
            } else {
                assert!(state.vms.error.contains("did not answer in 60 s"));
            }
            state.vms.visible = true;
            state.switch_surface(Surface::Vms);
            let fresh = state.vms.pending.clone().unwrap();
            assert_eq!(fresh.operation, Operation::List);
            assert!(fresh.request_id > request.request_id);
            let reply = ResultMessage {
                request_id: request.request_id,
                operation,
                target: request.target,
                snapshot: Some(Snapshot::default()),
                output: "completed; merge warning: kept host copy".into(),
                error: String::new(),
            };
            let mut wrong_target = reply.clone();
            wrong_target.target = None;
            wrong_target.request_id += 10;
            assert!(!state.accept_vm_result(wrong_target));
            assert_eq!(
                state.accept_vm_result(reply.clone()),
                operation != Operation::List
            );
            assert!(!state.accept_vm_result(reply));
            assert!(state.vms.timed_out_operations.is_empty());
            if operation != Operation::List {
                assert_eq!(
                    state.vms.results[0].text,
                    "completed; merge warning: kept host copy"
                );
            }
            assert_eq!(
                state.vms.pending.as_ref().unwrap().request_id,
                fresh.request_id
            );
            assert_eq!(state.vms.snapshot.vms.len(), 3);
        }
    }
    #[test]
    fn vm_delayed_failure_preserves_a_newer_operation_and_requires_matching_identity() {
        let mut state = state();
        state.vms.visible = false;
        let identity = state.vms.snapshot.vms[0].identity();
        state.vm_request(Operation::Sync, Some(identity.clone()));
        let request = state.vms.pending.clone().unwrap();
        state.expire_vm_request(state.vms.pending_deadline_ms);
        state.vm_request(Operation::Changes, Some(identity.clone()));
        let fresh = state.vms.pending.clone().unwrap();
        let mut reply = ResultMessage {
            request_id: request.request_id,
            operation: Operation::Close,
            target: Some(identity.clone()),
            snapshot: None,
            output: String::new(),
            error: "sync failed; guest disk retained".into(),
        };
        assert!(!state.accept_vm_result(reply.clone()));
        reply.operation = Operation::Sync;
        reply.target.as_mut().unwrap().generation = Some("replacement".into());
        assert!(!state.accept_vm_result(reply.clone()));
        reply.target = Some(identity);
        assert!(state.accept_vm_result(reply.clone()));
        assert_eq!(state.vms.results[0].text, reply.error);
        assert!(state.vms.results[0].failed);
        assert!(
            state.vms.has_error(),
            "a failure nobody was looking at marks the tab"
        );
        assert_eq!(
            state.vms.pending.as_ref().unwrap().request_id,
            fresh.request_id
        );
        assert!(!state.accept_vm_result(reply));
    }
    #[test]
    fn vm_timer_redraws_expiry_and_resumes_visible_polling() {
        let mut state = state();
        state.poll_vms();
        let id = state.vms.pending.as_ref().unwrap().request_id;
        state.vms.pending_deadline_ms = 0;
        assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
        assert!(state.vms.error.contains("did not answer"));
        assert!(state.vms.pending.as_ref().unwrap().request_id > id);
        state.vms.pending_deadline_ms = 0;
        state.vms.visible = false;
        // This retained VM instance expires pending work even while another child is selected.
        assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
        assert!(state.vms.pending.is_none());
    }
    /// Each question says what `y` does - what a close takes with it, which way a sync goes -
    /// and stays on the divider while the details are scrolled to read it.
    #[test]
    fn vm_confirmations_name_the_lease_and_do_not_follow_a_changed_selection() {
        let mut state = state();
        state.vm_key(key(BareKey::Char('x')));
        let text = details(&state);
        assert!(text.contains("Close api in session one? y/n"));
        assert!(text.contains("Closes 1 pane listed below, then releases this lease."));
        assert!(text.contains("VM keeps running for: two"));
        assert!(!text.contains("three"));
        state.render(10, 20);
        state.vm_key(key(BareKey::PageDown));
        assert!(state.vms.confirming.is_some());
        assert!(state.vms.scroll > 0);
        assert!(state
            .vm_divider_label(20, false)
            .starts_with("y close · n cancel · Close api?"));
        state.vms.selected = 2;
        state.vm_key(key(BareKey::Char('y')));
        assert!(state.vms.pending.is_none());
        state.vm_key(key(BareKey::Char('x')));
        assert!(details(&state).contains("No other session holds this VM: it stops"));
        state.vm_key(key(BareKey::Esc));
        state.vms.details = false;
        state.vm_key(key(BareKey::Char('s')));
        let text = details(&state);
        assert!(text.contains("guest changes into host worktree"));
        assert!(text.contains("nothing is closed"));
        assert!(!text.contains("Remaining leases") && !text.contains("Closes"));
        assert!(state
            .vm_divider_label(20, false)
            .starts_with("y sync · n cancel · Sync api to host?"));
        state.vm_key(key(BareKey::Char('y')));
        assert_eq!(
            state
                .vms
                .pending
                .as_ref()
                .unwrap()
                .target
                .as_ref()
                .unwrap()
                .session,
            "three"
        );
    }

    #[test]
    fn bare_vms_explain_disk_deletion_and_do_not_offer_worktree_sync() {
        let mut state = state();
        for lease in &mut state.vms.snapshot.vms {
            lease.entry.worktree = None;
        }
        state.vm_key(key(BareKey::Char('x')));
        assert!(details(&state).contains("VM keeps running for: two"));
        assert!(!details(&state).contains("deletes its guest disk"));
        state.vm_key(key(BareKey::Esc));

        state.vms.selected = 2;
        state.vm_key(key(BareKey::Char('x')));
        let text = details(&state);
        assert!(text.contains("deletes its guest disk"));
        assert!(text.contains("copy needed files out before closing"));
        assert!(!text.contains("merges its worktree back"));
        state.vm_key(key(BareKey::Esc));

        for action in ['s', 'c'] {
            state.vm_key(key(BareKey::Char(action)));
            assert!(state.vms.confirming.is_none());
            assert!(state.vms.pending.is_none() && state.vms.queued.is_none());
            assert!(details(&state).contains("No host worktree is linked to this VM"));
        }
        assert!(!state.vm_hints(false).contains(&"s sync"));
        assert!(!state.vm_hints(false).contains(&"c changes"));
        assert!(!state
            .vm_key_sheet()
            .iter()
            .any(|(key, _)| key == "s" || key == "c"));
    }
    #[test]
    fn vm_tabs_preserve_agent_reader_selection_and_both_scroll_positions() {
        let mut state = state();
        state.surface = Surface::Agents;
        assert!(
            state.take_ring_stop(Some(vms::STOP_TABS)),
            "the tab focus needs a redraw"
        );
        state.selected = 7;
        state.history_scroll = 9;
        state.vms.scroll = 4;
        state.vms.selected = 2;
        state.tabs_focused = true;
        state.on_key(key(BareKey::Right));
        assert_eq!(state.surface, Surface::Todos);
        state.on_key(key(BareKey::Right));
        assert_eq!(state.surface, Surface::Vms);
        assert_eq!(state.selected, 7);
        assert_eq!(state.history_scroll, 9);
        state.on_key(key(BareKey::Left));
        assert_eq!(state.surface, Surface::Todos);
        assert_eq!(state.vms.selected, 2);
        assert_eq!(state.vms.scroll, 4);
    }
    #[test]
    fn vm_ring_mouse_offset_and_small_sizes_keep_tabs_reachable() {
        let mut state = state();
        state.render(20, 40);
        let vm_x = state.tab_cells[2].x;
        state.vms.details = true;
        state.on_mouse(Mouse::LeftClick(0, vm_x));
        assert!(
            !state.tabs_focused,
            "a tab click gives the keys to its list"
        );
        assert!(!state.vms.details);
        state.on_key(key(BareKey::Tab));
        assert!(state.vms.details);
        state.on_key(key(BareKey::Tab));
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);
        state.on_key(key(BareKey::Tab).with_shift_modifier());
        assert!(!state.vms.details);
        state.on_key(key(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        state.on_key(key(BareKey::Tab));
        assert!(!state.tabs_focused);
        assert!(!state.vms.details);
        state.on_key(key(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        state.on_mouse(Mouse::LeftClick(2, 0));
        assert_eq!(state.vms.selected, 1);
        assert!(!state.tabs_focused);
        for rows in 1..8 {
            state.render(rows, 5);
            assert_eq!(state.vms.last_details_rows, 0);
        }
    }
    #[test]
    fn a_tab_click_hands_the_keys_to_the_vm_list() {
        let mut state = state();
        state.render(20, 40);
        state.vms.details = true;
        assert!(state.on_mouse(Mouse::LeftClick(0, state.tab_cells[2].x)));
        assert!(!state.tabs_focused);
        assert!(!state.vms.details);
        assert!(state.on_key(key(BareKey::Char('j'))));
        assert_eq!(state.vms.selected, 1);
    }

    /// hocus putting the panel away ends the visit here as well: the keys go back to the list
    /// and an armed question is dropped rather than taking the first key after `Ctrl e a`.
    #[test]
    fn the_owner_hiding_the_panel_ends_the_visit() {
        let mut state = state();
        state.render(20, 40);
        state.vm_key(key(BareKey::Char('x')));
        assert!(state.vms.confirming.is_some() && state.vms.details);
        state.tabs_focused = true;
        let hidden = PipeMessage::new(
            PipeSource::Plugin(PluginId::new(1)),
            vms::VISIBILITY,
            &Some("false".into()),
            &None,
            true,
        );
        state.vm_pipe(&hidden);
        assert!(state.vms.confirming.is_none());
        assert!(!state.vms.details);
        assert!(!state.tabs_focused);
        assert_eq!(state.surface, Surface::Vms);
    }

    /// Leaving the tab drops an armed question too, so it cannot wait there holding the
    /// list's polling.
    #[test]
    fn leaving_the_vm_tab_drops_the_armed_question() {
        let mut state = state();
        state.vm_key(key(BareKey::Char('x')));
        assert!(state.vms.confirming.is_some());
        state.switch_surface(Surface::Todos);
        state.switch_surface(Surface::Vms);
        assert!(state.vms.confirming.is_none());
        assert!(state.vms.pending.is_some(), "and the list polls again");
    }

    /// A click that moves the selection drops a question about the row it left: drawn only
    /// on that row, it would otherwise take the next key meant for this one, and hold the
    /// polling until then.
    #[test]
    fn vm_click_on_another_row_drops_the_armed_confirmation() {
        let mut state = state();
        state.render(20, 40);
        state.vm_key(key(BareKey::Char('x')));
        assert!(state.vms.confirming.is_some());
        assert!(state.on_mouse(Mouse::LeftClick(2, 0)));
        assert_eq!(state.vms.selected, 1);
        assert!(state.vms.confirming.is_none());
        state.on_key(key(BareKey::Down));
        assert_eq!(state.vms.selected, 2);
        state.poll_vms();
        assert!(state.vms.pending.is_some());
    }

    #[test]
    fn vm_mouse_ignores_the_header_and_empty_panes_without_losing_its_place() {
        let mut state = state();
        let identity = state.vms.snapshot.vms[1].identity();
        state.vms.results.push(outcome(
            identity,
            Operation::Changes,
            &"warning\n".repeat(100),
        ));
        state.vms.selected = 1;
        state.vms.details = true;
        state.vms.scroll = 3;
        state.render(20, 40);
        let position = |state: &State| {
            (
                state.vms.selected,
                state.vms.scroll,
                state.vms.details,
                state.vms.pane,
                state.tabs_focused,
            )
        };
        let before = position(&state);
        let dimensions = |state: &State| {
            [
                state.vms.last_list_rows,
                state.vms.last_details_rows,
                state.vms.window,
                state.vms.max_scroll,
            ]
        };
        let visible = dimensions(&state);
        // The wheel scrolls the half under it. This list fits its half, so over it there is
        // nothing to move; over the details, the details scroll.
        state.on_mouse(Mouse::Hover(1, 39));
        assert!(!state.on_mouse(Mouse::ScrollDown(1)));
        assert_eq!(position(&state), before);
        state.on_mouse(Mouse::Hover(19, 39));
        assert!(state.on_mouse(Mouse::ScrollDown(1)));
        assert_eq!(state.vms.scroll, 4);
        assert!(state.on_mouse(Mouse::ScrollUp(1)));
        assert_eq!(position(&state), before);
        for (line, column) in [(0, 0), (-1, 0), (20, 0), (1, 40)] {
            state.on_mouse(Mouse::Hover(line, column));
            assert!(!state.on_mouse(Mouse::ScrollUp(1)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            if line != 0 {
                assert!(!state.on_mouse(Mouse::LeftClick(line, column)));
            }
            assert_eq!(position(&state), before);
        }
        for (rows, cols) in [(1, 40), (0, 40), (20, 0)] {
            state.render(rows, cols);
            assert_eq!((state.last_rows, state.last_cols), (rows, cols));
            assert_eq!(dimensions(&state), [0; 4]);
            state.mouse_position = None;
            assert!(!state.on_mouse(Mouse::ScrollUp(1)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            state.on_mouse(Mouse::Hover(1, 0));
            assert!(!state.on_mouse(Mouse::ScrollUp(1)));
            assert!(!state.on_mouse(Mouse::ScrollDown(1)));
            assert!(!state.on_mouse(Mouse::LeftClick(1, 0)));
            assert_eq!(position(&state), before);

            let header_visible = rows > 0 && cols > 0;
            assert_eq!(state.on_mouse(Mouse::LeftClick(0, 0)), header_visible);
            assert_eq!(state.surface == Surface::Vms, !header_visible);
            let vm_x = state
                .tab_cells
                .iter()
                .find(|cell| cell.target == Surface::Vms)
                .map(|cell| cell.x)
                .unwrap_or(10);
            assert_eq!(state.on_mouse(Mouse::LeftClick(0, vm_x)), header_visible);
            assert_eq!(state.surface, Surface::Vms);
            // A tab click lands on the tab's list; put the keys back in details to go on.
            assert_eq!(state.vms.details, !header_visible);
            state.vms.details = true;

            state.render(20, 40);
            assert_eq!(dimensions(&state), visible);
            assert_eq!(position(&state), before);
        }
    }

    /// Over a list longer than its half the wheel moves the list's window and leaves the
    /// selection and the details alone; a click lands on the row under the pointer and hands
    /// the window back to the selection.
    #[test]
    fn the_wheel_scrolls_the_half_under_the_pointer() {
        let mut state = state();
        let lease = state.vms.snapshot.vms[2].clone();
        for index in 0..20 {
            let mut more = lease.clone();
            more.entry.name = format!("vm{index}");
            state.vms.snapshot.vms.push(more);
        }
        state.render(20, 40);
        assert_eq!(state.vms.window, 0);
        state.on_mouse(Mouse::Hover(1, 0));
        assert!(state.on_mouse(Mouse::ScrollDown(3)));
        state.render(20, 40);
        assert_eq!(state.vms.window, 3);
        assert_eq!((state.vms.selected, state.vms.scroll), (0, 0));
        assert!(state.on_mouse(Mouse::LeftClick(2, 0)));
        assert_eq!(state.vms.selected, 4);
        assert_eq!(state.vms.list_scroll, None);

        let identity = state.vms.snapshot.vms[4].identity();
        state
            .vms
            .results
            .push(outcome(identity, Operation::Changes, &"line\n".repeat(50)));
        state.render(20, 40);
        state.on_mouse(Mouse::Hover(15, 0));
        assert!(state.on_mouse(Mouse::ScrollDown(2)));
        assert_eq!(state.vms.scroll, 2);
        assert_eq!(state.vms.list_scroll, None);
    }

    #[test]
    fn vm_pane_navigation_reveals_the_cursor_after_wrapped_titles() {
        let mut state = state();
        let lease = &mut state.vms.snapshot.vms[0];
        lease.panes[0].title = "long pane title ".repeat(20);
        lease.panes.push(vms::Pane {
            id: 13,
            title: "next".into(),
            agent: false,
        });
        state.render(20, 20);
        state.vm_key(key(BareKey::Enter));
        state.render(20, 20);
        let (first, scroll) = (state.vms.pane_rows[0].start, state.vms.scroll);
        assert!(
            scroll <= first && first < scroll + state.vms.last_details_rows,
            "the first pane's first row is in view"
        );
        state.vm_key(key(BareKey::Down));
        state.render(20, 20);
        assert_eq!(state.vms.pane, Some(1));
        let row = state.vms.pane_rows[1].clone();
        let (scroll, rows) = (state.vms.scroll, state.vms.last_details_rows);
        assert!(scroll > 5, "scrolled past the first pane's wrapped title");
        assert!(scroll <= row.start && row.end <= scroll + rows);
    }
    #[test]
    fn vm_list_focus_controls_navigation_and_enter_with_a_retained_pane_cursor() {
        let mut state = state();
        state.render(20, 40);
        state.vm_key(key(BareKey::Enter));
        state.vm_key(key(BareKey::Tab).with_shift_modifier());
        assert_eq!(state.vms.pane, Some(0));
        assert!(!state.vms.details);
        state.vm_key(key(BareKey::Enter));
        assert!(state.vms.details, "Enter from the list must enter details");
        assert_eq!(state.panel_hides, 0);
        state.take_ring_stop(Some(wire::STOP_LIST));
        assert!(!state.vms.details);
        state.vm_key(key(BareKey::Down));
        assert_eq!(state.vms.selected, 1);
        assert!(state.vms.pane.is_none());
        state.vm_key(key(BareKey::Up));
        state.vm_key(key(BareKey::Enter));
        state.vm_key(key(BareKey::Tab).with_shift_modifier());
        state.vm_key(key(BareKey::Down));
        assert_eq!(state.vms.selected, 1);
        assert!(state.vms.pane.is_none());
    }

    /// Esc goes by which half has the keys - which the divider shows - not by whether a pane
    /// cursor happens to be kept: back to the list from the details, and away from the list.
    #[test]
    fn vm_esc_steps_back_from_details_and_hides_from_the_list() {
        let mut state = state();
        state.render(20, 40);
        state.vm_key(key(BareKey::Tab));
        assert!(state.vms.details && state.vms.pane.is_none());
        state.vm_key(key(BareKey::Esc));
        assert!(!state.vms.details);
        assert_eq!(state.panel_hides, 0);
        state.vm_key(key(BareKey::Esc));
        assert_eq!(state.panel_hides, 1);

        // A lease with no panes: Enter opens its details, and Esc comes back from them.
        state.vms.selected = 1;
        state.render(20, 40);
        state.vm_key(key(BareKey::Enter));
        assert!(state.vms.details);
        state.vm_key(key(BareKey::Esc));
        assert!(!state.vms.details);
        assert_eq!(state.panel_hides, 1);

        // A pane cursor kept across Shift+Tab does not swallow the list's Esc.
        state.vms.selected = 0;
        state.vm_key(key(BareKey::Enter));
        state.vm_key(key(BareKey::Tab).with_shift_modifier());
        assert_eq!(state.vms.pane, Some(0));
        state.vm_key(key(BareKey::Esc));
        assert_eq!(state.panel_hides, 2);
    }

    /// The first click on a pane line points at it, and a second one goes to it, as a second
    /// Enter does - a second click made with the keys already in the details. The click that
    /// brings them back, off the tab bar or from another pane, only points, even at the pane
    /// already chosen.
    #[test]
    fn a_click_picks_a_pane_and_a_second_click_goes_to_it() {
        let two_panes = || {
            let mut state = state();
            state.vms.snapshot.vms[0].panes.push(vms::Pane {
                id: 13,
                title: "next".into(),
                agent: true,
            });
            state.render(20, 40);
            state
        };
        // Under the tab bar, the list and the divider - measured again after every render,
        // since what the tab bar has the keys for can move the rows.
        let second = |state: &State| {
            (1 + state.vms.last_list_rows + 1 + state.vms.pane_rows[1].start) as isize
        };
        let mut state = two_panes();
        assert!(state.on_mouse(Mouse::LeftClick(second(&state), 0)));
        assert!(state.vms.details);
        assert_eq!(state.vms.pane, Some(1));
        assert_eq!(state.panel_hides, 0);

        state.tabs_focused = true;
        state.render(20, 40);
        assert!(state.on_mouse(Mouse::LeftClick(second(&state), 0)));
        assert!(!state.tabs_focused);
        assert_eq!(state.panel_hides, 0, "the click off the bar only points");
        state.render(20, 40);
        assert!(state.on_mouse(Mouse::LeftClick(second(&state), 0)));
        assert_eq!(state.panel_hides, 1);

        // Tab from the details on to the conversation, and a click back from there.
        let mut state = two_panes();
        assert!(state.on_mouse(Mouse::LeftClick(second(&state), 0)));
        assert!(state.vm_key(key(BareKey::Tab)));
        state.has_keyboard = Some(false);
        state.render(20, 40);
        state.on_mouse(Mouse::LeftClick(second(&state), 0));
        assert_eq!(
            state.panel_hides, 0,
            "the click from another pane only points"
        );
        // What the manifest the click makes the host send says.
        state.has_keyboard = Some(true);
        state.render(20, 40);
        assert!(state.on_mouse(Mouse::LeftClick(second(&state), 0)));
        assert_eq!(state.panel_hides, 1);
    }

    #[test]
    fn vm_retained_disks_are_informational_and_warnings_are_scrollable_in_full() {
        let mut state = state();
        let identity = state.vms.snapshot.vms[0].identity();
        let warning = (0..100)
            .map(|i| format!("warning {i}: host and guest differ"))
            .collect::<Vec<_>>()
            .join("\n");
        state
            .vms
            .results
            .push(outcome(identity, Operation::Sync, &warning));
        state.render(20, 30);
        assert!(state.vms.max_scroll > 100);
        assert!(details(&state).contains("warning 99:"));
        state.vms.selected = 3;
        let text = details(&state);
        assert!(text.contains("/keep/disk"));
        // A deadline, from the provider's own retention, in a form a person can read.
        assert!(text.contains("Retained at: 1970-01-01 00:02 UTC"));
        assert!(
            text.contains("Deleted automatically on 1970-01-08 00:02 UTC (7 days after retention)")
        );
        assert!(!text.contains("Unix seconds"));
        state.render(20, 30);
        assert_eq!(state.vm_divider_label(30, false), "Ctrl-r refresh");
        state.vm_key(key(BareKey::Char('x')));
        assert!(state.vms.confirming.is_none());
        assert!(state.vms.pending.is_none());
    }

    /// A background listing no longer eats `c`, `s` or `x`: the question arms as ever, and
    /// the request waits for the listing to answer - and goes only to a lease still listed.
    #[test]
    fn vm_keys_arm_and_queue_behind_a_background_list() {
        let mut state = state();
        state.render(20, 40);
        let one = state.vms.snapshot.vms[0].identity();
        state.poll_vms();
        let list = state.vms.pending.clone().unwrap();
        assert_eq!(list.operation, Operation::List);
        assert!(state.vm_key(key(BareKey::Char('x'))));
        assert!(state.vms.confirming.is_some());
        state.vm_key(key(BareKey::Char('y')));
        assert_eq!(
            state.vms.pending.as_ref().unwrap().request_id,
            list.request_id
        );
        assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));
        state.render(20, 40);
        assert_eq!(
            state.vm_divider_label(40, false),
            "Queued: close · one / api"
        );
        assert!(details(&state).contains("Close queued behind the list refresh"));
        let snapshot = state.vms.snapshot.clone();
        assert!(state.accept_vm_result(reply(&state, Some(snapshot.clone()), "", "")));
        let sent = state.vms.pending.clone().unwrap();
        assert_eq!(
            (sent.operation, sent.target),
            (Operation::Close, Some(one.clone()))
        );
        assert!(state.vms.queued.is_none());

        // `c` asks nothing, so it queues straight away.
        state.vms.pending = None;
        state.poll_vms();
        assert!(state.vm_key(key(BareKey::Char('c'))));
        assert!(state.vms.confirming.is_none());
        assert_eq!(state.vms.queued, Some((Operation::Changes, one.clone())));
        let mut gone = snapshot;
        gone.vms.remove(0);
        assert!(state.accept_vm_result(reply(&state, Some(gone), "", "")));
        assert!(
            state.vms.pending.is_none(),
            "a lease that went away is not asked"
        );
        let last = state.vms.results.last().unwrap();
        assert!(last.identity == one && last.failed);
        assert!(last.text.contains("changed while this waited"));
    }

    /// While a request of the user's own is out the keys that would start another redraw,
    /// and the divider names what is running and for how long.
    #[test]
    fn vm_keys_during_a_user_operation_redraw_and_name_it() {
        let mut state = state();
        let one = state.vms.snapshot.vms[0].identity();
        assert!(state.vm_request(Operation::Sync, Some(one)));
        assert!(state.vm_key(key(BareKey::Char('x'))));
        assert!(state.vms.confirming.is_none());
        state.render(20, 40);
        let label = state.vm_divider_label(40, false);
        assert!(
            label.starts_with("Syncing ") && label.ends_with(" · one / api"),
            "{label}"
        );
        assert!(details(&state).contains("Sync running…"));
        assert!(state.poll_vms(), "the seconds on the divider tick");
    }
    #[test]
    fn vm_background_list_keeps_the_hints() {
        let mut state = state();
        state.poll_vms();
        state.render(20, 40);
        let label = state.vm_divider_label(40, false);
        assert!(label.contains("x close"), "{label}");
        assert!(!state.poll_vms(), "a listing counts nothing");
    }
    /// The hints drop whole keys from the end rather than cutting one in half.
    #[test]
    fn vm_hints_drop_whole_segments() {
        let mut state = state();
        state.render(20, 80);
        let segments = [
            "Enter panes",
            "x close",
            "s sync",
            "c changes",
            "Ctrl-r refresh",
            "? keys",
        ];
        for cols in 17..80 {
            let label = state.vm_divider_label(cols, false);
            assert!(
                label
                    .split(" · ")
                    .all(|segment| segments.contains(&segment)),
                "{cols}: {label}"
            );
        }
        assert_eq!(
            state.vm_divider_label(80, false),
            "Enter panes · x close · s sync · c changes · Ctrl-r refresh"
        );
    }

    /// The answer to a key is what the details open on, the divider counts what is below
    /// them, and host output cannot pass for the pane cursor.
    #[test]
    fn vm_outcomes_open_the_details_and_the_divider_counts_what_is_below() {
        let mut state = state();
        state.render(12, 22);
        let one = state.vms.snapshot.vms[0].identity();
        state.vms.scroll = 2;
        state.vm_request(Operation::Changes, Some(one.clone()));
        let answer = reply(&state, None, "Guest paths changed since sync: 3", "");
        assert!(state.accept_vm_result(answer));
        state.render(12, 22);
        assert_eq!(state.vms.scroll, 0);
        let (wrapped, _) = state.vm_detail_rows(22);
        let shown = wrapped[..state.vms.last_details_rows].join("\n");
        assert!(shown.contains("Latest changes"), "{shown}");
        assert!(shown.contains("Guest paths changed"), "{shown}");
        let below = state.vms.max_scroll - state.vms.scroll;
        assert!(below > 0);
        let label = state.vm_divider_label(22, false);
        assert!(divider_line(&label, 0, below, 22).ends_with(&format!("↓{below} ")));

        // Output that starts like the pane cursor is not taken for it.
        state.vms.results.clear();
        state
            .vms
            .results
            .push(outcome(one.clone(), Operation::Changes, "> not a pane"));
        state.render(20, 40);
        state.vm_key(key(BareKey::Enter));
        state.render(20, 40);
        let (wrapped, pane_rows) = state.vm_detail_rows(40);
        assert!(wrapped[pane_rows[0].start].starts_with("> terminal 12"));
        assert_eq!(
            state.vms.scroll, 0,
            "the pane and the outcome above it both fit"
        );

        // A new request replaces the old answer rather than sitting under it.
        state.vms.results.clear();
        state.vms.results.push(outcome(
            one.clone(),
            Operation::Sync,
            "Worktree sync completed.",
        ));
        state.vms.pending = None;
        assert!(state.vm_request(Operation::Sync, Some(one)));
        let text = details(&state);
        assert!(text.contains("Sync running…"));
        assert!(!text.contains("Worktree sync completed."));
    }

    /// A provider that was never configured is unavailable, not stale: the list row gives the
    /// reason in the provider's words, the divider stays one line, and the tab gets no `!` -
    /// until a listing has loaded and a later one fails.
    #[test]
    fn an_unconfigured_provider_is_unavailable_not_stale() {
        let mut state = State {
            surface: Surface::Vms,
            ..Default::default()
        };
        state.vms.owner = Some(PluginId::new(1));
        state.vms.visible = true;
        state.poll_vms();
        let error = "VM command failed (exit 1): Error: Firecracker is not configured: set \
                     firecracker_provider in your Zellij config to the absolute path of a \
                     provider executable to use VMs\n";
        assert!(state.accept_vm_result(reply(&state, None, "", error)));
        state.render(20, 60);
        assert!(state
            .vm_placeholder()
            .starts_with("Firecracker is not configured: set"));
        let label = state.vm_divider_label(60, false);
        assert_eq!(label, "Unavailable · Ctrl-r retry");
        assert!(!divider_line(&label, 0, 0, 60).contains('\n'));
        assert!(!state.vms.has_error());
        assert!(state.vms.last_details_rows > 0);
        assert!(state
            .vm_detail_rows(60)
            .0
            .join("")
            .starts_with("Unavailable: VM command failed"));

        state.poll_vms();
        let listed: Snapshot =
            serde_json::from_str(r#"{"vms":[{"session":"s","name":"v","state":"running"}]}"#)
                .unwrap();
        assert!(state.accept_vm_result(reply(&state, Some(listed), "", "")));
        assert!(state.vms.error.is_empty());
        state.poll_vms();
        assert!(state.accept_vm_result(reply(&state, None, "", "provider denied\n")));
        assert_eq!(state.vm_divider_label(60, false), "STALE: provider denied");
        assert!(state.vms.has_error());
    }

    /// Past the deadline, hocus answering "busy" is proof the request is alive: the divider
    /// says it is still running rather than calling the list stale, and the late answer is
    /// still the answer.
    #[test]
    fn a_busy_reply_after_a_timeout_is_still_running_not_stale() {
        let mut state = state();
        let one = state.vms.snapshot.vms[0].identity();
        state.vm_request(Operation::Sync, Some(one));
        let sync = state.vms.pending.clone().unwrap();
        assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
        assert_eq!(
            state.vms.pending.as_ref().unwrap().operation,
            Operation::List
        );
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        assert!(state.vms.error.is_empty());
        assert!(!state.vms.has_error());
        state.render(20, 40);
        let label = state.vm_divider_label(40, false);
        assert!(label.starts_with("Syncing "), "{label}");
        assert!(label.contains(", still running · one / api"), "{label}");
        assert!(details(&state).contains("No reply after 60 s"));
        assert!(state.poll_vms(), "the seconds keep counting");

        let late = ResultMessage {
            request_id: sync.request_id,
            operation: sync.operation,
            target: sync.target,
            snapshot: None,
            output: "Worktree sync completed.".into(),
            error: String::new(),
        };
        assert!(state.accept_vm_result(late));
        assert!(!state.vms.busy);
        let text = details(&state);
        assert!(text.contains("Latest sync (just now):"));
        assert!(text.contains("Worktree sync completed."));
    }

    /// A failure that arrives while another tab is showing marks the tab; one that arrives
    /// in front of the user is shown where it happened and marks nothing.
    #[test]
    fn an_off_tab_failure_marks_the_tab_until_it_is_seen() {
        let mut state = state();
        let one = state.vms.snapshot.vms[0].identity();
        state.vm_request(Operation::Sync, Some(one.clone()));
        state.surface = Surface::Agents;
        assert!(state.accept_vm_result(reply(&state, None, "", "sync failed")));
        assert!(state.vms.has_error());
        assert!(details(&state).contains("Latest sync (failed, just now):"));
        state.switch_surface(Surface::Vms);
        assert!(!state.vms.has_error());

        state.vms.pending = None;
        state.vm_request(Operation::Close, Some(one));
        assert!(state.accept_vm_result(reply(&state, None, "", "close failed")));
        assert!(!state.vms.has_error());
    }

    /// Too short to split, the tab still keeps a row for what the divider would say: the
    /// error, the latest answer, the keys - and Enter leaves no invisible pane cursor behind.
    #[test]
    fn a_short_pane_keeps_a_status_row_and_enter_opens_nothing() {
        let mut state = state();
        state.render(7, 40);
        assert_eq!(
            (state.vms.last_list_rows, state.vms.last_details_rows),
            (5, 0)
        );
        assert!(state.vm_divider_label(40, true).starts_with("x close"));
        state.vms.error = "provider denied".into();
        assert_eq!(state.vm_divider_label(40, true), "STALE: provider denied");
        state.vms.error.clear();
        let one = state.vms.snapshot.vms[0].identity();
        state.vms.results.push(outcome(
            one,
            Operation::Changes,
            "Guest paths changed since sync: 3",
        ));
        assert!(state
            .vm_divider_label(40, true)
            .starts_with("Changes check: Guest paths changed"));
        assert!(!state.vm_key(key(BareKey::Enter)));
        assert!(state.vms.pane.is_none() && !state.vms.details);
        assert!(
            !state.on_mouse(Mouse::LeftClick(6, 0)),
            "the status row is not a half"
        );
        state.vm_key(key(BareKey::Esc));
        assert_eq!(state.panel_hides, 1);
    }

    /// With nothing to list the tab does not split - no empty half, no ring stop in it - and
    /// says how a VM is made; the answer to closing the last one keeps the details, since it
    /// has nowhere else to be.
    #[test]
    fn an_empty_list_does_not_split_and_says_how_vms_are_made() {
        let mut state = state();
        let closed = state.vms.snapshot.vms[0].identity();
        state.vms.snapshot = Snapshot::default();
        state.vms.selected = 0;
        state.render(20, 40);
        assert_eq!(state.vms.last_details_rows, 0);
        assert!(state
            .vm_placeholder()
            .starts_with("No Firecracker VMs. Start one from New agent (Ctrl e n)"));
        state.vm_key(key(BareKey::Tab));
        assert!(!state.vms.details);
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);

        state
            .vms
            .results
            .push(outcome(closed, Operation::Close, "Lease released."));
        state.render(20, 40);
        assert!(state.vms.last_details_rows > 0);
        assert!(state
            .vm_detail_rows(40)
            .0
            .join("\n")
            .contains("Lease released."));
        assert_eq!(state.vm_divider_label(40, false), "Ctrl-r refresh");
        // Too short for the details, the status row carries it, naming the lease it was.
        state.render(7, 60);
        assert_eq!(state.vms.last_details_rows, 0);
        assert!(state
            .vm_divider_label(60, true)
            .starts_with("Close of one / api: Lease released."));
    }

    /// Without one hocus to ask, nothing offers a question that cannot be answered, and the
    /// divider says why and what fixes it.
    #[test]
    fn vm_actions_without_an_owner_say_why() {
        let mut state = state();
        state.vms.owner = None;
        state.render(20, 40);
        assert!(state.vm_key(key(BareKey::Char('x'))));
        assert!(state.vms.confirming.is_none());
        assert_eq!(state.vm_divider_label(40, false), NO_OWNER_DIVIDER);
        assert!(state.vm_key(key(BareKey::Char('c'))));
        assert!(state.vm_key(key(BareKey::Char('r')).with_ctrl_modifier()));
        state.vm_key(key(BareKey::Char('y')));
        assert!(state.vms.pending.is_none() && state.vms.queued.is_none());
    }

    /// A fresh tab says what it is waiting on, and once a listing has loaded, the background
    /// polls that follow do not flash "Loading" over an empty list.
    #[test]
    fn a_fresh_vm_tab_waits_for_hocus_then_loads() {
        let mut state = State {
            surface: Surface::Vms,
            ..Default::default()
        };
        assert_eq!(state.vm_placeholder(), NO_OWNER);
        state.vms.owner = Some(PluginId::new(1));
        assert_eq!(state.vm_placeholder(), "Waiting for hocus…");
        state.vms.visible = true;
        state.poll_vms();
        assert_eq!(state.vm_placeholder(), "Loading VMs…");
        assert!(state.accept_vm_result(reply(&state, Some(Snapshot::default()), "", "")));
        state.poll_vms();
        assert!(state.vms.pending.is_some());
        assert!(state.vm_placeholder().starts_with("No Firecracker VMs."));
        // Ctrl-r lists straight away, whatever hocus last said about the panel.
        state.vms.pending = None;
        state.vms.visible = false;
        assert!(state.vm_key(key(BareKey::Char('r')).with_ctrl_modifier()));
        assert!(state.vms.pending.is_some());
    }

    /// The divider marks the details as having the keys - a `▸` as well as its colour - after
    /// Tab into them, and only while a key pressed now would reach them: not while the panel's
    /// tab bar has the keys, and not while another pane does.
    #[test]
    fn tab_into_the_details_marks_the_divider() {
        let mut state = state();
        state.render(20, 40);
        let drawn = |state: &State| {
            divider_text(
                &state.vm_divider_label(40, false),
                0,
                0,
                state.vm_details_have_the_keys(),
                40,
            )
            .content()
            .to_owned()
        };
        assert!(drawn(&state).starts_with("─   "), "{}", drawn(&state));
        assert!(state.vm_key(key(BareKey::Tab)));
        assert!(state.vms.details);
        assert!(drawn(&state).starts_with("─ ▸ "), "{}", drawn(&state));
        state.tabs_focused = true;
        assert!(drawn(&state).starts_with("─   "), "the bar has them");
        state.tabs_focused = false;
        state.own_pane_changed(true, true);
        state.own_pane_changed(true, false);
        assert!(drawn(&state).starts_with("─   "), "another pane has them");
        assert!(
            state.vms.details,
            "the stop is kept for the keys to come back to"
        );
    }

    /// The `?` sheet names every key the tab answers in the mode it is in.
    #[test]
    fn the_vm_key_sheet_follows_the_mode() {
        let mut state = state();
        state.render(20, 40);
        let keys = |state: &State| {
            state
                .vm_key_sheet()
                .into_iter()
                .map(|(keys, _)| keys)
                .collect::<Vec<_>>()
        };
        let what = |state: &State, key: &str| {
            state
                .vm_key_sheet()
                .into_iter()
                .find(|(keys, _)| keys == key)
                .map(|(_, what)| what)
        };
        let list = keys(&state);
        for key in [
            "↑↓ j k",
            "Enter",
            "Esc",
            "x",
            "s",
            "c",
            "PgUp PgDn",
            "Ctrl-r",
            "Tab",
            "Shift+Tab",
            "click",
            "wheel",
        ] {
            assert!(list.iter().any(|keys| keys == key), "{key}");
        }
        assert_eq!(what(&state, "Esc").as_deref(), Some("hide the panel"));
        state.vm_key(key(BareKey::Enter));
        assert_eq!(what(&state, "Esc").as_deref(), Some("back to the list"));
        assert_eq!(
            what(&state, "Enter").as_deref(),
            Some("go to the chosen pane")
        );
        // An armed question stays armed under the sheet, which lists its answers first.
        state.on_key(key(BareKey::Char('x')));
        assert!(state.on_key(key(BareKey::Char('?'))));
        assert_eq!(state.key_sheet_scroll, Some(0));
        assert!(state.vms.confirming.is_some());
        assert_eq!(state.key_sheet()[0].0, "y");
        state.on_key(key(BareKey::Esc));
        assert_eq!(state.key_sheet_scroll, None, "Esc shut the sheet");
        assert!(state.vms.confirming.is_some(), "and left the question");
        state.on_key(key(BareKey::Char('?')));
        state.on_key(key(BareKey::Char('n')));
        assert_eq!(state.key_sheet_scroll, None);
        assert!(state.vms.confirming.is_none());
        // `y` through the sheet answers the question.
        state.on_key(key(BareKey::Char('x')));
        state.on_key(key(BareKey::Char('?')));
        state.on_key(key(BareKey::Char('y')));
        assert_eq!(state.key_sheet_scroll, None);
        assert!(
            state
                .vms
                .pending
                .as_ref()
                .is_some_and(|pending| pending.operation == Operation::Close)
                || state
                    .vms
                    .queued
                    .as_ref()
                    .is_some_and(|(operation, _)| *operation == Operation::Close)
        );
    }

    /// The sheet names only keys that would do something now: none of a lease's on a retained
    /// disk, no details' on a pane with none drawn, nothing that asks hocus without one hocus
    /// to ask - and says what fixes that instead.
    #[test]
    fn the_vm_key_sheet_lists_only_keys_that_act() {
        let keys = |state: &State| {
            state
                .vm_key_sheet()
                .into_iter()
                .map(|(keys, _)| keys)
                .collect::<Vec<_>>()
        };
        let what = |state: &State, key: &str| {
            state
                .vm_key_sheet()
                .into_iter()
                .find(|(keys, _)| keys == key)
                .map(|(_, what)| what)
        };
        let lists = |state: &State, key: &str| keys(state).iter().any(|keys| keys == key);

        let mut disk = state();
        disk.render(20, 40);
        assert!(what(&disk, "c").is_some_and(|what| what.starts_with("count ")));
        disk.vms.selected = 3;
        for (name, bare) in [
            ("Enter", BareKey::Enter),
            ("x", BareKey::Char('x')),
            ("s", BareKey::Char('s')),
            ("c", BareKey::Char('c')),
        ] {
            assert!(!lists(&disk, name), "{name}");
            assert!(!disk.vm_key(key(bare)), "{name}");
        }
        assert!(disk.vms.confirming.is_none() && disk.vms.pending.is_none());

        let mut empty = state();
        empty.vms.snapshot = Snapshot::default();
        empty.vms.selected = 0;
        empty.render(20, 40);
        assert_eq!(empty.vms.last_details_rows, 0);
        for name in ["Enter", "x", "s", "c"] {
            assert!(!lists(&empty, name), "{name}");
        }
        assert!(!keys(&empty).iter().any(|keys| keys.contains("PgUp")));
        assert_eq!(
            what(&empty, "Tab").as_deref(),
            Some("on to the conversation")
        );

        let mut ownerless = state();
        ownerless.vms.owner = None;
        ownerless.render(20, 40);
        for name in ["x", "s", "c", "Ctrl-r"] {
            assert!(!lists(&ownerless, name), "{name}");
        }
        assert!(lists(&ownerless, "Enter"));
        assert!(lists(&ownerless, "Ctrl e c"));
        assert!(ownerless.vm_key(key(BareKey::Char('x'))));
        assert!(ownerless.vms.confirming.is_none());
    }

    /// However narrow the divider, a key on it is whole or not there - one too wide for the
    /// row gives way to the next rather than being cut - and where the first key leaves room,
    /// the row ends by saying `?` has the rest.
    #[test]
    fn vm_divider_never_cuts_a_key_at_narrow_widths() {
        for cols in [22, 24, 28, 34] {
            for rows in [10, 12, 14] {
                let mut state = state();
                for id in 13..18 {
                    state.vms.snapshot.vms[0].panes.push(vms::Pane {
                        id,
                        title: format!("pane {id}"),
                        agent: false,
                    });
                }
                state.render(rows, cols);
                assert!(
                    state.vms.max_scroll > 0,
                    "{cols}x{rows}: a readout is drawn"
                );
                let check = |state: &State| {
                    let label = state.vm_divider_label(cols, false);
                    let hints = state.vm_hints(false);
                    assert!(!label.is_empty(), "{cols}x{rows}");
                    for piece in label.split(" · ") {
                        assert!(
                            piece == "? keys" || hints.contains(&piece),
                            "{cols}x{rows}: {label}"
                        );
                    }
                    let (scroll, max) = (state.vms.scroll, state.vms.max_scroll);
                    let marked = format!("▸ {label}");
                    let drawn = divider_line(&marked, scroll, max - scroll, cols);
                    assert!(drawn.contains(&marked), "{cols}x{rows}: {drawn}");
                    assert!(drawn.width() <= cols, "{cols}x{rows}: {drawn}");
                    label
                };
                let list = check(&state);
                // At 34 the readout leaves room for `Enter panes · ? keys`; at 28 it leaves 17
                // or 18 columns, where `Enter panes` is all there is room for.
                if cols == 34 {
                    assert!(list.ends_with("? keys"), "{rows}: {list}");
                }
                state.vm_key(key(BareKey::Enter));
                state.render(rows, cols);
                check(&state);
                for _ in 0..6 {
                    state.vm_key(key(BareKey::Down));
                    state.render(rows, cols);
                    check(&state);
                }
            }
        }
    }

    /// Set up the way the timeout tests need it: a sync of lease 0 given up on, with the
    /// listing the expiry sent still out.
    fn expired_sync(state: &mut State) -> Request {
        state.render(20, 40);
        let one = state.vms.snapshot.vms[0].identity();
        assert!(state.vm_request(Operation::Sync, Some(one)));
        let sync = state.vms.pending.clone().unwrap();
        assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
        assert_eq!(
            state.vms.pending.as_ref().map(|pending| pending.operation),
            Some(Operation::List)
        );
        sync
    }

    /// A listing given up on is most likely still running in hocus, which refuses anything else
    /// as busy. A close confirmed behind it waits for a listing hocus answers - it used to go
    /// out at the expiry, be refused, and be recorded as a failed close, lost - and a busy
    /// refusal of a request that did go out puts it back in the queue rather than failing it.
    #[test]
    fn a_request_behind_an_expired_listing_waits_for_one_that_answers() {
        let mut state = state();
        state.render(20, 40);
        let one = state.vms.snapshot.vms[0].identity();
        let out = |state: &State| state.vms.pending.as_ref().map(|pending| pending.operation);
        state.poll_vms();
        assert_eq!(out(&state), Some(Operation::List));
        state.vm_key(key(BareKey::Char('x')));
        state.vm_key(key(BareKey::Char('y')));
        assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));

        assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
        assert!(state.vms.list_expired);
        assert_eq!(out(&state), None, "not sent into the listing still running");
        assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));
        // Refused while hocus still runs the first listing: still waiting, nothing failed.
        state.poll_vms();
        assert_eq!(out(&state), Some(Operation::List));
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));
        assert!(state.vms.results.is_empty());
        // A key pressed now waits the same way.
        state.vms.queued = None;
        state.vm_key(key(BareKey::Char('x')));
        state.vm_key(key(BareKey::Char('y')));
        assert_eq!(out(&state), None);
        assert_eq!(state.vms.queued, Some((Operation::Close, one.clone())));

        // A listing hocus answers sends it.
        state.poll_vms();
        let snapshot = state.vms.snapshot.clone();
        assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
        assert!(!state.vms.list_expired);
        assert_eq!(out(&state), Some(Operation::Close));
        assert!(state.vms.queued.is_none());
        // Were hocus busy after all, the close is not a failure: it waits again.
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        assert!(
            state.vms.results.is_empty(),
            "a busy refusal is not a failed close"
        );
        assert_eq!(state.vms.queued, Some((Operation::Close, one)));
    }

    /// An armed close or sync question does not hold the list's polling: it names the panes and
    /// the sharing the list says, which used to freeze under it for as long as the keys were in
    /// the conversation.
    #[test]
    fn an_armed_question_keeps_the_list_polling() {
        let mut state = state();
        state.render(20, 40);
        assert!(state.vm_key(key(BareKey::Char('x'))));
        assert!(state.vms.confirming.is_some());
        state.poll_vms();
        assert_eq!(
            state.vms.pending.as_ref().map(|pending| pending.operation),
            Some(Operation::List)
        );
        let snapshot = state.vms.snapshot.clone();
        assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
        assert!(
            state.vms.confirming.is_some(),
            "a listing that keeps the lease keeps the question"
        );
        assert!(state.vm_key(key(BareKey::Char('y'))));
        assert_eq!(
            state.vms.pending.as_ref().map(|pending| pending.operation),
            Some(Operation::Close)
        );
    }

    /// hocus's late answer to `request`.
    fn late(request: &Request, output: &str) -> ResultMessage {
        ResultMessage {
            request_id: request.request_id,
            operation: request.operation,
            target: request.target.clone(),
            snapshot: None,
            output: output.into(),
            error: String::new(),
        }
    }

    /// While hocus says a request given up on is still running, `c`, `s` and `x` are held as
    /// they are while one is out: sent, they would only be refused. The late reply lets them
    /// go again.
    #[test]
    fn keys_are_held_while_a_timed_out_request_is_busy() {
        let mut state = state();
        let sync = expired_sync(&mut state);
        let one = state.vms.snapshot.vms[0].identity();
        assert!(state.vm_key(key(BareKey::Char('c'))));
        assert_eq!(state.vms.queued, Some((Operation::Changes, one)));
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        assert!(state.vms.busy);
        assert!(state.vms.pending.is_none());
        assert!(state.vms.queued.is_some(), "not sent into a known refusal");

        state.vms.queued = None;
        for c in ['c', 's', 'x'] {
            assert!(state.vm_key(key(BareKey::Char(c))));
            assert!(state.vms.pending.is_none(), "{c}");
            assert!(state.vms.confirming.is_none(), "{c}");
        }
        assert!(details(&state).contains("No reply after 60 s"));
        state.render(20, 40);
        let label = state.vm_divider_label(40, false);
        assert!(label.starts_with("Syncing "), "{label}");

        assert!(state.accept_vm_result(late(&sync, "Worktree sync completed.")));
        let snapshot = state.vms.snapshot.clone();
        assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
        assert!(!state.vms.busy);
        assert!(state.vm_key(key(BareKey::Char('c'))));
        assert_eq!(
            state.vms.pending.as_ref().map(|pending| pending.operation),
            Some(Operation::Changes)
        );
    }

    /// A request queued while hocus is still busy waits out the refusals, and goes once the
    /// late reply is in and a listing answers - `c` pressed behind the listing, and a `y` armed
    /// just before hocus said it was busy.
    #[test]
    fn a_request_queued_during_busy_goes_out_after_the_late_reply() {
        for (press, operation) in [
            (&['c'][..], Operation::Changes),
            (&['x', 'y'][..], Operation::Close),
        ] {
            let mut state = state();
            let sync = expired_sync(&mut state);
            let (arm, answer) = press.split_at(press.len() - 1);
            for &c in arm {
                state.vm_key(key(BareKey::Char(c)));
            }
            if !arm.is_empty() {
                assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
            }
            state.vm_key(key(BareKey::Char(answer[0])));
            if arm.is_empty() {
                assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
            }
            assert!(state.vms.busy);
            assert!(state.vms.pending.is_none(), "{operation:?}");
            assert_eq!(
                state.vms.queued.as_ref().map(|(queued, _)| *queued),
                Some(operation)
            );
            // Still busy at the next tick: still queued.
            state.poll_vms();
            assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
            assert!(state.vms.queued.is_some());

            assert!(state.accept_vm_result(late(&sync, "Worktree sync completed.")));
            let snapshot = state.vms.snapshot.clone();
            assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
            assert_eq!(
                state.vms.pending.as_ref().map(|pending| pending.operation),
                Some(operation)
            );
            assert!(state.vms.queued.is_none());
        }
    }

    /// The same queue with the tab out of sight when the late reply comes - the panel put
    /// away, or another tab chosen: the request still goes once hocus has finished, behind a
    /// listing that checks its lease, rather than at the next visit to the tab, which can be
    /// hours after the user confirmed it.
    #[test]
    fn a_request_queued_during_busy_goes_out_after_the_late_reply_while_hidden() {
        for leave in ["hide", "switch"] {
            for (press, operation) in [
                (&['c'][..], Operation::Changes),
                (&['x', 'y'][..], Operation::Close),
            ] {
                let mut state = state();
                let sync = expired_sync(&mut state);
                let (arm, answer) = press.split_at(press.len() - 1);
                for &c in arm {
                    state.vm_key(key(BareKey::Char(c)));
                }
                if !arm.is_empty() {
                    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
                }
                state.vm_key(key(BareKey::Char(answer[0])));
                if arm.is_empty() {
                    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
                }
                assert!(state.vms.busy, "{leave} {operation:?}");
                assert_eq!(
                    state.vms.queued.as_ref().map(|(queued, _)| *queued),
                    Some(operation)
                );
                match leave {
                    "hide" => {
                        state.vms.visible = false;
                        state.end_visit();
                    },
                    _ => state.switch_surface(Surface::Agents),
                }
                // Still busy: the tick sends nothing out of sight, and the request waits.
                state.poll_vms();
                assert!(state.vms.pending.is_none(), "{leave} {operation:?}");
                assert!(state.vms.queued.is_some());

                assert!(state.accept_vm_result(late(&sync, "Worktree sync completed.")));
                assert_eq!(
                    state.vms.pending.as_ref().map(|pending| pending.operation),
                    Some(Operation::List),
                    "{leave} {operation:?}: the listing the queued request waits on"
                );
                let snapshot = state.vms.snapshot.clone();
                assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
                assert_eq!(
                    state.vms.pending.as_ref().map(|pending| pending.operation),
                    Some(operation),
                    "{leave}"
                );
                assert!(state.vms.queued.is_none());
                // Its answer is taken out of sight too, and nothing more goes out after it.
                assert!(state.accept_vm_result(reply(&state, None, "done", "")));
                assert!(state.vms.pending.is_none(), "{leave} {operation:?}");
                state.poll_vms();
                assert!(state.vms.pending.is_none(), "{leave} {operation:?}");
            }
        }
    }

    /// At the width a divider has in a pane at its default half, what each label cannot do
    /// without is what shows - the answer keys, the seconds, the key that fixes things -
    /// however long the VM's name.
    #[test]
    fn vm_divider_keeps_keys_and_seconds_at_the_default_half() {
        let drawn = |state: &State| {
            let label = state.vm_divider_label(24, false);
            divider_text(&label, 0, 5, false, 24).content().to_owned()
        };
        for name in ["9cc0eea7664ed86b", "hocus-agent-18a7b3c4d5e6f708-1"] {
            let mut state = state();
            state.vms.snapshot.vms[0].entry.name = name.into();
            state.render(20, 24);
            state.vm_key(key(BareKey::Char('x')));
            assert!(drawn(&state).contains("y close"), "{}", drawn(&state));
            state.vm_key(key(BareKey::Char('n')));
            state.vm_key(key(BareKey::Char('s')));
            assert!(drawn(&state).contains("y sync"), "{}", drawn(&state));
            state.vm_key(key(BareKey::Char('n')));

            let identity = state.vms.snapshot.vms[0].identity();
            assert!(state.vm_request(Operation::Sync, Some(identity.clone())));
            state.vms.pending_started_ms = now_ms() - 12_000;
            assert!(drawn(&state).contains("12s"), "{}", drawn(&state));
            state.vms.pending = None;

            state.vms.queued = Some((Operation::Close, identity));
            assert!(drawn(&state).contains("Queued"), "{}", drawn(&state));
            state.vms.queued = None;

            state.vms.owner = None;
            assert!(drawn(&state).contains("Ctrl e c"), "{}", drawn(&state));
        }
    }

    /// A lease's row leads with the VM's name, so rows of one session's VMs differ where they
    /// start, and a generated name too long for the row keeps the end that tells it apart.
    #[test]
    fn vm_rows_name_the_vm_and_fit_beside_the_marker() {
        let mut state = state();
        state.vms.snapshot = serde_json::from_str(
            r#"{"vms":[{"session":"one","name":"api","state":"running"},{"session":"one","name":"web","state":"running"}]}"#,
        )
        .unwrap();
        assert_eq!(
            state.vm_rows(24),
            ["api / one  running  —", "web / one  running  —"]
        );

        state.vms.snapshot = serde_json::from_str(
            r#"{"vms":[{"session":"brave-lemur","name":"hocus-agent-186a4f3b2c1d0e9f-1","state":"running"},{"session":"brave-lemur","name":"hocus-agent-186a4f3b2c1d0e9f-2","state":"running"}]}"#,
        )
        .unwrap();
        for cols in 12..80 {
            let rows = state.vm_rows(cols);
            for row in &rows {
                assert!(2 + row.width() <= cols, "{cols}: {row}");
            }
            if cols >= 13 {
                assert_ne!(rows[0], rows[1], "{cols}");
            }
        }
        assert!(
            details(&state).starts_with("VM: hocus-agent-186a4f3b2c1d0e9f-1 · session brave-lemur")
        );
    }

    /// Too short to split, the status row gives a failure's reason rather than the CLI's
    /// `VM command failed (exit N):` in front of it; the details keep every word.
    #[test]
    fn a_short_pane_status_row_names_a_failures_reason() {
        for error in [
            "VM command failed (exit 1): Error: guest agent unreachable",
            "VM command failed (exit 1): syncing 3 files\nError: guest agent unreachable",
        ] {
            let mut state = state();
            state.render(7, 60);
            assert_eq!(state.vms.last_details_rows, 0);
            let one = state.vms.snapshot.vms[0].identity();
            assert!(state.vm_request(Operation::Sync, Some(one)));
            assert!(state.accept_vm_result(reply(&state, None, "", error)));
            let label = state.vm_divider_label(60, true);
            assert!(
                label.starts_with("Sync failed: guest agent unreachable"),
                "{label}"
            );
            assert!(details(&state).contains(error));
        }
    }

    /// A session whose only `zellij:hocus` overlays are `ids`: one of them owns the tab, two
    /// leave it with no one to ask.
    fn overlays(ids: &[u32]) -> SessionInfo {
        SessionInfo {
            is_current_session: true,
            plugins: ids
                .iter()
                .map(|&id| {
                    (
                        PluginId::new(id),
                        PluginInfo {
                            location: "zellij:hocus".into(),
                            configuration: launch::overlay_configuration(),
                        },
                    )
                })
                .collect(),
            panes: PaneManifest {
                panes: std::collections::HashMap::from([(
                    Default::default(),
                    ids.iter()
                        .map(|&id| PaneInfo {
                            id: PaneId::Plugin(PluginId::new(id)),
                            is_floating: true,
                            ..Default::default()
                        })
                        .collect(),
                )]),
            },
            ..Default::default()
        }
    }

    fn result_text(state: &State, identity: &Identity) -> Option<String> {
        state
            .vms
            .results
            .iter()
            .find(|outcome| outcome.identity == *identity)
            .map(|outcome| outcome.text.clone())
    }

    /// A reply is taken only from the hocus a request went to, so a change of owner says, for
    /// each request left out, that its answer will not come here - not a failure, since it may
    /// have run - rather than leaving a promise that the result will replace the line.
    #[test]
    fn an_owner_change_says_unanswered_requests_will_not_be_answered() {
        let mut queued = state();
        let mut state = state();
        state.vms.owner = None;
        assert!(state.vm_sessions(&[overlays(&[1])]));
        assert_eq!(state.vms.owner, Some(PluginId::new(1)));
        let zero = state.vms.snapshot.vms[0].identity();
        let one = state.vms.snapshot.vms[1].identity();
        assert!(state.vm_request(Operation::Sync, Some(zero.clone())));
        let sync = state.vms.pending.clone().unwrap();
        assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
        assert!(state.vm_request(Operation::Changes, Some(one.clone())));

        assert!(state.vm_sessions(&[overlays(&[1, 2])]));
        assert!(state.vms.owner.is_none());
        assert!(!state
            .vms
            .results
            .iter()
            .any(|outcome| outcome.text.contains("will replace this line")));
        for identity in [&zero, &one] {
            assert_eq!(
                result_text(&state, identity).as_deref(),
                Some(OWNER_CHANGED)
            );
        }
        assert!(!state.vms.has_error());

        assert!(state.vm_sessions(&[overlays(&[1])]));
        let message = PipeMessage::new(
            PipeSource::Plugin(PluginId::new(1)),
            vms::RESULT,
            &Some(serde_json::to_string(&late(&sync, "Worktree sync completed.")).unwrap()),
            &None,
            true,
        );
        assert_eq!(state.vm_pipe(&message), Some(false));
        assert_eq!(result_text(&state, &zero).as_deref(), Some(OWNER_CHANGED));

        // A request still queued behind the listing was never sent at all.
        queued.poll_vms();
        assert!(queued.vm_key(key(BareKey::Char('c'))));
        assert!(queued.vms.queued.is_some());
        assert!(queued.vm_sessions(&[overlays(&[1, 2])]));
        assert!(queued.vms.queued.is_none());
        assert!(result_text(&queued, &zero).is_some_and(|text| text.starts_with("Not sent")));
        assert!(!queued.vms.has_error());
    }

    /// A listing given up on is most likely the request hocus is still busy with, so its
    /// refusal keeps the expiry's words - still stale, still marked - rather than blaming
    /// another request nobody sent. A refusal with nothing stuck is still said as it is.
    #[test]
    fn a_listing_stuck_past_the_deadline_is_not_called_another_request() {
        let mut state = state();
        state.poll_vms();
        assert!(state.expire_vm_request(state.vms.pending_deadline_ms));
        for _ in 0..2 {
            state.poll_vms();
            assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
            state.render(20, 40);
            assert_eq!(
                state.vm_divider_label(40, false),
                "STALE: VM listing did not answer in 60 s; retrying."
            );
            assert!(!state
                .vm_detail_rows(200)
                .0
                .join("\n")
                .contains("Another VM request"));
            assert!(state.vms.has_error());
        }
        state.poll_vms();
        let snapshot = state.vms.snapshot.clone();
        assert!(state.accept_vm_result(reply(&state, Some(snapshot), "", "")));
        assert!(state.vms.error.is_empty());
        state.poll_vms();
        assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
        assert_eq!(state.vms.error, vms::BUSY);
    }

    /// While the panel's tab bar has the keys the divider names the bar's, on the row it has
    /// anyway, and the list and details keep every row they had.
    #[test]
    fn the_bar_hint_takes_the_vm_divider_not_a_row() {
        let mut state = state();
        state.render(12, 60);
        let halves = |state: &State| (state.vms.last_list_rows, state.vms.last_details_rows);
        let unfocused = halves(&state);
        assert!(!state.vm_divider_label(60, false).contains("←/→ switch"));
        state.tabs_focused = true;
        state.render(12, 60);
        assert_eq!(halves(&state), unfocused);
        let label = state.vm_divider_label(60, false);
        assert!(label.contains("←/→ switch"), "{label}");
        state.tabs_focused = false;
        state.render(12, 60);
        assert_eq!(halves(&state), unfocused);
        assert!(!state.vm_divider_label(60, false).contains("←/→ switch"));

        // With no hocus to ask, the bar's keys still come first while it has them.
        state.vms.owner = None;
        assert_eq!(state.vm_divider_label(60, false), NO_OWNER_DIVIDER);
        state.tabs_focused = true;
        let label = state.vm_divider_label(60, false);
        assert!(label.contains("←/→ switch"), "{label}");
    }

    /// A status on the divider - a request queued, running or still running after its
    /// timeout, or a listing that failed - still leads it while the bar has the keys, and the
    /// bar's keys follow it: the divider is the one row on the tab that names keys, so the
    /// status taking all of it left the bar's named nowhere.
    #[test]
    fn a_vm_status_keeps_the_bar_hint_while_the_bar_has_the_keys() {
        for case in ["stale", "empty", "queued", "pending", "still running"] {
            let mut state = state();
            state.render(12, 60);
            let one = state.vms.snapshot.vms[0].identity();
            match case {
                "stale" => state.vms.error = "provider denied".into(),
                "empty" => {
                    state.vms.snapshot = Snapshot::default();
                    state.vms.error = "provider denied".into();
                },
                "queued" => state.vms.queued = Some((Operation::Sync, one)),
                "pending" => assert!(state.vm_request(Operation::Sync, Some(one))),
                _ => {
                    expired_sync(&mut state);
                    assert!(state.accept_vm_result(reply(&state, None, "", vms::BUSY)));
                    assert!(state.vms.busy);
                },
            }
            let own = state.vm_divider_label(60, false);
            let expected = match case {
                "stale" => "STALE: provider denied",
                "empty" => "STALE · Ctrl-r retry",
                "queued" => "Queued: ",
                "pending" => "Syncing ",
                _ => ", still running",
            };
            assert!(own.contains(expected), "{case}: {own}");
            assert!(!own.contains("←/→ switch"), "{case}: {own}");

            // A running request's seconds can tick between two labels, so those are compared
            // by the status's text rather than whole.
            let timeless = matches!(case, "stale" | "empty" | "queued");
            state.tabs_focused = true;
            let label = state.vm_divider_label(60, false);
            let bar = label.find("←/→ switch");
            assert!(
                bar.is_some_and(|at| label[..at].contains(expected)),
                "{case}: {label}"
            );
            assert!(!timeless || label.starts_with(&own), "{case}: {label}");
            state.render(12, 60);
            assert_eq!(
                state.footer_hint(),
                None,
                "{case}: said once, on the divider"
            );
            state.tabs_focused = false;
            let back = state.vm_divider_label(60, false);
            assert!(!back.contains("←/→ switch"), "{case}: {back}");
            assert!(!timeless || back == own, "{case}: {back}");
        }
    }

    /// A quiet list - nothing listed and nothing to say under it - has no divider, so while the
    /// tab bar has the keys it gets the one row that names them, and gives it back after. With
    /// no hocus to ask, the placeholder says so and the row still names the bar's keys, rather
    /// than the same complaint twice.
    #[test]
    fn a_quiet_vm_list_gives_the_bar_hint_a_row_while_the_bar_has_the_keys() {
        for owner in [Some(PluginId::new(1)), None] {
            let mut state = State {
                surface: Surface::Vms,
                ..Default::default()
            };
            state.vms.owner = owner;
            state.render(12, 60);
            assert_eq!(state.vms.last_list_rows, 11, "{owner:?}");
            assert_eq!(state.vms.last_details_rows, 0);
            state.tabs_focused = true;
            state.render(12, 60);
            assert_eq!(state.vms.last_list_rows, 10, "{owner:?}");
            let label = state.vm_divider_label(60, true);
            assert!(label.contains("←/→ switch"), "{owner:?}: {label}");
            assert_eq!(
                state.footer_hint(),
                None,
                "said once, not on a footer as well"
            );
            state.tabs_focused = false;
            state.render(12, 60);
            assert_eq!(state.vms.last_list_rows, 11, "{owner:?}");
        }
    }
}
