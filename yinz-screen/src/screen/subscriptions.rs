use std::collections::{HashMap, HashSet};

use yinz_types::data::{PaneContents, PaneRenderReport};
use {yinz_ipc::ipc::ExitReason, yinz_ipc::ipc::ServerToClientMsg};

use crate::{panes::PaneId, ClientId};

use super::{Pane, PaneRenderSubscription, Screen};

impl Screen {
    fn subscriber_pane_contents(
        &self,
        pane: &dyn Pane,
        get_full_scrollback: bool,
        max_lines: Option<usize>,
        ansi: bool,
        report: Option<&PaneRenderReport>,
    ) -> PaneContents {
        let contents = |client| {
            if ansi {
                pane.pane_contents_with_ansi(client, get_full_scrollback, max_lines)
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
                    .cloned()
                    .unwrap_or_else(|| pane.pane_contents(client, get_full_scrollback, max_lines))
            }
        };
        if matches!(pane.pid(), PaneId::Terminal(_)) {
            return contents(None);
        }
        // Include live clients on other tabs: an inactive plugin can retain their grids.
        let mut clients: Vec<_> = self.connected_clients.borrow().iter().copied().collect();
        clients.sort_unstable();
        clients
            .into_iter()
            .chain(self.followed_client_id)
            .filter(|id| !self.watcher_clients.contains_key(id))
            .find_map(|id| {
                let contents = contents(Some(id));
                (!contents.viewport.is_empty()).then_some(contents)
            })
            .unwrap_or_default()
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

        for pane_id in &pane_ids {
            let server_pane_id: PaneId = *pane_id;
            let mut found = false;

            for tab in self.tabs.values() {
                if let Some(pane) = tab.get_pane_with_id(server_pane_id) {
                    let get_full_scrollback = scrollback.is_some();
                    let max_lines = scrollback.filter(|&n| n != 0);

                    let contents = self.subscriber_pane_contents(
                        pane,
                        get_full_scrollback,
                        max_lines,
                        ansi,
                        None,
                    );

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

            if !found {
                if let Some(os_input) = &self.bus.os_input {
                    let _ = os_input.send_to_client(
                        subscriber_client_id,
                        ServerToClientMsg::LogError {
                            lines: vec![format!("Pane {} not found", pane_id)],
                        },
                    );
                }
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
        if !self.pane_subscriber_snapshots_dirty || self.deferred_subscriber_renders {
            return;
        }
        // Collect unique pane IDs across all subscribers
        let all_subscribed_ids: HashSet<yinz_types::data::PaneId> = self
            .pane_render_subscribers
            .values()
            .flat_map(|sub| sub.pane_ids.iter().copied())
            .collect();

        let has_ansi_subscribers = self.pane_render_subscribers.values().any(|s| s.ansi);

        // Reuse this pass's plain snapshots; missing panes and ANSI still need direct reads.
        let mut pane_map: HashMap<yinz_types::data::PaneId, PaneContents> = HashMap::new();
        let mut ansi_pane_map: HashMap<yinz_types::data::PaneId, PaneContents> = HashMap::new();
        for pane_id in &all_subscribed_ids {
            let server_pane_id: PaneId = *pane_id;
            for tab in self.tabs.values() {
                if let Some(pane) = tab.get_pane_with_id(server_pane_id) {
                    pane_map.insert(
                        *pane_id,
                        self.subscriber_pane_contents(pane, false, None, false, report),
                    );
                    if has_ansi_subscribers {
                        ansi_pane_map.insert(
                            *pane_id,
                            self.subscriber_pane_contents(pane, false, None, true, None),
                        );
                    }
                    break;
                }
            }
        }

        let ansi_map_ref = if has_ansi_subscribers {
            Some(&ansi_pane_map)
        } else {
            None
        };
        self.deliver_subscriber_updates_from_map(&pane_map, ansi_map_ref);
        self.pane_subscriber_snapshots_dirty = false;
    }
    pub(super) fn deliver_subscriber_updates_from_map(
        &mut self,
        pane_map: &HashMap<yinz_types::data::PaneId, PaneContents>,
        ansi_pane_map: Option<&HashMap<yinz_types::data::PaneId, PaneContents>>,
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
            if let Some(os_input) = &self.bus.os_input {
                if os_input
                    .send_to_client(*subscriber_id, msg.clone())
                    .is_err()
                {
                    dead_subscribers.push(*subscriber_id);
                }
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
            {
                if let Some(subscription) = self.pane_render_subscribers.get_mut(&subscriber_id) {
                    subscription.previous_viewports.insert(pane_id, viewport);
                }
            }
        }

        for id in dead_subscribers {
            self.pane_render_subscribers.remove(&id);
        }
    }
    pub(super) fn notify_pane_closed_to_subscribers(&mut self, pane_id: yinz_types::data::PaneId) {
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
