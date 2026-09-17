use super::*;
use hocus_proto::vms::{self, Identity, Operation, Request, ResultMessage, Snapshot};

const REQUEST_TIMEOUT_MS: i64 = 60_000;

#[derive(Default)]
pub(super) struct View {
    owner: Option<PluginId>,
    pub visible: bool,
    snapshot: Snapshot,
    selected: usize,
    scroll: usize,
    pub details: bool,
    pane: Option<usize>,
    reveal_pane: bool,
    pending: Option<Request>,
    timed_out_operations: Vec<Request>,
    pending_deadline_ms: i64,
    sequence: u64,
    confirming: Option<(Operation, Identity)>,
    results: Vec<(Identity, String)>,
    error: String,
    last_list_rows: usize,
    pub last_details_rows: usize,
    window: usize,
    max_scroll: usize,
}
impl View {
    pub(super) fn clear_dimensions(&mut self) {
        self.last_list_rows = 0;
        self.last_details_rows = 0;
        self.window = 0;
        self.max_scroll = 0;
    }

    pub(super) fn has_error(&self) -> bool {
        !self.error.is_empty()
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
        self.vms.owner = owner;
        self.vms.visible = false;
        self.vms.pending = None;
        self.vms.timed_out_operations.clear();
        self.vms.confirming = None;
        if owner.is_some() {
            // Request the owner's visibility after verifying it, even if its first update
            // arrived before this manifest.
            self.send_selection();
        }
        true
    }

    pub(super) fn poll_vms(&mut self) -> bool {
        let expired = self.expire_vm_request(now_ms());
        if self.surface == Surface::Vms
            && self.vms.visible
            && self.vms.pending.is_none()
            && self.vms.confirming.is_none()
        {
            self.vm_request(Operation::List, None);
        }
        expired
    }
    fn expire_vm_request(&mut self, now: i64) -> bool {
        if now < self.vms.pending_deadline_ms {
            return false;
        }
        let Some(request) = self.vms.pending.clone() else {
            return false;
        };
        self.accept_vm_result(ResultMessage {
            request_id: request.request_id,
            operation: request.operation,
            target: request.target.clone(),
            snapshot: None,
            output: String::new(),
            error: "VM request timed out; outcome unknown. The command may still be running; check its state before retrying.".into(),
        });
        if request.operation != Operation::List {
            self.vms.timed_out_operations.push(request);
        }
        true
    }
    fn vm_request(&mut self, operation: Operation, target: Option<Identity>) {
        let Some(owner) = self.vms.owner else {
            return;
        };
        if self.vms.pending.is_some() {
            return;
        }
        let now = now_ms();
        self.vms.sequence = self.vms.sequence.max(now.max(0) as u64) + 1;
        let request = Request {
            request_id: self.vms.sequence,
            operation,
            target,
        };
        if let Ok(payload) = serde_json::to_string(&request) {
            pipe_message_to_plugin(
                MessageToPlugin::new(vms::REQUEST)
                    .with_destination_plugin_id(owner)
                    .with_payload(payload),
            );
            self.vms.pending = Some(request);
            self.vms.pending_deadline_ms = now.saturating_add(REQUEST_TIMEOUT_MS);
        }
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
                Some(opened)
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
        } else if let Some(index) = self.vms.timed_out_operations.iter().position(matches) {
            self.vms.timed_out_operations.remove(index);
        } else {
            return false;
        }
        if result.operation == Operation::List {
            self.vms.error = result.error;
            if let Some(snapshot) = result.snapshot {
                let selected = self
                    .vms
                    .snapshot
                    .vms
                    .get(self.vms.selected)
                    .map(|lease| lease.identity());
                let previous_selection = selected.clone();
                let pane_id = self
                    .vms
                    .snapshot
                    .vms
                    .get(self.vms.selected)
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
        } else {
            if let Some(identity) = result.target {
                self.vms.results.retain(|(key, _)| *key != identity);
                let text = if result.error.is_empty() {
                    result.output
                } else {
                    result.error
                };
                self.vms.results.push((identity, text));
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
                    .snapshot
                    .vms
                    .get(self.vms.selected)
                    .is_some_and(|lease| lease.identity() == identity)
            {
                self.vm_request(operation, Some(identity));
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
        if key.has_modifiers(&[KeyModifier::Ctrl]) {
            if key.bare_key == BareKey::Char('r') {
                self.poll_vms();
            }
            if key.bare_key == BareKey::Char('o') {
                // Cycle the whole panel one corner clockwise; hocus owns the geometry.
                pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
            }
            return true;
        }
        if !key.has_no_modifiers() {
            return false;
        }
        match key.bare_key {
            BareKey::Esc if self.vms.pane.is_some() => {
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
                if let Some(lease) = self.vms.snapshot.vms.get(self.vms.selected) {
                    if let Some(pane) = self
                        .vms
                        .pane
                        .filter(|_| self.vms.details)
                        .and_then(|index| lease.panes.get(index))
                    {
                        let session = SessionName::from(lease.entry.session.as_str());
                        let id = TerminalId::new(pane.id);
                        let current =
                            self.vms.snapshot.current_session.as_deref() == Some(session.as_str());
                        self.hide_the_panel();
                        if current {
                            focus_terminal_pane(id, false, false);
                        } else {
                            switch_session_with_focus(&session, None, Some(PaneId::Terminal(id)));
                        }
                    } else {
                        self.vms.details = true;
                        self.vms.pane = (!lease.panes.is_empty()).then_some(0);
                        self.vms.reveal_pane = true;
                        self.vms.scroll = 0;
                    }
                }
            },
            BareKey::Char(c @ ('c' | 's' | 'x')) => {
                if self.vms.pending.is_some() {
                    return false;
                }
                if let Some(lease) = self.vms.snapshot.vms.get(self.vms.selected) {
                    let identity = lease.identity();
                    let operation = match c {
                        'c' => Operation::Changes,
                        's' => Operation::Sync,
                        _ => Operation::Close,
                    };
                    if operation == Operation::Changes {
                        self.vm_request(operation, Some(identity));
                    } else {
                        self.vms.confirming = Some((operation, identity));
                        self.vms.details = true;
                        self.vms.scroll = 0;
                    }
                }
            },
            _ => return false,
        }
        true
    }
    fn vm_step(&mut self, delta: isize) {
        if let Some(index) = self.vms.pane.filter(|_| self.vms.details) {
            let count = self
                .vms
                .snapshot
                .vms
                .get(self.vms.selected)
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
        }
    }
    pub(super) fn vm_mouse(&mut self, event: Mouse) -> bool {
        match event {
            Mouse::LeftClick(line, _) if line >= 0 => {
                let line = line as usize;
                self.vms.details = line >= self.vms.last_list_rows;
                if !self.vms.details {
                    // The retained-disk group header is display-only.
                    let displayed = self.vms.window + line;
                    let count = self.vms.snapshot.vms.len();
                    if displayed != count || self.vms.snapshot.recoveries.is_empty() {
                        self.vms.selected = displayed
                            .saturating_sub(usize::from(displayed > count))
                            .min((count + self.vms.snapshot.recoveries.len()).saturating_sub(1));
                        self.vms.scroll = 0;
                        self.vms.pane = None;
                    }
                }
            },
            Mouse::ScrollUp(lines) => {
                self.vms.scroll = self.vms.scroll.saturating_sub(lines.max(1))
            },
            Mouse::ScrollDown(lines) => {
                self.vms.scroll = (self.vms.scroll + lines.max(1)).min(self.vms.max_scroll)
            },
            _ => return false,
        }
        true
    }
    fn vm_details(&self) -> Vec<String> {
        let mut lines = vec![];
        if let Some(lease) = self.vms.snapshot.vms.get(self.vms.selected) {
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
                    lines.push(format!(
                        "{} {} in session {}? y/n",
                        if *op == Operation::Close {
                            "Close"
                        } else {
                            "Sync worktree for"
                        },
                        target.name,
                        target.session
                    ));
                    lines.push(format!(
                        "Affected panes: {}. Remaining leases: {}",
                        if lease.panes_error.is_some() {
                            "unknown (session listing failed)".into()
                        } else if lease.panes.is_empty() {
                            "none".into()
                        } else {
                            lease
                                .panes
                                .iter()
                                .map(|pane| pane.id.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        },
                        if sharing.is_empty() {
                            "none".into()
                        } else {
                            sharing.join(", ")
                        }
                    ));
                }
            }
            if let Some(error) = &lease.panes_error {
                lines.push(error.clone());
            }
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
            if let Some((_, result)) = self.vms.results.iter().find(|(key, _)| *key == identity) {
                lines.push("Latest operation:".into());
                lines.extend(result.lines().map(str::to_owned));
            }
        } else if let Some(disk) = self
            .vms
            .selected
            .checked_sub(self.vms.snapshot.vms.len())
            .and_then(|index| self.vms.snapshot.recoveries.get(index))
        {
            lines.extend([
                format!("Reason: {}", disk.reason),
                format!("Retained at: {} (Unix seconds)", disk.retained_at),
                format!("Recovery path: {}", disk.path.display()),
                "Informational; no delete or recovery action.".into(),
            ]);
        }
        if let Some((identity, result)) = self.vms.results.last() {
            if !self
                .vms
                .snapshot
                .vms
                .get(self.vms.selected)
                .is_some_and(|lease| lease.identity() == *identity)
            {
                lines.push(format!(
                    "Latest operation: {} / {}",
                    identity.session, identity.name
                ));
                lines.extend(result.lines().map(str::to_owned));
            }
        }
        lines
    }
    pub(super) fn render_vms(&mut self, rows: usize, cols: usize) {
        let (mut list_rows, mut detail_rows) = split(rows);
        if self.vms.confirming.is_some() && detail_rows == 0 && rows > 1 {
            list_rows = 0;
            detail_rows = rows - 1;
        }
        self.vms.last_list_rows = list_rows;
        self.vms.last_details_rows = detail_rows;
        if detail_rows == 0 {
            self.vms.details = false;
        }
        let mut lines: Vec<String> =
            self.vms
                .snapshot
                .vms
                .iter()
                .map(|lease| {
                    let shared_mark = if self.vms.snapshot.vms.iter().any(|other| {
                        other.entry.session != lease.entry.session && shared(lease, other)
                    }) {
                        " shared"
                    } else {
                        ""
                    };
                    let identity = format!("{} / {}", lease.entry.session, lease.entry.name);
                    let cpu = format!("  {}", percent(lease.cpu_percent));
                    let memory = format!(
                        "  {}",
                        bytes(lease.entry.metrics.as_ref().and_then(|m| m.memory_bytes))
                    );
                    let room = cols.saturating_sub(
                        identity.width() + lease.entry.state.width() + shared_mark.len() + 4,
                    );
                    let extra = if cpu.width() + memory.width() <= room {
                        format!("{cpu}{memory}")
                    } else if cpu.width() <= room {
                        cpu
                    } else {
                        String::new()
                    };
                    format!(
                        "{}  {}{shared_mark}{extra}",
                        agents::fit_label(
                            identity,
                            cols.saturating_sub(
                                lease.entry.state.width() + shared_mark.len() + extra.width() + 4
                            )
                        ),
                        lease.entry.state
                    )
                })
                .collect();
        if !self.vms.snapshot.recoveries.is_empty() {
            lines.push("Retained disks".into());
            lines.extend(self.vms.snapshot.recoveries.iter().map(|disk| {
                format!(
                    "{} / {}: {}",
                    disk.session.as_deref().unwrap_or("—"),
                    disk.name.as_deref().unwrap_or("—"),
                    disk.reason
                )
            }));
        }
        if lines.is_empty() {
            lines.push(
                if self.vms.pending.is_some() {
                    "Loading VMs…"
                } else {
                    "No VMs or retained disks"
                }
                .into(),
            );
        }
        let selected = self.vms.selected
            + usize::from(
                self.vms.selected >= self.vms.snapshot.vms.len()
                    && !self.vms.snapshot.recoveries.is_empty(),
            );
        self.vms.window = selected
            .saturating_sub(list_rows / 2)
            .min(lines.len().saturating_sub(list_rows));
        for (index, line) in lines
            .iter()
            .enumerate()
            .skip(self.vms.window)
            .take(list_rows)
        {
            let text = Text::new(format!(
                "{} {line}",
                if index == selected { ">" } else { " " }
            ));
            paint(
                if index == selected {
                    text.selected()
                } else {
                    text
                },
                0,
                index - self.vms.window,
                Some(cols),
                None,
            );
        }
        if detail_rows == 0 {
            return;
        }
        let label = if !self.vms.error.is_empty() {
            format!("STALE: {}", self.vms.error)
        } else if self.vms.pending.is_some() {
            "VM request running…".into()
        } else {
            "c changes · s sync · x close · Ctrl-r refresh".into()
        };
        render_divider(
            &label,
            self.vms.scroll,
            0,
            self.vms.details,
            list_rows,
            cols,
        );
        let mut details = self.vm_details();
        if !self.vms.error.is_empty() {
            details.insert(0, format!("STALE: {}", self.vms.error));
        }
        // Host output is literal text. Wrapping must preserve punctuation in recovery paths.
        let chars: Vec<_> = details
            .join("\n")
            .chars()
            .map(|ch| markdown::StyledChar {
                ch,
                style: markdown::MdStyle::default(),
            })
            .collect();
        let wrapped: Vec<String> = markdown::hard_wrap_styled(&chars, cols.max(1))
            .into_iter()
            .map(|line| line.into_iter().map(|ch| ch.ch).collect())
            .collect();
        self.vms.max_scroll = wrapped.len().saturating_sub(detail_rows);
        if self.vms.reveal_pane {
            self.vms.reveal_pane = false;
            if let Some(line) = wrapped
                .iter()
                .position(|line| line.starts_with("> ") || line == ">")
            {
                self.vms.scroll = line;
            }
        }
        self.vms.scroll = self.vms.scroll.min(self.vms.max_scroll);
        for (index, line) in wrapped
            .iter()
            .skip(self.vms.scroll)
            .take(detail_rows)
            .enumerate()
        {
            paint(Text::new(line), 0, list_rows + 1 + index, Some(cols), None);
        }
    }
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
        state
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
                assert_eq!(state.vms.results[0].0, identity);
                assert!(state.vms.results[0].1.contains("outcome unknown"));
            } else {
                assert!(state.vms.error.contains("outcome unknown"));
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
                    state.vms.results[0].1,
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
        assert_eq!(state.vms.results[0].1, reply.error);
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
        assert!(state.vms.error.contains("outcome unknown"));
        assert!(state.vms.pending.as_ref().unwrap().request_id > id);
        state.vms.pending_deadline_ms = 0;
        state.vms.visible = false;
        state.surface = Surface::Agents;
        assert!(state.update(Event::Timer(FractionalSeconds::new(0.0))));
        assert!(state.vms.pending.is_none());
    }
    #[test]
    fn vm_confirmations_name_the_lease_and_do_not_follow_a_changed_selection() {
        let mut state = state();
        state.vm_key(KeyWithModifier::new(BareKey::Char('x')));
        let text = state.vm_details().join("\n");
        assert!(text.contains("Close api in session one?"));
        assert!(text.contains("Affected panes: 12. Remaining leases: two"));
        assert!(!text.contains("three"));
        state.render(10, 20);
        state.vm_key(KeyWithModifier::new(BareKey::PageDown));
        assert!(state.vms.confirming.is_some());
        assert!(state.vms.scroll > 0);
        state.vms.selected = 2;
        state.vm_key(KeyWithModifier::new(BareKey::Char('y')));
        assert!(state.vms.pending.is_none());
        state.vms.details = false;
        state.vm_key(KeyWithModifier::new(BareKey::Char('s')));
        state.vm_key(KeyWithModifier::new(BareKey::Char('y')));
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
        state.on_key(KeyWithModifier::new(BareKey::Right));
        assert_eq!(state.surface, Surface::Todos);
        state.on_key(KeyWithModifier::new(BareKey::Right));
        assert_eq!(state.surface, Surface::Vms);
        assert_eq!(state.selected, 7);
        assert_eq!(state.history_scroll, 9);
        state.on_key(KeyWithModifier::new(BareKey::Left));
        assert_eq!(state.surface, Surface::Todos);
        assert_eq!(state.vms.selected, 2);
        assert_eq!(state.vms.scroll, 4);
    }
    #[test]
    fn vm_ring_mouse_offset_and_small_sizes_keep_tabs_reachable() {
        let mut state = state();
        state.render(20, 40);
        let vm_x = state.tab_cells[2].x;
        state.on_mouse(Mouse::LeftClick(0, vm_x));
        assert!(state.tabs_focused);
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert!(!state.tabs_focused);
        assert!(!state.vms.details);
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert!(state.vms.details);
        state.on_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(state.ring_yields, [wire::STOP_TABS]);
        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert!(!state.vms.details);
        state.on_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert!(state.tabs_focused);
        state.on_mouse(Mouse::LeftClick(2, 0));
        assert_eq!(state.vms.selected, 1);
        for rows in 1..8 {
            state.render(rows, 5);
            assert_eq!(state.vms.last_details_rows, 0);
        }
    }
    #[test]
    fn vm_mouse_ignores_the_header_and_empty_panes_without_losing_its_place() {
        let mut state = state();
        let identity = state.vms.snapshot.vms[1].identity();
        state.vms.results.push((identity, "warning\n".repeat(100)));
        state.vms.selected = 1;
        state.vms.details = true;
        state.vms.scroll = 3;
        state.tabs_focused = true;
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
        // VM wheels keep scrolling details everywhere in the content area.
        for line in [1, 19] {
            state.on_mouse(Mouse::Hover(line, 39));
            assert!(state.on_mouse(Mouse::ScrollDown(1)));
            assert_eq!(state.vms.scroll, 4);
            assert!(state.on_mouse(Mouse::ScrollUp(1)));
            assert_eq!(position(&state), before);
        }
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

            state.render(20, 40);
            assert_eq!(dimensions(&state), visible);
            assert_eq!(position(&state), before);
        }
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
        state.vm_key(KeyWithModifier::new(BareKey::Enter));
        state.render(20, 20);
        state.vm_key(KeyWithModifier::new(BareKey::Down));
        state.render(20, 20);
        assert_eq!(state.vms.pane, Some(1));
        assert!(state.vms.scroll > 10);
    }
    #[test]
    fn vm_list_focus_controls_navigation_and_enter_with_a_retained_pane_cursor() {
        let mut state = state();
        state.render(20, 40);
        state.vm_key(KeyWithModifier::new(BareKey::Enter));
        state.vm_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        assert_eq!(state.vms.pane, Some(0));
        assert!(!state.vms.details);
        state.vm_key(KeyWithModifier::new(BareKey::Enter));
        assert!(state.vms.details, "Enter from the list must enter details");
        assert_eq!(state.panel_hides, 0);
        state.take_ring_stop(Some(wire::STOP_LIST));
        assert!(!state.vms.details);
        state.vm_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(state.vms.selected, 1);
        assert!(state.vms.pane.is_none());
        state.vm_key(KeyWithModifier::new(BareKey::Up));
        state.vm_key(KeyWithModifier::new(BareKey::Enter));
        state.vm_key(KeyWithModifier::new(BareKey::Tab).with_shift_modifier());
        state.vm_key(KeyWithModifier::new(BareKey::Down));
        assert_eq!(state.vms.selected, 1);
        assert!(state.vms.pane.is_none());
    }
    #[test]
    fn vm_retained_disks_are_informational_and_warnings_are_scrollable_in_full() {
        let mut state = state();
        let identity = state.vms.snapshot.vms[0].identity();
        let warning = (0..100)
            .map(|i| format!("warning {i}: host and guest differ"))
            .collect::<Vec<_>>()
            .join("\n");
        state.vms.results.push((identity, warning));
        state.render(20, 30);
        assert!(state.vms.max_scroll > 100);
        assert!(state.vm_details().join("\n").contains("warning 99:"));
        state.vms.selected = 3;
        assert!(state.vm_details().join("\n").contains("/keep/disk"));
        state.vm_key(KeyWithModifier::new(BareKey::Char('x')));
        assert!(state.vms.confirming.is_none());
        assert!(state.vms.pending.is_none());
    }
}
