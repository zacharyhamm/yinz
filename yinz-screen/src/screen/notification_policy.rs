//! Asynchronous session notification policy. Only this queue commits delivery and history.
use super::{Notification, NotificationClick, NotificationRecord, Screen};
use crate::{
    background_jobs::BackgroundJob,
    notifications::NotificationProtocol,
    output::Output,
    panes::PaneId,
    plugins::{PluginInstanceId, PluginInstruction},
    route::NotificationEnd,
    ServerInstruction,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use yinz_host_protocol::notifications::{
    namespace_notification_id, parse_osc99_metadata, split_osc99_payload, Osc99PayloadType,
    PendingNotification,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{
    Event, NotificationData, NotificationDecision, NotificationDelivery, NotificationInfo,
    NotificationResolution, NotificationSource,
};
use yinz_types::internal_ids::{NotificationId, PluginId, TerminalId};
use yinz_types::notifications::MAX_NOTIFICATION_REPLACEMENT_BYTES;
use yinz_types::units::UnixMillis;

const MAX_PENDING: usize = 256;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ASSEMBLY: usize = 64 * 1024;
const POLICY_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct NotificationPolicy {
    plugin: Option<PluginId>,
    available: bool,
    unavailable_reason: Option<String>,
    pending: VecDeque<Pending>,
    assemblies: HashMap<(TerminalId, String), Assembly>,
    next_assembly: u64,
}

#[derive(Default)]
struct Assembly {
    chunks: Vec<PendingNotification>,
    bytes: usize,
    sequence: u64,
    bypass: Option<String>,
}

struct Pending {
    original: NotificationData,
    desktop: Vec<PendingNotification>,
    osc99_identity: Option<(String, bool)>,
    on_click: Option<NotificationClick>,
    _completion: Option<NotificationEnd>,
    policy: Option<PluginId>,
    instance: Option<PluginInstanceId>,
    deadline: Instant,
    decision: Option<NotificationDecision>,
    fallback: Option<String>,
}

impl Pending {
    fn bytes(&self) -> usize {
        self.original.title.len()
            + self.original.body.len()
            + self.desktop.iter().map(raw_bytes).sum::<usize>()
    }
    fn fallback(&mut self, reason: &str) {
        if self.fallback.as_deref() != Some("application_closed") {
            self.decision = Some(NotificationDecision::Pass);
            self.fallback = Some(reason.into());
        }
    }
}

fn raw_bytes(notification: &PendingNotification) -> usize {
    match notification {
        PendingNotification::Osc99 {
            payload,
            terminator,
            ..
        } => payload.len() + terminator.len(),
        _ => {
            let (title, body) = notification.title_and_body();
            title.len() + body.len()
        },
    }
}

impl Screen {
    pub(super) fn set_notification_policy(
        &mut self,
        plugin: Option<PluginId>,
        reason: Option<String>,
    ) -> Result<()> {
        for pending in &mut self.notification_policy.pending {
            pending.fallback("policy_reconfigured");
        }
        self.notification_policy.plugin = plugin;
        self.notification_policy.available = plugin.is_some();
        self.notification_policy.unavailable_reason = reason;
        self.release_notification_assemblies("policy_reconfigured");
        self.commit_notifications()
    }

    pub(super) fn notification_policy_unavailable(
        &mut self,
        plugin: PluginId,
        reason: String,
        disable: bool,
    ) -> Result<()> {
        if self.notification_policy.plugin != Some(plugin) {
            return Ok(());
        }
        for pending in &mut self.notification_policy.pending {
            pending.fallback(&reason);
        }
        self.notification_policy.available = !disable;
        self.notification_policy.unavailable_reason = disable.then_some(reason.clone());
        self.release_notification_assemblies(&reason);
        self.commit_notifications()
    }

    fn release_notification_assemblies(&mut self, reason: &str) {
        let mut raw = vec![];
        for ((pane, _), assembly) in &mut self.notification_policy.assemblies {
            if !assembly.chunks.is_empty() {
                raw.push((*pane, std::mem::take(&mut assembly.chunks)));
                assembly.bytes = 0;
                assembly.bypass = Some(reason.into());
            }
        }
        for (pane, chunks) in raw {
            self.deliver_desktop_notifications(chunks, pane);
        }
    }

    pub(super) fn bind_notification_request(
        &mut self,
        id: NotificationId,
        plugin: PluginId,
        instance: PluginInstanceId,
    ) {
        if let Some(pending) = self
            .notification_policy
            .pending
            .iter_mut()
            .find(|p| p.original.id == id)
            && pending.policy == Some(plugin)
            && pending.instance.is_none()
            && pending.decision.is_none()
            && Instant::now() < pending.deadline
        {
            pending.instance = Some(instance);
        }
    }

    pub(super) fn resolve_notification(
        &mut self,
        id: NotificationId,
        decision: NotificationDecision,
        plugin: PluginId,
        instance: PluginInstanceId,
    ) -> Result<()> {
        let Some(pending) = self
            .notification_policy
            .pending
            .iter_mut()
            .find(|p| p.original.id == id)
        else {
            return Ok(());
        };
        if pending.policy != Some(plugin)
            || pending.instance != Some(instance)
            || pending.decision.is_some()
            || Instant::now() >= pending.deadline
        {
            return Ok(());
        }
        if let NotificationDecision::Replace { title, body, .. } = &decision
            && title.as_ref().unwrap_or(&pending.original.title).len()
                + body.as_ref().unwrap_or(&pending.original.body).len()
                > MAX_NOTIFICATION_REPLACEMENT_BYTES
        {
            pending.fallback("replacement_too_large");
            return self.commit_notifications();
        }
        pending.decision = Some(decision);
        self.commit_notifications()
    }

    pub(super) fn forget_notification_assemblies(&mut self, pane: PaneId) {
        if let PaneId::Terminal(pane) = pane {
            self.notification_policy
                .assemblies
                .retain(|(owner, _), _| *owner != pane);
        }
    }

    pub(super) fn notification_deadline(&self) -> Option<Duration> {
        self.notification_policy
            .pending
            .iter()
            .filter(|p| p.decision.is_none())
            .map(|p| p.deadline.saturating_duration_since(Instant::now()))
            .min()
    }

    pub(super) fn expire_notification_requests(&mut self, now: Instant) -> Result<()> {
        for pending in &mut self.notification_policy.pending {
            if pending.decision.is_none() && now >= pending.deadline {
                pending.fallback("policy_timeout");
            }
        }
        self.commit_notifications()
    }

    fn notification_data(
        &mut self,
        source: NotificationSource,
        pane_id: Option<PaneId>,
        plugin_id: Option<PluginId>,
        title: String,
        body: String,
    ) -> NotificationData {
        let id = self.next_notification_id;
        self.next_notification_id = NotificationId::new(id.get() + 1);
        NotificationData {
            id,
            received_at_unix_ms: UnixMillis::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or_else(|_| UnixMillis::new(u64::MAX)),
            source,
            pane_id,
            plugin_id,
            title,
            body,
            delivery: if source == NotificationSource::Toast {
                NotificationDelivery::Toast
            } else {
                NotificationDelivery::Desktop
            },
        }
    }

    pub(super) fn receive_toast(
        &mut self,
        text: String,
        pane: Option<PaneId>,
        click: Option<NotificationClick>,
        plugin: Option<PluginId>,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        let original =
            self.notification_data(NotificationSource::Toast, pane, plugin, String::new(), text);
        self.queue_notification(original, vec![], None, click, completion, None)
    }

    fn queue_notification(
        &mut self,
        original: NotificationData,
        desktop: Vec<PendingNotification>,
        osc99_identity: Option<(String, bool)>,
        on_click: Option<NotificationClick>,
        completion: Option<NotificationEnd>,
        fallback: Option<String>,
    ) -> Result<()> {
        self.bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                None,
                None,
                Event::NotificationReceived(original.clone()),
            )]))
            .context("publish notification arrival")?;
        let policy = self
            .notification_policy
            .plugin
            .filter(|id| self.notification_policy.available && original.plugin_id != Some(*id));
        let fallback = fallback.or_else(|| self.notification_policy.unavailable_reason.clone());
        let mut pending = Pending {
            original,
            desktop,
            osc99_identity,
            on_click,
            _completion: completion,
            policy,
            instance: None,
            deadline: Instant::now() + POLICY_TIMEOUT,
            decision: None,
            fallback,
        };
        let bytes = self
            .notification_policy
            .pending
            .iter()
            .map(Pending::bytes)
            .sum::<usize>()
            + self
                .notification_policy
                .assemblies
                .values()
                .map(|a| a.bytes)
                .sum::<usize>();
        if self.notification_policy.pending.len() >= MAX_PENDING
            || bytes.saturating_add(pending.bytes()) > MAX_BYTES
        {
            for pending in &mut self.notification_policy.pending {
                pending.fallback("pending_overflow");
            }
            self.commit_notifications()?;
            pending.fallback("pending_overflow");
        }
        if policy.is_none() || pending.fallback.is_some() {
            pending.decision = Some(NotificationDecision::Pass);
        } else if let Some(plugin) = policy {
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Update(vec![(
                    Some(plugin),
                    None,
                    Event::NotificationRequested(pending.original.clone()),
                )]))
                .context("request notification policy")?;
        }
        self.notification_policy.pending.push_back(pending);
        self.commit_notifications()
    }

    pub(super) fn forward_desktop_notifications(
        &mut self,
        notifications: Vec<PendingNotification>,
        pane: TerminalId,
    ) {
        self.receive_desktop_notifications(notifications, pane)
            .context("process desktop notifications")
            .non_fatal();
    }

    pub(super) fn receive_desktop_notifications(
        &mut self,
        notifications: Vec<PendingNotification>,
        pane: TerminalId,
    ) -> Result<()> {
        for notification in notifications {
            // Agent state is derived from original content, before policy changes.
            self.report_codex_plan_prompt(&notification, pane)
                .context("report Codex plan prompt")
                .non_fatal();
            let source = match &notification {
                PendingNotification::Osc9 { .. } => NotificationSource::Osc9,
                PendingNotification::Osc777 { .. } => NotificationSource::Osc777,
                PendingNotification::Osc99 { .. } => NotificationSource::Osc99,
            };
            let (title, body) = notification.title_and_body();
            let mut chunks = vec![];
            let mut osc99_identity = None;
            let mut fallback = None;
            if let PendingNotification::Osc99 {
                payload,
                display,
                wants_report,
                ..
            } = &notification
            {
                let (metadata, _) = split_osc99_payload(payload);
                let metadata = parse_osc99_metadata(metadata);
                let id = metadata.get("i").copied().unwrap_or_default().to_owned();
                let key = (pane, id.clone());
                let kind = metadata
                    .get("p")
                    .map(|p| Osc99PayloadType::from_metadata_value(p))
                    .unwrap_or(Osc99PayloadType::Title);
                if kind.is_control_request() {
                    if kind == Osc99PayloadType::Close {
                        self.notification_policy.assemblies.remove(&key);
                        for pending in &mut self.notification_policy.pending {
                            if pending.original.pane_id == Some(PaneId::Terminal(pane))
                                && pending
                                    .osc99_identity
                                    .as_ref()
                                    .is_some_and(|(pending_id, _)| pending_id == &id)
                            {
                                pending.decision = Some(NotificationDecision::Suppress);
                                pending.fallback = Some("application_closed".into());
                            }
                        }
                        self.commit_notifications()?;
                    }
                    self.deliver_desktop_notifications(vec![notification], pane);
                    continue;
                }
                let is_done = metadata.get("d").copied().unwrap_or("1") != "0";
                // Bound raw assemblies and bypass markers in both policy states.
                // Existing IDs must keep their markers until completion or cleanup.
                if (self.notification_policy.available || !is_done)
                    && !self.notification_policy.assemblies.contains_key(&key)
                    && self
                        .notification_policy
                        .assemblies
                        .keys()
                        .filter(|(p, _)| *p == pane)
                        .count()
                        >= 512
                {
                    let oldest = self
                        .notification_policy
                        .assemblies
                        .iter()
                        .filter(|((p, _), a)| *p == pane && a.bypass.is_some())
                        .min_by_key(|(_, a)| a.sequence)
                        .map(|(key, _)| key.clone());
                    if let Some(oldest) = oldest {
                        self.notification_policy.assemblies.remove(&oldest);
                    }
                }
                if !self.notification_policy.available {
                    if !is_done {
                        self.notification_policy.next_assembly += 1;
                        let sequence = self.notification_policy.next_assembly;
                        let reason = self
                            .notification_policy
                            .unavailable_reason
                            .clone()
                            .unwrap_or_else(|| "policy_unavailable".into());
                        self.notification_policy
                            .assemblies
                            .entry(key.clone())
                            .or_insert_with(|| Assembly {
                                sequence,
                                bypass: Some(reason),
                                ..Default::default()
                            });
                    }
                    if is_done && display.is_some() {
                        // Publish the complete arrival before sending its final display chunk.
                        chunks.push(notification.clone());
                    } else {
                        self.deliver_desktop_notifications(vec![notification.clone()], pane);
                    }
                    if is_done {
                        fallback = self
                            .notification_policy
                            .assemblies
                            .remove(&key)
                            .and_then(|assembly| assembly.bypass);
                    }
                } else {
                    // Match the grid's existing 256-ID bound per pane.
                    if !self.notification_policy.assemblies.contains_key(&key)
                        && self
                            .notification_policy
                            .assemblies
                            .iter()
                            .filter(|((p, _), assembly)| *p == pane && assembly.bypass.is_none())
                            .count()
                            >= 256
                    {
                        let oldest = self
                            .notification_policy
                            .assemblies
                            .iter()
                            .filter(|((p, _), assembly)| *p == pane && assembly.bypass.is_none())
                            .min_by_key(|(_, a)| a.sequence)
                            .map(|(k, _)| k.clone());
                        if let Some(oldest) = oldest
                            && let Some(assembly) =
                                self.notification_policy.assemblies.get_mut(&oldest)
                        {
                            assembly.bypass = Some("assembly_overflow".into());
                            assembly.bytes = 0;
                            let chunks = std::mem::take(&mut assembly.chunks);
                            self.deliver_desktop_notifications(chunks, pane);
                        }
                    }
                    let total_bytes = self
                        .notification_policy
                        .pending
                        .iter()
                        .map(Pending::bytes)
                        .sum::<usize>()
                        + self
                            .notification_policy
                            .assemblies
                            .values()
                            .map(|a| a.bytes)
                            .sum::<usize>();
                    self.notification_policy.next_assembly += 1;
                    let sequence = self.notification_policy.next_assembly;
                    let assembly = self
                        .notification_policy
                        .assemblies
                        .entry(key.clone())
                        .or_insert_with(|| Assembly {
                            sequence,
                            ..Default::default()
                        });
                    if let Some(reason) = &assembly.bypass {
                        fallback = Some(reason.clone());
                        self.deliver_desktop_notifications(vec![notification.clone()], pane);
                    } else {
                        assembly.bytes += raw_bytes(&notification);
                        assembly.chunks.push(notification.clone());
                        if assembly.bytes > MAX_ASSEMBLY
                            || total_bytes.saturating_add(raw_bytes(&notification)) > MAX_BYTES
                        {
                            assembly.bypass = Some("assembly_overflow".into());
                            assembly.bytes = 0;
                            let raw = std::mem::take(&mut assembly.chunks);
                            self.deliver_desktop_notifications(raw, pane);
                            fallback = Some("assembly_overflow".into());
                        }
                    }
                    if is_done
                        && let Some(assembly) = self.notification_policy.assemblies.remove(&key)
                    {
                        chunks = assembly.chunks;
                        fallback = assembly.bypass.or(fallback);
                    }
                }
                if !is_done {
                    continue;
                }
                if display.is_none() {
                    // Non-display payloads (for example an icon with no title/body) retain
                    // their existing host protocol behavior.
                    self.deliver_desktop_notifications(chunks, pane);
                    continue;
                }
                osc99_identity = Some((id, *wants_report));
            } else {
                chunks.push(notification);
            }
            let original =
                self.notification_data(source, Some(PaneId::Terminal(pane)), None, title, body);
            self.queue_notification(original, chunks, osc99_identity, None, None, fallback)?;
        }
        Ok(())
    }

    fn deliver_policy_desktop(&mut self, outcome: &NotificationResolution) {
        let clients: std::collections::HashSet<_> =
            self.connected_clients.borrow().iter().copied().collect();
        if clients.is_empty() {
            return;
        }
        let mut output = Output::default();
        output.add_clients(
            &clients,
            std::rc::Rc::new(std::cell::RefCell::new(
                crate::panes::link_handler::LinkHandler::new(),
            )),
            None,
        );
        let title = yinz_types::text::clean_string_from_control_and_linebreak(&outcome.title);
        let body = yinz_types::text::clean_string_from_control_and_linebreak(&outcome.body);
        for client in clients {
            let protocol = self.notification_protocol_for_client(&client);
            let raw = if protocol == NotificationProtocol::Osc99 {
                Some(format!("\u{1b}]99;i=yinz-notification-{}:a=report:p=title:d=0;{}\u{7}\u{1b}]99;i=yinz-notification-{}:a=report:p=body:d=1;{}\u{7}", outcome.original.id, title, outcome.original.id, body))
            } else {
                protocol.render(&title, &body)
            };
            if let Some(raw) = raw {
                output.add_post_vte_instruction_to_multiple_clients(std::iter::once(client), &raw);
            }
        }
        match output.serialize() {
            Ok(output) if !output.is_empty() => self
                .bus
                .senders
                .send_to_server(ServerInstruction::Render(Some(output)))
                .context("deliver policy desktop notification")
                .non_fatal(),
            Ok(_) => {},
            Err(error) => Err::<(), _>(error)
                .context("serialize policy desktop notification")
                .non_fatal(),
        }
    }

    pub(super) fn activate_policy_desktop(&mut self, raw: &[u8], client: crate::ClientId) -> bool {
        let Ok(raw) = std::str::from_utf8(raw) else {
            return false;
        };
        let (metadata, _) = split_osc99_payload(raw);
        let metadata = parse_osc99_metadata(metadata);
        let Some(id) = metadata
            .get("i")
            .and_then(|id| id.strip_prefix("yinz-notification-"))
            .and_then(|id| id.parse::<u64>().ok())
            .map(NotificationId::new)
        else {
            return false;
        };
        // Query/alive/close responses are not clicks.
        if metadata
            .get("p")
            .is_none_or(|p| !Osc99PayloadType::from_metadata_value(p).is_control_request())
            && let Some(record) = self
                .notification_history
                .iter()
                .find(|record| record.info.id == id)
        {
            self.activate_notification(
                id,
                record.pane_id,
                record.on_click.clone(),
                client,
                Instant::now(),
            );
        }
        true
    }

    pub(super) fn consume_pane_desktop_notification(
        &mut self,
        pane_id: PaneId,
        raw: &[u8],
        now: Instant,
    ) {
        let Ok(raw) = std::str::from_utf8(raw) else {
            return;
        };
        let (metadata, _) = split_osc99_payload(raw);
        let metadata = parse_osc99_metadata(metadata);
        let Some(forwarded_id) = metadata.get("i") else {
            return;
        };
        let target = self
            .notification_history
            .iter()
            .rev()
            .find(|record| {
                record.pane_id == Some(pane_id)
                    && record.forwarded_id.as_deref() == Some(*forwarded_id)
                    && record.info.outcome.as_ref().is_some_and(|outcome| {
                        !outcome.suppressed
                            && matches!(
                                outcome.delivery,
                                NotificationDelivery::Desktop | NotificationDelivery::Both
                            )
                    })
            })
            .map(|record| record.info.id);
        if let Some(id) = target {
            self.consume_notification(id, now);
            self.dismiss_notification(id, now).non_fatal();
        }
    }

    fn commit_notifications(&mut self) -> Result<()> {
        let mut changed = false;
        while self
            .notification_policy
            .pending
            .front()
            .is_some_and(|p| p.decision.is_some())
        {
            let Some(mut pending) = self.notification_policy.pending.pop_front() else {
                break;
            };
            let decision = pending
                .decision
                .take()
                .unwrap_or(NotificationDecision::Pass);
            let replaced = matches!(decision, NotificationDecision::Replace { .. });
            let outcome =
                NotificationResolution::from_decision(pending.original, decision, pending.fallback);
            let id = outcome.original.id;
            let pane = outcome.original.pane_id;
            let desktop = if replaced && outcome.original.source == NotificationSource::Osc99 {
                rewrite_desktop(&pending.desktop, &outcome)
            } else {
                pending.desktop
            };
            let forwarded_id = match (pane, pending.osc99_identity) {
                (Some(PaneId::Terminal(terminal)), Some((id, wants_report))) => {
                    let metadata =
                        namespace_notification_id(&format!("i={id}"), terminal, wants_report);
                    parse_osc99_metadata(&metadata)
                        .get("i")
                        .map(|id| (*id).to_owned())
                },
                _ => None,
            };
            let text = outcome.message();
            let now = Instant::now();
            let wants_toast = !outcome.suppressed
                && matches!(
                    outcome.delivery,
                    NotificationDelivery::Toast | NotificationDelivery::Both
                );
            // No client to show it to. Before the first one ever connects the toast waits for
            // it; once the session has had a client, a detached session only records it.
            let show_toast = wants_toast && !self.connected_clients.borrow().is_empty();
            if wants_toast
                && !show_toast
                && let Some(held) = self.toasts_before_first_client.as_mut()
            {
                held.push(id);
            }
            self.with_notification_stack_change(now, |screen| {
                screen.notification_history.push(NotificationRecord {
                    info: NotificationInfo {
                        id,
                        received_at_unix_ms: outcome.original.received_at_unix_ms,
                        message: text.clone(),
                        outcome: Some(outcome.clone()),
                    },
                    pane_id: pane,
                    on_click: pending.on_click.clone(),
                    pending: !outcome.suppressed,
                    forwarded_id,
                });
                if show_toast {
                    screen
                        .notifications
                        .insert(0, Notification::new(id, text, pane, pending.on_click, now));
                }
            });
            self.notification_browsers_saw_arrival();
            self.sync_animation_clock(now)?;
            if !outcome.suppressed {
                if show_toast {
                    self.bus
                        .senders
                        .send_to_background_jobs(BackgroundJob::DismissNotification(
                            id,
                            self.notification_timeout_secs,
                        ))
                        .context("schedule notification dismissal")?;
                }
                if matches!(
                    outcome.delivery,
                    NotificationDelivery::Desktop | NotificationDelivery::Both
                ) {
                    if let Some(PaneId::Terminal(terminal)) = pane.filter(|_| {
                        outcome.original.source != NotificationSource::Toast
                            && (!replaced || outcome.original.source == NotificationSource::Osc99)
                    }) {
                        self.deliver_desktop_notifications(desktop, terminal);
                    } else {
                        self.deliver_policy_desktop(&outcome);
                    }
                }
            }
            self.bus
                .senders
                .send_to_plugin(PluginInstruction::Update(vec![(
                    None,
                    None,
                    Event::NotificationResolved(outcome),
                )]))
                .context("publish resolved notification")?;
            changed = true;
        }
        if changed {
            self.render()?;
        }
        Ok(())
    }
}

impl Screen {
    /// A client has just been registered. If it is the session's first, it is shown the toasts
    /// committed before it, unless something consumed them meanwhile, each with its full
    /// timeout since nobody has seen it yet. Later clients find nothing held.
    pub(super) fn show_toasts_held_for_the_first_client(&mut self) -> Result<()> {
        let Some(held) = self.toasts_before_first_client.take() else {
            return Ok(());
        };
        let toasts: Vec<Notification> = held
            .iter()
            .filter_map(|id| {
                self.notification_history
                    .iter()
                    .find(|record| record.info.id == *id && record.pending)
            })
            .map(|record| {
                Notification::new(
                    record.info.id,
                    record.info.message.clone(),
                    record.pane_id,
                    record.on_click.clone(),
                    Instant::now(),
                )
            })
            .collect();
        if toasts.is_empty() {
            return Ok(());
        }
        let now = Instant::now();
        let ids: Vec<NotificationId> = toasts.iter().map(|toast| toast.id).collect();
        self.with_notification_stack_change(now, |screen| {
            // Newest first, as `commit_notifications` stacks them.
            for toast in toasts {
                screen.notifications.insert(0, toast);
            }
        });
        self.sync_animation_clock(now)?;
        for id in ids {
            self.bus
                .senders
                .send_to_background_jobs(BackgroundJob::DismissNotification(
                    id,
                    self.notification_timeout_secs,
                ))
                .context("schedule notification dismissal")?;
        }
        Ok(())
    }
}

fn rewrite_desktop(
    raw: &[PendingNotification],
    outcome: &NotificationResolution,
) -> Vec<PendingNotification> {
    let title = yinz_types::text::clean_string_from_control_and_linebreak(&outcome.title);
    let body = yinz_types::text::clean_string_from_control_and_linebreak(&outcome.body);
    let mut metadata = BTreeMap::new();
    let mut retained = vec![];
    let mut terminator = "\u{7}".to_owned();
    let mut wants_report = false;
    for chunk in raw {
        if let PendingNotification::Osc99 {
            payload,
            terminator: end,
            wants_report: report,
            ..
        } = chunk
        {
            let (params, content) = split_osc99_payload(payload);
            let fields = parse_osc99_metadata(params);
            if fields.get("p").is_some_and(|p| {
                Osc99PayloadType::from_metadata_value(p) == Osc99PayloadType::Other
            }) {
                // Preserve non-text payloads, but complete only after the rewritten body.
                let params = params
                    .split(':')
                    .filter(|field| !field.starts_with("d="))
                    .collect::<Vec<_>>()
                    .join(":");
                retained.push(PendingNotification::Osc99 {
                    payload: format!("{params}:d=0;{content}"),
                    terminator: end.clone(),
                    wants_report: *report,
                    display: None,
                });
            }
            for (k, v) in fields {
                metadata.insert(k.to_owned(), v.to_owned());
            }
            terminator = end.clone();
            wants_report = *report;
        }
    }
    for key in ["p", "e", "d"] {
        metadata.remove(key);
    }
    let params = metadata
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(":");
    retained.extend([
        PendingNotification::Osc99 {
            payload: format!("{params}:p=title:d=0:e=0;{title}"),
            terminator: terminator.clone(),
            wants_report,
            display: None,
        },
        PendingNotification::Osc99 {
            payload: format!("{params}:p=body:d=1:e=0;{body}"),
            terminator,
            wants_report,
            display: Some((title, body)),
        },
    ]);
    retained
}
