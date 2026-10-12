use crate::pane::Pane;

use crate::{os_input_output::ServerOsApi, panes::PaneId, ClientId};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Somewhere a `PaneId` can be resolved to a pane, so that focus bookkeeping does not care how the
/// collection that owns the pane chooses to store it.
///
/// `ActivePanes` only ever *reads* a pane, to ask it for its focus/unfocus escape sequence. It used
/// to take `&mut BTreeMap<PaneId, Box<dyn Pane>>` and so could only serve a collection that stores
/// bare panes; `OverlayPanes` stores each pane beside its `OverlayKind` and coordinates, which is
/// the whole point of that collection existing.
pub trait PaneLookup {
    #[allow(clippy::borrowed_box)]
    fn pane(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>>;
}

impl PaneLookup for BTreeMap<PaneId, Box<dyn Pane>> {
    fn pane(&self, pane_id: &PaneId) -> Option<&Box<dyn Pane>> {
        self.get(pane_id)
    }
}

#[derive(Clone)]
pub struct ActivePanes {
    active_panes: HashMap<ClientId, PaneId>,
    last_panes: HashMap<ClientId, PaneId>,
    os_api: Box<dyn ServerOsApi>,
}

impl std::fmt::Debug for ActivePanes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.active_panes)
    }
}

impl ActivePanes {
    #[allow(clippy::borrowed_box)]
    pub fn new(os_api: &Box<dyn ServerOsApi>) -> Self {
        let os_api = os_api.clone();
        ActivePanes {
            active_panes: HashMap::new(),
            last_panes: HashMap::new(),
            os_api,
        }
    }
    pub fn get(&self, client_id: &ClientId) -> Option<&PaneId> {
        self.active_panes.get(client_id)
    }
    pub fn get_last(&self, client_id: &ClientId) -> Option<&PaneId> {
        self.last_panes.get(client_id)
    }
    pub fn insert(&mut self, client_id: ClientId, pane_id: PaneId, panes: &impl PaneLookup) {
        if panes.pane(&pane_id).is_none() {
            return;
        }
        self.unfocus_pane_for_client(client_id, panes);
        self.active_panes.insert(client_id, pane_id);
        self.focus_pane(pane_id, panes);
    }
    pub fn set_last_pane(&mut self, client_id: ClientId, pane_id: PaneId) {
        self.last_panes.insert(client_id, pane_id);
    }
    pub fn clear(&mut self, panes: &impl PaneLookup) {
        for pane_id in self.active_panes.values() {
            self.unfocus_pane(*pane_id, panes);
        }
        self.active_panes.clear();
    }
    pub fn is_empty(&self) -> bool {
        self.active_panes.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ClientId, &PaneId)> {
        self.active_panes.iter()
    }
    pub fn values(&self) -> impl Iterator<Item = &PaneId> {
        self.active_panes.values()
    }
    pub fn remove(&mut self, client_id: &ClientId, panes: &impl PaneLookup) -> Option<PaneId> {
        if let Some(pane_id_to_unfocus) = self.active_panes.get(client_id) {
            self.unfocus_pane(*pane_id_to_unfocus, panes);
        }
        self.active_panes.remove(client_id)
    }
    pub fn unfocus_all_panes(
        &self,
        panes: &impl PaneLookup,
        connected_clients: &HashSet<ClientId>,
    ) {
        for pane_id in self.panes_focused_by(connected_clients) {
            self.unfocus_pane(pane_id, panes);
        }
    }
    pub fn focus_all_panes(&self, panes: &impl PaneLookup, connected_clients: &HashSet<ClientId>) {
        for pane_id in self.panes_focused_by(connected_clients) {
            self.focus_pane(pane_id, panes);
        }
    }
    fn panes_focused_by(&self, connected_clients: &HashSet<ClientId>) -> BTreeSet<PaneId> {
        self.active_panes
            .iter()
            .filter(|(client_id, _)| connected_clients.contains(client_id))
            .map(|(_, pane_id)| *pane_id)
            .collect()
    }
    pub fn retarget_last_pane(&mut self, from: PaneId, to: PaneId) {
        for pane_id in self.last_panes.values_mut() {
            if *pane_id == from {
                *pane_id = to;
            }
        }
    }
    pub fn remove_last_pane(&mut self, pane_id: PaneId) {
        self.last_panes.retain(|_, last| *last != pane_id);
    }
    pub fn clone_active_panes(&self) -> HashMap<ClientId, PaneId> {
        self.active_panes.clone()
    }
    pub fn contains_key(&self, client_id: &ClientId) -> bool {
        self.active_panes.contains_key(client_id)
    }
    fn unfocus_pane_for_client(&self, client_id: ClientId, panes: &impl PaneLookup) {
        if let Some(pane_id_to_unfocus) = self.active_panes.get(&client_id) {
            self.unfocus_pane(*pane_id_to_unfocus, panes);
        }
    }
    fn unfocus_pane(&self, pane_id: PaneId, panes: &impl PaneLookup) {
        if let PaneId::Terminal(terminal_id) = pane_id {
            if let Some(focus_event) = panes.pane(&pane_id).and_then(|p| p.unfocus_event()) {
                let _ = self
                    .os_api
                    .write_to_tty_stdin(terminal_id, focus_event.as_bytes());
            }
        }
    }
    fn focus_pane(&self, pane_id: PaneId, panes: &impl PaneLookup) {
        if let PaneId::Terminal(terminal_id) = pane_id {
            if let Some(focus_event) = panes.pane(&pane_id).and_then(|p| p.focus_event()) {
                let _ = self
                    .os_api
                    .write_to_tty_stdin(terminal_id, focus_event.as_bytes());
            }
        }
    }
    pub fn pane_id_is_focused(&self, pane_id: &PaneId) -> bool {
        self.active_panes
            .values()
            .find(|p_id| **p_id == *pane_id)
            .is_some()
    }
}
