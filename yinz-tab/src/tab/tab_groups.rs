use super::*;

impl Tab {
    pub fn update_tab_groups(&mut self, enabled: bool) -> Result<()> {
        self.default_mode_info.tab_groups = Some(enabled);
        for mode in self.mode_info.borrow_mut().values_mut() {
            mode.tab_groups = Some(enabled);
        }
        self.sync_tab_bar_height()
    }

    pub(super) fn visible_tab_group_count(&self) -> usize {
        if self.default_mode_info.tab_groups == Some(false) {
            1
        } else {
            self.tab_group_count
        }
    }

    pub(super) fn sync_tab_bar_height(&mut self) -> Result<()> {
        if self
            .tiled_panes
            .tab_bar_needs_resize(self.visible_tab_group_count())
        {
            self.resize_whole_tab_to(self.size)
                .context("resize tab after tab group visibility change")?;
        }
        Ok(())
    }
}
