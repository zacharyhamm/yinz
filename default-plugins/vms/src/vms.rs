use super::*;
use agent_proto::vms::{self, Identity, Operation, Request, ResultMessage, Snapshot};
use std::ops::Range;

/// How long the tab waits on a reply before it stops holding its one request slot for it.
///
/// A safety net for replies agent may never send - a agent that reloaded, a companion
/// re-verified - and not a bound on the work: the provider times out on inactivity, so a
/// healthy sync can run past this. Which is why a request given up on is still answered
/// (`timed_out_operations`) and why the words for it say "no reply yet", not "failed".
const REQUEST_TIMEOUT_MS: i64 = 60_000;

const NO_OWNER: &str = "VM command runner is unavailable";
const NO_OWNER_DIVIDER: &str = NO_OWNER;

#[derive(Default)]
pub(super) struct View {
    pub(crate) owner: Option<PluginId>,
    pub visible: bool,
    pub(crate) snapshot: Snapshot,
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
    /// The user's own request, waiting for a background listing to answer first - or for agent
    /// to finish one it is still [`busy`](Self::busy) with.
    ///
    /// agent runs one request at a time, so sending it alongside the listing would only be
    /// refused - and dropping it, which is what `c`, `s` and `x` used to do whenever the
    /// 2-second poll happened to be out, threw away a key the divider was advertising. It
    /// goes out as soon as a listing answers without being refused, and only if its lease is
    /// still listed.
    queued: Option<(Operation, Identity)>,
    /// Requests given up on that agent may still answer, each with when it was sent.
    timed_out_operations: Vec<(Request, i64)>,
    /// agent answered the last listing with [`vms::BUSY`] while one of those was still out:
    /// proof the request is alive, only slow, so the tab says it is still running rather than
    /// calling the list stale - and holds `c`, `s` and `x`, which agent would refuse the same
    /// way. Cleared by the late reply, by the next listing that is not refused, and by an owner
    /// change, so it cannot hold them for good.
    busy: bool,
    /// The last listing went unanswered past the deadline, and agent has answered none since
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
    /// on that agent has since answered [`busy`](Self::busy) for, which is still running there
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
    /// The 2-second tick's part in this tab: expire a request agent has not answered, list
    /// again if the tab is showing and nothing is out, and say whether that is worth a redraw.
    pub(super) fn poll_vms(&mut self) -> bool {
        let expired = self.expire_vm_request(now_ms());
        let showing = self.vms.visible;
        if showing {
            // Seen: the `!` is for a failure that arrived while the user was elsewhere.
            self.vms.failed = false;
        }
        // A request the user confirmed goes out from the listing's reply, which checks its
        // lease is still listed - so once agent is no longer busy it gets that listing even
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
            // queued behind the listing: agent is most likely still running the listing, and
            // would refuse it as busy - which used to be recorded as the request failing, and
            // lose it. It waits for a listing agent answers, as `send_or_queue` makes a new one
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
                    "No reply after {seconds} s; agent may still be running it. The result will \
                     replace this line when it arrives."
                ),
            );
        }
        let started = self.vms.pending_started_ms;
        self.vms.timed_out_operations.push((request, started));
        self.poll_vms();
        true
    }
    /// Send `operation` to the owner, or say it could not go: there is no single agent to
    /// ask, or a request is already out.
    fn vm_request(&mut self, operation: Operation, target: Option<Identity>) -> bool {
        let Some(_owner) = self.vms.owner else {
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
        self.vms.pending = Some(request.clone());
        self.vms.pending_started_ms = now;
        self.vms.pending_deadline_ms = now.saturating_add(REQUEST_TIMEOUT_MS);
        if operation != Operation::List {
            // A new attempt: the old failure's `!` has said what it had to.
            self.vms.failed = false;
        }
        #[cfg(not(test))]
        self.execute_vm_request(request);
        true
    }
    /// The user's own request: out now, or queued behind a background listing, which answers
    /// within the tick, or behind a request agent is still [`busy`](View::busy) with - a `y`
    /// armed just before agent said so, which sent now would only be refused. So too behind a
    /// listing given up on ([`list_expired`](View::list_expired)), which agent is most likely
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
        if failed && !(self.vms.visible) {
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
    #[cfg(test)]
    pub(super) fn vm_pipe(&mut self, message: &PipeMessage) -> Option<bool> {
        if matches!(
            message.name.as_str(),
            "vm_panel_visibility" | "vm_panel_result"
        ) && !self
            .vms
            .owner
            .is_some_and(|owner| message.source == PipeSource::Plugin(owner))
        {
            return Some(false);
        }
        match message.name.as_str() {
            "vm_panel_visibility" => {
                let visible = message.payload.as_deref() == Some("true") && self.pane_shown;
                let opened = visible && !self.vms.visible;
                self.vms.visible = visible;
                if opened {
                    self.poll_vms();
                }
                // agent putting the panel away is the panel going away, whichever half asked.
                if !visible {
                    self.end_visit();
                }
                Some(opened)
            },
            "vm_panel_result" => {
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
    pub(crate) fn accept_vm_result(&mut self, result: ResultMessage) -> bool {
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
            // agent answers one request at a time, so "busy" while one we stopped waiting on
            // is still out says that request is alive - not that the listing failed. With none
            // out but a listing given up on, the one agent is busy with is most likely that
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
            // a lease still listed: agent would refuse a changed one anyway, and saying so
            // here names the lease rather than leaving it to a later refusal. Not after a
            // listing agent refused as busy - with a request given up on, or with a listing
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
                    // Not run: agent was still busy with another request. That is proof the
                    // other one is alive, not that this one failed - recorded as a failure, the
                    // user's confirmed close or sync was lost. It goes back in the queue, to go
                    // out after a listing agent answers.
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
                _ => self.focus_panel_bar(wire::STOP_TABS),
            }
            return true;
        }
        // Ctrl-o never gets this far: `on_key` takes it ahead of every surface.
        if key.has_modifiers(&[KeyModifier::Ctrl]) {
            if key.bare_key == BareKey::Char('r') && self.vms.pending.is_none() {
                // Asked for, so not held to `poll_vms`'s conditions: a key reaching this pane
                // means the tab is on screen, whatever agent last said about the panel. One
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
    /// with no one agent to ask nothing is sent.
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
        if split {
            sheet.push(row("PgUp PgDn", "scroll the details"));
        }
        if owner {
            sheet.push(row("Ctrl-r", "refresh the list now"));
        }
        // As `vm_key` walks the ring: the details are a stop only where they are drawn.
        sheet.extend(match (self.vms.details, split) {
            (true, _) => [
                row("Tab", "on to the panel tab bar"),
                row("Shift+Tab", "back to the list"),
            ],
            (false, true) => [
                row("Tab", "into the details"),
                row("Shift+Tab", "to the panel's tabs"),
            ],
            (false, false) => [
                row("Tab", "on to the panel tab bar"),
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
        // on_mouse has checked the bounds; mouse rows are already content-relative.
        let over_list = match self.mouse_position.map(|(line, _)| line as usize) {
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
                    "{} queued until agent finishes the request still running…",
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
    /// in: a listing that failed, one not yet asked for or not yet answered, no agent to ask,
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
                false => "Waiting for agent…",
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
        let bar = self.bar_has_keys();
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
            return agents::fit_keys(&segments, room);
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
        if bar {
            agents::fit_keys(&segments[first..], room)
        } else {
            agents::fit_hints_marked(&segments[first..], room, status + 1)
        }
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
        let bar = self.bar_has_keys();
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
/// given up on that agent says it is still running - then to which lease. The lease last,
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
/// printed them (the CLI's `Error: …`), rather than the `VM command failed (exit 1):` agent
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
#[path = "unit/vms.rs"]
mod tests;
