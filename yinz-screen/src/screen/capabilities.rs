use super::Screen;
use crate::panes::alacritty_functions::xparse_color;
use crate::panes::kitty_graphics::KittyHostSupport;
use crate::panes::terminal_character::AnsiCode;
use crate::panes::PaneId;
use crate::ClientId;
use yinz_bus::plugins::PluginInstruction;
use yinz_ipc::ipc::PixelDimensions;
use yinz_runtime::errors::prelude::*;
use yinz_terminal::output::NativeRenderState;
use yinz_types::colors::PaletteColor;
use yinz_types::data::Event;
use yinz_types::internal_ids::PluginId;
use yinz_types::pane_size::SizeInPixels;
use yinz_types::render_negotiation as caps;

impl Screen {
    /// Returns whether the character cell size changed, so the caller can re-apply
    /// pty sizes: ptys created before the host reported its pixel dimensions were
    /// sized without them.
    pub fn update_pixel_dimensions(
        &mut self,
        client_id: ClientId,
        mut pixel_dimensions: PixelDimensions,
    ) -> bool {
        // Reports may precede the first size. Screen dispatch admitted the reporting attachment.
        let previous_character_cell_size = *self.core.character_cell_size.borrow();
        let nonzero = |size: &SizeInPixels| size.width > 0 && size.height > 0;
        // Geometry comes from the host terminal. Unknown/invalid sizes must not replace a
        // valid cell: both graphics renderers use these dimensions as divisors.
        pixel_dimensions.character_cell_size = pixel_dimensions.character_cell_size.filter(nonzero);
        pixel_dimensions.text_area_size = pixel_dimensions.text_area_size.filter(nonzero);
        self.core.pixel_dimensions.merge(pixel_dimensions);
        let previous_client_cell_size = self.cell_pixel_size(client_id);
        let dimensions = self
            .core
            .client_pixel_dimensions
            .entry(client_id)
            .or_default();
        dimensions.merge(pixel_dimensions);
        let cell_size = dimensions
            .character_cell_size
            .or_else(|| {
                // A resize must not divide an old text area by the new grid. Modern clients pair
                // each area report with its grid; legacy reports can also wait for the first size.
                if pixel_dimensions.text_area_size.is_none() && previous_client_cell_size.is_some()
                {
                    return previous_client_cell_size;
                }
                let area = dimensions.text_area_size?;
                let size = dimensions
                    .text_area_grid_size
                    .or_else(|| self.core.client_sizes.get(&client_id).copied())?;
                Some(SizeInPixels {
                    height: area.height.checked_div(size.rows)?,
                    width: area.width.checked_div(size.cols)?,
                })
                .filter(nonzero)
            })
            .or(previous_client_cell_size);
        if let Some(cell_size) = cell_size {
            *self.core.character_cell_size.borrow_mut() = Some(cell_size);
        }
        self.core
            .client_cell_pixel_sizes
            .insert(client_id, cell_size);
        if previous_client_cell_size != cell_size && self.connected_clients_contains(&client_id) {
            self.report_cell_pixel_size(None, client_id);
        }
        *self.core.character_cell_size.borrow() != previous_character_cell_size
    }

    pub(super) fn cell_pixel_size(&self, client_id: ClientId) -> Option<SizeInPixels> {
        self.core
            .client_cell_pixel_sizes
            .get(&client_id)
            .copied()
            .flatten()
    }

    pub(super) fn report_cell_pixel_size(&self, plugin_id: Option<PluginId>, client_id: ClientId) {
        self.core
            .bus
            .senders
            .send_to_plugin(PluginInstruction::Update(vec![(
                plugin_id,
                Some(client_id),
                Event::CellPixelSizeChanged(self.cell_pixel_size(client_id)),
            )]))
            .context("report client cell pixel size to plugins")
            .non_fatal();
    }

    /// Re-send every pane's pty size, so a character cell size learned after the
    /// panes were created reaches their ptys as pixel dimensions.
    pub fn resize_pty_all_panes(&mut self) -> Result<()> {
        for tab in self.core.tabs.values_mut() {
            tab.resize_pty_all_panes()
                .context("failed to re-apply pty sizes")?;
        }
        Ok(())
    }

    pub fn update_kitty_graphics_support(&mut self, client_id: ClientId, supported: bool) {
        // The server registers the first client's size before spawning its layout, but it joins
        // connected_clients only when that layout lands. Keep a probe that arrives in between.
        // Both registrations are removed on detach, so a departed client's reply stays ignored.
        if !self.connected_clients_contains(&client_id)
            && !self.core.client_sizes.contains_key(&client_id)
        {
            return;
        }
        log::info!(
            "kitty graphics: client {client_id} probe={supported}, protocol_enabled={}",
            self.core.support_kitty_graphics_protocol
        );
        self.core.kitty_probe_answers.insert(client_id, supported);
        self.apply_kitty_answer(client_id);
    }

    fn apply_kitty_answer(&mut self, client_id: ClientId) {
        let supported = self
            .core
            .native_render_clients
            .get(&client_id)
            .map(|s| s.agreement.features & caps::KITTY != 0)
            .unwrap_or_else(|| {
                self.core
                    .kitty_probe_answers
                    .get(&client_id)
                    .copied()
                    .unwrap_or(false)
            })
            && self.core.support_kitty_graphics_protocol;
        let previous = self
            .core
            .kitty_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        if previous != Some(supported) {
            if let Some(state) = self.core.native_render_clients.get_mut(&client_id) {
                state.force_repaint();
                self.core
                    .client_kitty_host_state
                    .borrow_mut()
                    .remove(&client_id);
            }
            // A per-client gate may have consumed the only plugin frame, even when another
            // client already supported graphics. Repaint retained pane content.
            for tab in self.core.tabs.values_mut() {
                tab.set_force_render();
            }
        }
        self.push_kitty_host_support_to_tabs();
        self.push_kitty_capability_to_plugins(client_id, supported);
    }

    /// Re-derive every client's kitty graphics capability from its probe answer, after a reload
    /// changed `support_kitty_graphics_protocol`.
    pub(super) fn reapply_kitty_graphics_support(&mut self) {
        let clients: std::collections::HashSet<_> = self
            .core
            .kitty_probe_answers
            .keys()
            .chain(self.core.native_render_clients.keys())
            .copied()
            .collect();
        for client in clients {
            self.apply_kitty_answer(client);
        }
        self.push_kitty_host_support_to_tabs();
    }

    pub(super) fn kitty_host_support_aggregate(&self) -> Option<KittyHostSupport> {
        if !self.core.support_kitty_graphics_protocol {
            return Some(KittyHostSupport::ProtocolDisabled);
        }
        let capabilities = self.core.kitty_host_capabilities.borrow();
        if capabilities.is_empty() {
            None
        } else {
            Some(KittyHostSupport::from_host_capability(
                capabilities.values().any(|supported| *supported),
            ))
        }
    }

    pub(super) fn push_kitty_host_support_to_tabs(&mut self) {
        if let Some(aggregate) = self.kitty_host_support_aggregate() {
            for tab in self.core.tabs.values_mut() {
                tab.update_kitty_host_support(aggregate);
            }
        }
    }

    /// Plugins learn whether an image is worth drawing from `ModeInfo`, which - unlike the pane
    /// side above - is per client and cached in three places. The capability is not known when
    /// `ModeInfo` is built (`thread_main.rs`, from config, before anyone attaches); it arrives with
    /// the client's probe reply, so it has to be written back into each copy and re-fanned out.
    /// Same shape as the host-theme swap in `Screen::apply_host_terminal_theme_mode`.
    fn push_kitty_capability_to_plugins(&mut self, client_id: ClientId, supported: bool) {
        let default_for_new = self.core.default_mode_info.clone();
        let mode_info = self
            .core
            .mode_info
            .entry(client_id)
            .or_insert(default_for_new);
        mode_info.capabilities.kitty_graphics = supported;
        let mode_info = mode_info.clone();
        // `update_input_modes` fans out `ModeUpdate` from the tabs' per-client map, not from
        // Screen's, so without this step plugins would keep being told the old answer.
        for tab in self.core.tabs.values_mut() {
            tab.change_mode_info(mode_info.clone(), client_id);
        }
        for tab in self.core.tabs.values_mut() {
            tab.update_input_modes()
                .context("failed to push kitty graphics support to plugins")
                .non_fatal();
        }
    }

    pub fn update_sixel_support(&mut self, client_id: ClientId, supported: bool) -> Result<()> {
        let previous = self.core.sixel_probe_answers.insert(client_id, supported);
        self.apply_sixel_answer(client_id);
        if previous.unwrap_or(false) != supported
            && let Some(watcher) = self.core.watcher_clients.get_mut(&client_id)
        {
            watcher.set_force_render();
            self.render()
                .context("repaint watcher after Sixel capability change")?;
        }
        Ok(())
    }

    fn apply_sixel_answer(&mut self, client_id: ClientId) {
        let supported = self
            .core
            .native_render_clients
            .get(&client_id)
            .map(|s| s.agreement.features & caps::SIXEL != 0)
            .unwrap_or_else(|| {
                self.core
                    .sixel_probe_answers
                    .get(&client_id)
                    .copied()
                    .unwrap_or(false)
            });
        self.core
            .sixel_host_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        self.push_sixel_host_support_to_tabs();
    }

    fn repaint_render_profile(&mut self, client_id: ClientId) {
        self.core
            .client_kitty_host_state
            .borrow_mut()
            .remove(&client_id);
        self.apply_kitty_answer(client_id);
        self.apply_sixel_answer(client_id);
        self.force_client_repaint(client_id);
    }

    pub(super) fn legacy_render_support(&mut self, client: ClientId, enabled: bool) {
        if !(self.core.client_sizes.contains_key(&client)
            || self.connected_clients_contains(&client))
            || self.core.retired_native_clients.contains_key(&client)
            || self
                .core
                .native_render_clients
                .get(&client)
                .is_some_and(|state| state.negotiated)
        {
            return;
        }
        if self.core.native_render_clients.contains_key(&client) == enabled {
            return;
        }
        if enabled {
            self.core
                .native_render_clients
                .entry(client)
                .or_default()
                .seq = self
                .core
                .paused_legacy_sequences
                .remove(&client)
                .unwrap_or_default();
        } else if let Some(state) = self.core.native_render_clients.remove(&client) {
            self.core.paused_legacy_sequences.insert(client, state.seq);
        }
        self.repaint_render_profile(client);
    }

    pub(super) fn negotiate_render(
        &mut self,
        client: ClientId,
        request: caps::Request,
    ) -> Result<()> {
        use caps::{Decision, Request};
        use yinz_runtime::native_media::{self, MediaBudget, MediaRoot};
        if !self.core.client_sizes.contains_key(&client)
            && !self.connected_clients_contains(&client)
        {
            return Ok(());
        }
        let decision = match request {
            Request::Offer(_)
                if self.core.native_render_clients.contains_key(&client)
                    || self.core.paused_legacy_sequences.contains_key(&client)
                    || self.core.retired_native_clients.contains_key(&client) =>
            {
                Decision::Rejected(
                    "native profile already used; reattach to negotiate again".into(),
                )
            },
            Request::Offer(offer) => {
                let proven = offer.proof.as_ref().and_then(|proof| {
                    let root = MediaRoot::open(&native_media::directory()).ok()?;
                    let response = root.answer(proof).ok()?;
                    Some((root, response))
                });
                let (root, response) = proven.unzip();
                match offer.select(response) {
                    Ok(agreement) => {
                        let root = root.filter(|_| agreement.media & caps::SHARED != 0);
                        // Every negotiated viewer gets its own budget: shared staging draws on
                        // it now, and portable transfer buffers and assembled images will.
                        self.core.native_render_clients.insert(
                            client,
                            NativeRenderState::negotiated(
                                agreement.clone(),
                                root,
                                Some(MediaBudget::viewer()),
                            ),
                        );
                        Decision::Selected(agreement)
                    },
                    Err(reason) => {
                        self.core.retired_native_clients.entry(client).or_default();
                        Decision::Rejected(reason)
                    },
                }
            },
            Request::Withdraw => {
                if let Some(state) = self.core.native_render_clients.remove(&client) {
                    self.core.retired_native_clients.insert(client, state);
                } else if let Some(seq) = self.core.paused_legacy_sequences.remove(&client) {
                    self.core
                        .retired_native_clients
                        .entry(client)
                        .or_default()
                        .seq = seq;
                }
                Decision::Withdrawn(
                    self.core
                        .retired_native_clients
                        .entry(client)
                        .or_default()
                        .seq,
                )
            },
        };
        let repaint = !matches!(decision, Decision::Rejected(_));
        self.core
            .bus
            .senders
            .send_to_server(
                yinz_bus::server_instruction::ServerInstruction::NativeRenderResult(
                    client, decision,
                ),
            )
            .context("return native render decision")?;
        if repaint {
            self.repaint_render_profile(client);
            self.render()?;
        }
        Ok(())
    }

    /// Apply an accepted native ACK. When it releases media that deferred images were
    /// waiting for, re-render those panes so the images can be staged.
    pub(super) fn acknowledge_native_frame(&mut self, client: ClientId, seq: u64) -> Result<()> {
        let deferred = NativeRenderState::acknowledge_recipient(
            &mut self.core.native_render_clients,
            &mut self.core.retired_native_clients,
            client,
            seq,
        );
        if self.retry_deferred_panes(deferred) {
            self.render().context("retry deferred native images")?;
        }
        Ok(())
    }

    /// Mark each still-open pane for render. Returns whether any was marked.
    pub(super) fn retry_deferred_panes(&mut self, panes: impl IntoIterator<Item = PaneId>) -> bool {
        let mut marked = false;
        for pane_id in panes {
            // A pane closed since its frame has nothing left to show.
            let Some(tab) = self.core.tab_containing_pane_mut(pane_id) else {
                continue;
            };
            match tab.get_pane_with_id_mut(pane_id) {
                Some(pane) => pane.set_should_render(true),
                None => tab.set_force_render(),
            }
            marked = true;
        }
        marked
    }

    /// Remember whether this client's host presents each frame atomically (DECSET 2026).
    ///
    /// Like Kitty support, this takes the same route through the server's main loop as
    /// `AddClient` and `RemoveClient`, preserving their order. Watchers report too, and they
    /// are not in `connected_clients`, so no connected-client guard applies here.
    pub fn update_synchronized_output_support(
        &mut self,
        client_id: ClientId,
        supported: bool,
    ) -> Result<()> {
        self.core
            .synchronized_output_capabilities
            .borrow_mut()
            .insert(client_id, supported);
        // The watchers share one byte stream, hidden according to what every one of their hosts
        // can do (see `Screen::render_to_clients`), so one watcher's answer changes the frame the
        // others get. Rare enough to just repaint them all - and to ask for that repaint here,
        // since this answer arrives on its own and a session with nothing else to say would
        // otherwise leave the request armed and unspent.
        if self.core.watcher_clients.contains_key(&client_id) {
            for watcher_state in self.core.watcher_clients.values_mut() {
                watcher_state.set_force_render();
            }
            self.render()
                .context("failed to repaint the watchers after a synchronized-output answer")?;
        }
        Ok(())
    }

    pub(super) fn sixel_host_support_aggregate(&self) -> Option<bool> {
        let capabilities = self.core.sixel_host_capabilities.borrow();
        if capabilities.is_empty() {
            None
        } else {
            Some(capabilities.values().any(|supported| *supported))
        }
    }

    pub(super) fn push_sixel_host_support_to_tabs(&mut self) {
        if let Some(aggregate) = self.sixel_host_support_aggregate() {
            for tab in self.core.tabs.values_mut() {
                tab.update_sixel_host_support(aggregate);
            }
        }
    }

    pub fn update_terminal_background_color(&mut self, background_color_instruction: String) {
        if let Some(AnsiCode::RgbCode((r, g, b))) =
            xparse_color(background_color_instruction.as_bytes())
        {
            let bg_palette_color = PaletteColor::Rgb((r, g, b));
            self.core.terminal_emulator_colors.borrow_mut().bg = bg_palette_color;
        }
    }

    pub fn update_terminal_foreground_color(&mut self, foreground_color_instruction: String) {
        if let Some(AnsiCode::RgbCode((r, g, b))) =
            xparse_color(foreground_color_instruction.as_bytes())
        {
            let fg_palette_color = PaletteColor::Rgb((r, g, b));
            self.core.terminal_emulator_colors.borrow_mut().fg = fg_palette_color;
        }
    }

    pub fn update_terminal_color_registers(&mut self, color_registers: Vec<(usize, String)>) {
        let mut terminal_emulator_color_codes =
            self.core.terminal_emulator_color_codes.borrow_mut();
        for (color_register, color_sequence) in color_registers {
            terminal_emulator_color_codes.insert(color_register, color_sequence);
        }
    }
}
