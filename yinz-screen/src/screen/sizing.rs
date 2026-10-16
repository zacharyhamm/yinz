use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TabId;

use super::Screen;
use crate::ClientId;
use yinz_ipc::ipc::PixelDimensions;
use yinz_types::pane_size::Size;

impl Screen {
    pub fn set_client_size(&mut self, client_id: ClientId, size: Size) {
        self.client_sizes.insert(client_id, size);
        self.update_pixel_dimensions(client_id, PixelDimensions::default());
    }

    pub(super) fn session_size(&self) -> Option<Size> {
        let connected = self.connected_clients.borrow();
        connected
            .iter()
            .filter(|client_id| !self.watcher_clients.contains_key(client_id))
            .filter_map(|client_id| self.client_sizes.get(client_id).copied())
            .reduce(|acc, size| Size {
                rows: acc.rows.min(size.rows),
                cols: acc.cols.min(size.cols),
            })
    }

    pub(super) fn surface_size(&self) -> Size {
        self.session_size().unwrap_or(self.last_surface_size)
    }

    pub(super) fn remember_surface_size(&mut self) {
        if let Some(size) = self.session_size() {
            self.last_surface_size = size;
        }
    }

    #[cfg(test)]
    pub(super) fn resize_to_screen(&mut self, new_screen_size: Size) -> Result<()> {
        let client_ids: Vec<ClientId> = self
            .connected_clients
            .borrow()
            .iter()
            .copied()
            .chain(self.client_sizes.keys().copied())
            .collect();
        for client_id in client_ids {
            self.set_client_size(client_id, new_screen_size);
        }
        self.resize_to_clients()
    }

    pub fn resize_to_clients(&mut self) -> Result<()> {
        let err_context = || "failed to resize to client sizes".to_string();
        self.reflow_all_tabs().with_context(err_context)?;
        self.log_and_report_session_state()
            .with_context(err_context)?;
        self.render().with_context(err_context)
    }

    pub(super) fn size_for_client(&self, client_id: Option<ClientId>) -> Size {
        client_id
            .and_then(|client_id| self.client_sizes.get(&client_id).copied())
            .or_else(|| {
                self.get_first_client_id()
                    .and_then(|client_id| self.client_sizes.get(&client_id).copied())
            })
            .or_else(|| self.client_sizes.values().next().copied())
            .unwrap_or(self.last_surface_size)
    }

    pub(super) fn size_of_tab_of_client(&self, client_id: ClientId) -> Size {
        self.active_tab_ids
            .get(&client_id)
            .and_then(|tab_id| self.tabs.get(tab_id))
            .map(|tab| tab.size)
            .unwrap_or_else(|| self.size_for_client(Some(client_id)))
    }

    pub fn recompute_tab_size(&mut self, tab_id: TabId) -> Result<()> {
        self.reflow_tab(tab_id)
            .with_context(|| format!("failed to recompute size for tab {tab_id}"))
    }
}
