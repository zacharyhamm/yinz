use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use yinz_types::data::{PaneContents, PaneRenderReport};
use {yinz_ipc::ipc::ExitReason, yinz_ipc::ipc::ServerToClientMsg};

use crate::{panes::PaneId, ClientId};

use super::{Pane, PaneRenderSubscription, Screen};

#[cfg(test)]
thread_local! {
    pub(super) static SUBSCRIBER_SNAPSHOT_READS: std::cell::RefCell<Vec<(PaneId, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Screen {
    pub(super) fn subscriber_pane_contents<'a>(
        pane: &dyn Pane,
        client: Option<ClientId>,
        get_full_scrollback: bool,
        max_lines: Option<usize>,
        ansi: bool,
        report: Option<&'a PaneRenderReport>,
    ) -> Cow<'a, PaneContents> {
        let direct_contents = |client| {
            #[cfg(test)]
            SUBSCRIBER_SNAPSHOT_READS.with_borrow_mut(|reads| reads.push((pane.pid(), ansi)));
            Cow::Owned(if ansi {
                pane.pane_contents_with_ansi(client, get_full_scrollback, max_lines)
            } else {
                pane.pane_contents(client, get_full_scrollback, max_lines)
            })
        };
        let contents = |client| {
            if ansi {
                direct_contents(client)
            } else {
                report
                    .and_then(|report| match client {
                        Some(client) => report
                            .all_pane_contents
                            .get(&client)
                            .and_then(|panes| panes.get(&pane.pid())),
                        None => report
                            .all_pane_contents
                            .values()
                            .find_map(|panes| panes.get(&pane.pid())),
                    })
                    .map(Cow::Borrowed)
                    .unwrap_or_else(|| direct_contents(client))
            }
        };
        if matches!(pane.pid(), PaneId::Terminal(_)) {
            return contents(None);
        }
        client
            .map(|client| contents(Some(client)))
            .unwrap_or_default()
    }

    fn subscriber_client(&self) -> Option<ClientId> {
        // Choose independently of pane contents, including cached grids on inactive tabs.
        self.connected_clients
            .borrow()
            .iter()
            .filter(|id| !self.watcher_clients.contains_key(id))
            .copied()
            .min()
            .or_else(|| {
                self.followed_client_id
                    .filter(|id| !self.watcher_clients.contains_key(id))
            })
    }

    pub(super) fn mark_subscriber_pane_dirty(&mut self, pane_id: PaneId) {
        if self
            .pane_render_subscribers
            .values()
            .any(|sub| sub.pane_ids.contains(&pane_id))
        {
            self.pane_subscriber_dirty_panes.insert(pane_id);
        }
    }

    pub(super) fn subscriber_snapshots_pending(&self) -> bool {
        !self.pane_render_subscribers.is_empty()
            && (self.pane_subscriber_snapshots_dirty
                || !self.pane_subscriber_dirty_panes.is_empty())
    }

    pub(super) fn subscribe_to_pane_renders(
        &mut self,
        subscriber_client_id: ClientId,
        pane_ids: Vec<yinz_types::data::PaneId>,
        scrollback: Option<usize>,
        ansi: bool,
    ) {
        let mut previous_viewports = HashMap::new();
        let mut valid_pane_ids = HashSet::new();
        let client = self.subscriber_client();

        for pane_id in &pane_ids {
            let server_pane_id: PaneId = *pane_id;
            let mut found = false;

            for tab in self.tabs.values() {
                if let Some(pane) = tab.get_pane_with_id(server_pane_id) {
                    let get_full_scrollback = scrollback.is_some();
                    let max_lines = scrollback.filter(|&n| n != 0);

                    let contents = Self::subscriber_pane_contents(
                        pane,
                        client,
                        get_full_scrollback,
                        max_lines,
                        ansi,
                        None,
                    )
                    .into_owned();

                    if let Some(os_input) = &self.bus.os_input {
                        let scrollback_data = if scrollback.is_some() {
                            Some(contents.lines_above_viewport.clone())
                        } else {
                            None
                        };
                        let _ = os_input.send_to_client(
                            subscriber_client_id,
                            ServerToClientMsg::PaneRenderUpdate {
                                pane_id: *pane_id,
                                viewport: contents.viewport.clone(),
                                scrollback: scrollback_data,
                                is_initial: true,
                            },
                        );
                    }

                    previous_viewports.insert(*pane_id, contents.viewport);
                    valid_pane_ids.insert(*pane_id);
                    found = true;
                    break;
                }
            }

            if !found && let Some(os_input) = &self.bus.os_input {
                let _ = os_input.send_to_client(
                    subscriber_client_id,
                    ServerToClientMsg::LogError {
                        lines: vec![format!("Pane {} not found", pane_id)],
                    },
                );
            }
        }

        if !valid_pane_ids.is_empty() {
            self.pane_render_subscribers.insert(
                subscriber_client_id,
                PaneRenderSubscription {
                    pane_ids: valid_pane_ids,
                    previous_viewports,
                    ansi,
                },
            );
        }
    }
    pub(super) fn deliver_to_pane_subscribers(&mut self, report: Option<&PaneRenderReport>) {
        if self.pane_render_subscribers.is_empty() {
            self.pane_subscriber_dirty_panes.clear();
            self.pane_subscriber_snapshots_dirty = false;
            return;
        }
        if !self.subscriber_snapshots_pending() || self.deferred_subscriber_renders {
            return;
        }
        let client = self.subscriber_client();
        let mut pane_map = HashMap::new();
        let mut ansi_pane_map = HashMap::new();
        for (ansi, map) in [(false, &mut pane_map), (true, &mut ansi_pane_map)] {
            let pane_ids: HashSet<_> = self
                .pane_render_subscribers
                .values()
                .filter(|sub| sub.ansi == ansi)
                .flat_map(|sub| sub.pane_ids.iter().copied())
                .filter(|id| {
                    self.pane_subscriber_snapshots_dirty
                        || self.pane_subscriber_dirty_panes.contains(id)
                })
                .collect();
            for pane_id in pane_ids {
                if let Some(pane) = self
                    .tabs
                    .values()
                    .find_map(|tab| tab.get_pane_with_id(pane_id))
                {
                    map.insert(
                        pane_id,
                        Self::subscriber_pane_contents(pane, client, false, None, ansi, report),
                    );
                }
            }
        }
        self.deliver_subscriber_updates_from_map(&pane_map, Some(&ansi_pane_map));
        self.pane_subscriber_dirty_panes.clear();
        self.pane_subscriber_snapshots_dirty = false;
    }
    pub(super) fn deliver_subscriber_updates_from_map(
        &mut self,
        pane_map: &HashMap<yinz_types::data::PaneId, Cow<'_, PaneContents>>,
        ansi_pane_map: Option<&HashMap<yinz_types::data::PaneId, Cow<'_, PaneContents>>>,
    ) {
        // Collect updates to send, avoiding borrow conflicts
        let mut updates_to_send: Vec<(ClientId, ServerToClientMsg)> = Vec::new();
        let mut dead_subscribers: Vec<ClientId> = Vec::new();

        for (subscriber_id, subscription) in &self.pane_render_subscribers {
            let effective_map = if subscription.ansi {
                ansi_pane_map.unwrap_or(pane_map)
            } else {
                pane_map
            };

            for pane_id in &subscription.pane_ids {
                if let Some(contents) = effective_map.get(pane_id) {
                    let changed = subscription
                        .previous_viewports
                        .get(pane_id)
                        .map(|prev| prev != &contents.viewport)
                        .unwrap_or(true);

                    if changed {
                        updates_to_send.push((
                            *subscriber_id,
                            ServerToClientMsg::PaneRenderUpdate {
                                pane_id: *pane_id,
                                viewport: contents.viewport.clone(),
                                scrollback: None,
                                is_initial: false,
                            },
                        ));
                    }
                }
            }
        }

        // Send updates and track dead subscribers
        for (subscriber_id, msg) in &updates_to_send {
            if let Some(os_input) = &self.bus.os_input
                && os_input
                    .send_to_client(*subscriber_id, msg.clone())
                    .is_err()
            {
                dead_subscribers.push(*subscriber_id);
            }
        }

        // Update previous viewports for successful sends
        for (subscriber_id, msg) in updates_to_send {
            if dead_subscribers.contains(&subscriber_id) {
                continue;
            }
            if let ServerToClientMsg::PaneRenderUpdate {
                pane_id, viewport, ..
            } = msg
                && let Some(subscription) = self.pane_render_subscribers.get_mut(&subscriber_id)
            {
                subscription.previous_viewports.insert(pane_id, viewport);
            }
        }

        for id in dead_subscribers {
            self.pane_render_subscribers.remove(&id);
        }
    }
    pub(super) fn notify_pane_closed_to_subscribers(&mut self, pane_id: yinz_types::data::PaneId) {
        self.pane_subscriber_dirty_panes.remove(&pane_id);
        let mut dead_subscribers = Vec::new();

        for (subscriber_id, subscription) in &mut self.pane_render_subscribers {
            if subscription.pane_ids.remove(&pane_id) {
                if let Some(os_input) = &self.bus.os_input {
                    let _ = os_input.send_to_client(
                        *subscriber_id,
                        ServerToClientMsg::SubscribedPaneClosed { pane_id },
                    );
                }
                if subscription.pane_ids.is_empty() {
                    if let Some(os_input) = &self.bus.os_input {
                        let _ = os_input.send_to_client(
                            *subscriber_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Normal,
                            },
                        );
                    }
                    dead_subscribers.push(*subscriber_id);
                }
            }
        }

        for id in dead_subscribers {
            self.pane_render_subscribers.remove(&id);
        }
    }
}
