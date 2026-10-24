//! Instructions waiting for a tab, client, or pane to become available.

use super::agent_reports::{apply_pane_foreground_command, apply_queued_agent_report};
use super::{Screen, ScreenInstruction};
use crate::background_jobs::BackgroundJob;
use crate::panes::PaneId;
use crate::ClientId;
use std::collections::{HashMap, HashSet};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

// Reports and output waiting for creation are applied before newer instructions already in
// the queue. Re-enqueuing them at the tail would invert state and byte arrival order.
pub(super) fn replay_pending_pane_event(
    screen: &mut Screen,
    event: ScreenInstruction,
    pending: &mut PendingEvents,
) -> Result<()> {
    match event {
        ScreenInstruction::NewPane(
            pid,
            title,
            hold,
            invoked_with,
            placement,
            suppressed,
            target,
            completion,
            blocking,
        ) => {
            screen
                .place_new_pane(
                    pid,
                    title,
                    hold,
                    invoked_with,
                    placement,
                    suppressed,
                    target,
                    completion,
                    blocking,
                    pending,
                )
                .context("failed to replay pending pane placement")?;
        },
        ScreenInstruction::AgentReport(report, accepted_at) => {
            apply_queued_agent_report(screen, &report, accepted_at)?;
        },
        ScreenInstruction::UpdatePaneForegroundCommand(terminal_id, command, program) => {
            apply_pane_foreground_command(screen, terminal_id, command, program);
        },
        ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix) => {
            screen.with_pane(pane_id, |pane| pane.set_title_prefix(prefix));
        },
        ScreenInstruction::PtyBytes(pid, bytes) => {
            screen
                .handle_pty_bytes(pid, bytes, pending)
                .context("failed to replay buffered terminal output")?;
            let _ = screen
                .bus
                .senders
                .send_to_background_jobs(BackgroundJob::RenderToClients);
        },
        event => screen.bus.senders.send_to_screen(event)?,
    }
    Ok(())
}

/// Work the screen thread has accepted but cannot act on yet.
///
/// An instruction that names a tab, client or pane which does not exist yet is parked here and
/// replayed once the thing it names arrives; a tab switch requested while a layout is still
/// landing waits until the layout is applied. Owned by the screen thread's loop and handed to
/// the `Screen` methods that queue or drain it.
#[derive(Default)]
pub(super) struct PendingEvents {
    pub(super) tab_switches: HashSet<(TabId, ClientId)>,
    pub(super) waiting_for_tab: Vec<ScreenInstruction>,
    pub(super) waiting_for_client: Vec<ScreenInstruction>,
    pub(super) waiting_for_pane: HashMap<PaneId, Vec<ScreenInstruction>>,
}

impl PendingEvents {
    pub(super) fn remove_client_plugin_output(&mut self, client_id: ClientId) {
        self.waiting_for_pane.retain(|_, events| {
            events.retain_mut(|event| match event {
                ScreenInstruction::PluginBytes(assets) => {
                    assets.retain(|asset| asset.client_id != client_id);
                    !assets.is_empty()
                },
                ScreenInstruction::ShowPluginCursor(_, client, _) => *client != client_id,
                _ => true,
            });
            !events.is_empty()
        });
    }
}
