use crate::{
    channels::SenderWithContext, plugins::PluginInstruction, route::NotificationEnd, tab::Pane,
    ClientId,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use zellij_types::internal_ids::{PluginId, TerminalId};
use zellij_types::style::{Palette, Style};
use zellij_utils::{
    input::layout::Run,
    pane_size::{PaneGeom, SizeInPixels},
};

use super::{KittyImageStore, LinkHandler, PluginPane, SixelImageStore, TerminalPane};

pub(crate) struct PaneFactory;

impl PaneFactory {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn terminal(
        pid: TerminalId,
        position_and_size: PaneGeom,
        style: Style,
        terminal_position: usize,
        pane_name: String,
        link_handler: Rc<RefCell<LinkHandler>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        terminal_emulator_colors: Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
        initial_pane_title: Option<String>,
        invoked_with: Option<Run>,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        explicitly_disable_kitty_keyboard_protocol: bool,
        blocking_notification: Option<NotificationEnd>,
    ) -> Box<dyn Pane> {
        Box::new(TerminalPane::new(
            pid,
            position_and_size,
            style,
            terminal_position,
            pane_name,
            link_handler,
            character_cell_size,
            sixel_image_store,
            kitty_image_store,
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            initial_pane_title,
            invoked_with,
            debug,
            arrow_fonts,
            styled_underlines,
            osc8_hyperlinks,
            explicitly_disable_kitty_keyboard_protocol,
            blocking_notification,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plugin(
        pid: PluginId,
        position_and_size: PaneGeom,
        send_plugin_instructions: SenderWithContext<PluginInstruction>,
        title: String,
        pane_name: String,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        terminal_emulator_colors: Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
        link_handler: Rc<RefCell<LinkHandler>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        currently_connected_clients: Vec<ClientId>,
        style: Style,
        invoked_with: Option<Run>,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
    ) -> Box<dyn Pane> {
        Box::new(PluginPane::new(
            pid,
            position_and_size,
            send_plugin_instructions,
            title,
            pane_name,
            sixel_image_store,
            kitty_image_store,
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            link_handler,
            character_cell_size,
            currently_connected_clients,
            style,
            invoked_with,
            debug,
            arrow_fonts,
            styled_underlines,
        ))
    }
}
