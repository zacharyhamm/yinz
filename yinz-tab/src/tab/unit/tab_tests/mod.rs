use super::{AcceptedPane, ExtractedPanePlacement, Tab};
use crate::pane_groups::PaneGroups;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::pty_writer::PtyWriteInstruction;
use crate::tab::CopyOptions;
use crate::{os_input_output::ServerOsApi, panes::PaneId, thread_bus::ThreadSenders, ClientId};
use std::path::PathBuf;
use yinz_runtime::channels::{unbounded, Receiver, SenderWithContext};
use yinz_runtime::errors::prelude::*;
use yinz_types::geometry::{Direction, Resize, ResizeStrategy};
use yinz_types::geometry::{SplitDirection, SplitSize};
use yinz_types::input::layout::TiledPaneLayout;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::TerminalId;
use yinz_types::multiplayer::AttachedClients;
use {yinz_types::data::NewPanePlacement, yinz_types::data::OverlayPlacement};

use crate::os_input_output::AsyncReader;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use yinz_ipc::ipc::IpcReceiverWithContext;
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::pane_size::{PaneGeom, Size, SizeInPixels, Viewport};
use yinz_types::style::{Palette, Style};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ServerToClientMsg, yinz_types::data::ModeInfo,
    yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction,
};

#[derive(Clone)]
struct FakeInputOutput {}

impl ServerOsApi for FakeInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        _id: TerminalId,
        _cols: u16,
        _rows: u16,
        _width_in_pixels: Option<u16>,
        _height_in_pixels: Option<u16>,
    ) -> Result<()> {
        // noop
        Ok(())
    }
    fn spawn_terminal(
        &self,
        _file_to_open: TerminalAction,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn write_to_tty_stdin(&self, _id: TerminalId, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn tcdrain(&self, _id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn kill(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn force_kill(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn send_to_client(&self, _client_id: ClientId, _msg: ServerToClientMsg) -> Result<()> {
        unimplemented!()
    }
    fn new_client(
        &mut self,
        _client_id: ClientId,
        _stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        unimplemented!()
    }
    fn remove_client(&mut self, _client_id: ClientId) -> Result<()> {
        unimplemented!()
    }
    fn load_palette(&self) -> Palette {
        unimplemented!()
    }
    fn get_cwd(&self, _pid: ProcessId) -> Option<PathBuf> {
        unimplemented!()
    }

    fn write_to_file(&mut self, _buf: String, _name: Option<String>) -> Result<()> {
        unimplemented!()
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: TerminalId,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigint(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
    fn send_sigcont(&self, _pid: ProcessId) -> Result<()> {
        unimplemented!()
    }
}

fn tab_resize_increase(tab: &mut Tab, id: ClientId) {
    tab.resize(id, ResizeStrategy::new(Resize::Increase, None))
        .unwrap();
}

fn tab_resize_left(tab: &mut Tab, id: ClientId) {
    tab.resize(
        id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
    )
    .unwrap();
}

fn tab_resize_down(tab: &mut Tab, id: ClientId) {
    tab.resize(
        id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
    )
    .unwrap();
}

fn tab_resize_up(tab: &mut Tab, id: ClientId) {
    tab.resize(
        id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Up)),
    )
    .unwrap();
}

fn tab_resize_right(tab: &mut Tab, id: ClientId) {
    tab.resize(
        id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
    )
    .unwrap();
}

fn create_new_tab(size: Size, stacked_resize: bool) -> Tab {
    create_new_tab_with_senders(
        size,
        stacked_resize,
        ThreadSenders::default().silently_fail_on_send(),
    )
}

/// `create_new_tab` with the given senders, which every pane container clones at construction.
fn create_new_tab_with_senders(size: Size, stacked_resize: bool, senders: ThreadSenders) -> Tab {
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput {});
    let max_panes = None;
    let mode_info = ModeInfo::default();
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(stacked_resize));
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let current_pane_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
    let currently_marking_pane_group = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let advanced_mouse_actions = true;
    let mouse_scroll_resize = true;
    let mut tab = Tab::new(
        TabId::new(index),
        TabPosition::new(position),
        name,
        size,
        character_cell_info,
        stacked_resize,
        Rc::new(RefCell::new(false)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        os_api,
        senders,
        max_panes,
        style,
        mode_info,
        draw_pane_frames,
        auto_layout,
        connected_clients,
        session_is_mirrored,
        Some(client_id),
        copy_options,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        (vec![], vec![]), // swap layouts
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_pane_group,
        currently_marking_pane_group,
        advanced_mouse_actions,
        mouse_scroll_resize,
        true,  // mouse_hover_effects
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        true,  // context_menu_override_agent_mouse
    );
    tab.apply_layout(
        TiledPaneLayout::default(),
        vec![],
        vec![(crate::TerminalId::new(1), None)],
        vec![],
        HashMap::new(),
        client_id,
        None,
    )
    .unwrap();
    tab
}

fn create_new_tab_with_layout(size: Size, layout: TiledPaneLayout) -> Tab {
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput {});
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = ModeInfo::default();
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let current_pane_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
    let currently_marking_pane_group = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let advanced_mouse_actions = true;
    let mouse_scroll_resize = true;
    let mut tab = Tab::new(
        TabId::new(index),
        TabPosition::new(position),
        name,
        size,
        character_cell_info,
        stacked_resize,
        Rc::new(RefCell::new(false)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        os_api,
        senders,
        max_panes,
        style,
        mode_info,
        draw_pane_frames,
        auto_layout,
        connected_clients,
        session_is_mirrored,
        Some(client_id),
        copy_options,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        (vec![], vec![]), // swap layouts
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_pane_group,
        currently_marking_pane_group,
        advanced_mouse_actions,
        mouse_scroll_resize,
        true,  // mouse_hover_effects
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        true,  // context_menu_override_agent_mouse
    );
    let mut new_terminal_ids = vec![];
    for i in 0..layout.extract_run_instructions().len() {
        new_terminal_ids.push((TerminalId::new(i as u32), None));
    }
    tab.apply_layout(
        layout,
        vec![],
        new_terminal_ids,
        vec![],
        HashMap::new(),
        client_id,
        None,
    )
    .unwrap();
    tab
}

fn create_new_tab_with_cell_size(
    size: Size,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
) -> Tab {
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput {});
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = ModeInfo::default();
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let stacked_resize = Rc::new(RefCell::new(true));
    let current_pane_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
    let currently_marking_pane_group = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let advanced_mouse_actions = true;
    let mouse_scroll_resize = true;
    let mut tab = Tab::new(
        TabId::new(index),
        TabPosition::new(position),
        name,
        size,
        character_cell_size,
        stacked_resize,
        Rc::new(RefCell::new(false)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        os_api,
        senders,
        max_panes,
        style,
        mode_info,
        draw_pane_frames,
        auto_layout,
        connected_clients,
        session_is_mirrored,
        Some(client_id),
        copy_options,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        (vec![], vec![]), // swap layouts
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_pane_group,
        currently_marking_pane_group,
        advanced_mouse_actions,
        mouse_scroll_resize,
        true,  // mouse_hover_effects
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        true,  // context_menu_override_agent_mouse
    );
    tab.apply_layout(
        TiledPaneLayout::default(),
        vec![],
        vec![(crate::TerminalId::new(1), None)],
        vec![],
        HashMap::new(),
        client_id,
        None,
    )
    .unwrap();
    tab
}

fn create_tab_with_two_floating_panes() -> Tab {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    tab
}

fn floating_pane_geom(tab: &Tab, pane_id: PaneId) -> PaneGeom {
    tab.floating_panes
        .get(&pane_id)
        .expect("floating pane exists")
        .current_geom()
}

fn create_tab_with_four_panes() -> Tab {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(crate::TerminalId::new(i));
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    }
    tab
}

fn tiled_pane_geoms(tab: &Tab) -> Vec<(PaneId, PaneGeom)> {
    tab.tiled_panes
        .panes
        .iter()
        .map(|(pane_id, pane)| (*pane_id, pane.position_and_size()))
        .collect()
}

fn open_pane_five(tab: &mut Tab) {
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(5)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .unwrap();
}

fn assert_panes_tile_the_whole_display(tab: &Tab, display: Size) {
    let geoms = tiled_pane_geoms(tab);
    let mut total_pane_area = 0;
    for (pane_id, geom) in &geoms {
        let right_edge = geom.x + geom.cols.as_usize();
        let bottom_edge = geom.y + geom.rows.as_usize();
        assert!(
            right_edge <= display.cols && bottom_edge <= display.rows,
            "{pane_id:?} fits inside the display: {geom:?}"
        );
        total_pane_area += geom.rows.as_usize() * geom.cols.as_usize();
    }
    for (pane_id, geom) in &geoms {
        for (other_pane_id, other_geom) in &geoms {
            if pane_id == other_pane_id {
                continue;
            }
            let overlap_horizontally = geom.x < other_geom.x + other_geom.cols.as_usize()
                && other_geom.x < geom.x + geom.cols.as_usize();
            let overlap_vertically = geom.y < other_geom.y + other_geom.rows.as_usize()
                && other_geom.y < geom.y + geom.rows.as_usize();
            assert!(
                !(overlap_horizontally && overlap_vertically),
                "{pane_id:?} and {other_pane_id:?} do not overlap"
            );
        }
    }
    assert_eq!(
        total_pane_area,
        display.rows * display.cols,
        "Panes cover the whole display with no gaps"
    );
}

fn install_pty_writer_capture(
    tab: &mut Tab,
) -> Receiver<(PtyWriteInstruction, yinz_types::diagnostics::ErrorContext)> {
    let (tx, rx) = unbounded();
    tab.senders
        .replace_to_pty_writer(SenderWithContext::new(tx));
    rx
}

fn drain_pty_writer(
    rx: &Receiver<(PtyWriteInstruction, yinz_types::diagnostics::ErrorContext)>,
) -> Vec<PtyWriteInstruction> {
    let mut out = Vec::new();
    while let Ok((msg, _ctx)) = rx.try_recv() {
        out.push(msg);
    }
    out
}

mod clients;
mod closing;
mod floating_fullscreen;
mod focus_guards;
mod focus_movement;
mod fullscreen;
mod fullscreen_interaction;
mod layout_rejection;
mod pane_id_api;
mod renaming;
mod resize_down;
mod resize_left;
mod resize_right;
mod resize_up;
mod scroll_forwarding;
mod splitting;
mod stack_hardening;
mod stack_resize;
mod text_selection;
mod viewport_clamp;

mod agent_double_escape;
