use super::{Output, Tab};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::screen::ScreenInstruction;
use crate::tab::CopyOptions;
use crate::{
    os_input_output::ServerOsApi, pane_groups::PaneGroups, panes::PaneId,
    plugins::PluginInstruction, thread_bus::ThreadSenders, ClientId,
};
use std::sync::Arc;
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::TerminalId;
use yinz_types::multiplayer::AttachedClients;

use std::path::PathBuf;
use std::sync::Mutex;
use yinz_runtime::channels::Receiver;
use yinz_runtime::envs::set_session_name;
use yinz_runtime::errors::prelude::*;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::geometry::Direction;
use yinz_types::geometry::PercentOrFixed;
use yinz_types::geometry::Resize;
use yinz_types::geometry::ResizeStrategy;
use {
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::RunPluginOrAlias,
    yinz_types::input::layout::SwapFloatingLayout, yinz_types::input::layout::SwapTiledLayout,
    yinz_types::input::layout::TiledPaneLayout,
};

use crate::os_input_output::AsyncReader;
use crate::pty_writer::PtyWriteInstruction;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use yinz_ipc::ipc::IpcReceiverWithContext;
use yinz_runtime::channels::{self, ChannelWithContext, SenderWithContext};
use yinz_types::geometry::FloatingPaneCoordinates;
use yinz_types::mouse::MouseEvent;
use yinz_types::pane_size::{Size, SizeInPixels};
use yinz_types::position::Position;
use yinz_types::style::{Palette, Style};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ServerToClientMsg,
    yinz_types::data::InputMode, yinz_types::data::ModeInfo, yinz_types::data::NewPanePlacement,
    yinz_types::data::OverlayPlacement, yinz_types::input::command::RunCommand,
    yinz_types::input::command::TerminalAction,
};

#[derive(Clone, Default)]
struct FakeInputOutput {
    file_dumps: Arc<Mutex<HashMap<String, String>>>,
    pub tty_stdin_bytes: Arc<Mutex<BTreeMap<TerminalId, Vec<u8>>>>,
    remote_panes: Arc<Mutex<HashSet<TerminalId>>>,
}

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
        _quit_db: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        _default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        unimplemented!()
    }
    fn write_to_tty_stdin(&self, id: TerminalId, buf: &[u8]) -> Result<usize> {
        self.tty_stdin_bytes
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn tcdrain(&self, _id: TerminalId) -> Result<()> {
        unimplemented!()
    }
    fn is_remote_pane(&self, terminal_id: TerminalId) -> bool {
        self.remote_panes.lock().unwrap().contains(&terminal_id)
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
    fn write_to_file(&mut self, buf: String, name: Option<String>) -> Result<()> {
        let f: String = match name {
            Some(x) => x,
            None => "tmp-name".to_owned(),
        };
        self.file_dumps.lock().to_anyhow()?.insert(f, buf);
        Ok(())
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

struct MockPtyInstructionBus {
    output: Arc<Mutex<Vec<String>>>,
    pty_writer_sender: SenderWithContext<PtyWriteInstruction>,
    pty_writer_receiver: Arc<Receiver<(PtyWriteInstruction, ErrorContext)>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockPtyInstructionBus {
    fn new() -> Self {
        let output = Arc::new(Mutex::new(vec![]));
        let (pty_writer_sender, pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
            channels::unbounded();
        let pty_writer_sender = SenderWithContext::new(pty_writer_sender);
        let pty_writer_receiver = Arc::new(pty_writer_receiver);

        Self {
            output,
            pty_writer_sender,
            pty_writer_receiver,
            handle: None,
        }
    }

    fn start(&mut self) {
        let output = self.output.clone();
        let pty_writer_receiver = self.pty_writer_receiver.clone();
        let handle = std::thread::Builder::new()
            .name("pty_writer".to_string())
            .spawn({
                move || loop {
                    let (event, _err_ctx) = pty_writer_receiver
                        .recv()
                        .expect("failed to receive event on channel");
                    match event {
                        PtyWriteInstruction::Write(msg, _, _) => output
                            .lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&msg).to_string()),
                        PtyWriteInstruction::Exit => break,
                        _ => {},
                    }
                }
            })
            .unwrap();
        self.handle = Some(handle);
    }

    fn exit(&mut self) {
        self.pty_writer_sender
            .send(PtyWriteInstruction::Exit)
            .unwrap();
        let handle = self.handle.take().unwrap();
        handle.join().unwrap();
    }

    fn pty_write_sender(&self) -> SenderWithContext<PtyWriteInstruction> {
        self.pty_writer_sender.clone()
    }

    fn clone_output(&self) -> Vec<String> {
        self.output.lock().unwrap().clone()
    }
}

fn create_new_tab(size: Size, default_mode: ModeInfo) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        (vec![], vec![]),
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_group,
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

/// A base tiled layout and the terminal ids (with their commands) it is applied with.
type BaseLayoutAndIds = (TiledPaneLayout, Vec<(TerminalId, Option<RunCommand>)>);

fn create_new_tab_with_stacked_pane_list(
    size: Size,
    default_mode: ModeInfo,
    stacked_pane_list: bool,
    base_layout_and_ids: Option<BaseLayoutAndIds>,
) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let stacked_pane_list = Rc::new(RefCell::new(stacked_pane_list));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        stacked_pane_list,
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
        (vec![], vec![]),
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_group,
        currently_marking_pane_group,
        advanced_mouse_actions,
        mouse_scroll_resize,
        true,
        false,
        false,
        false, // context_menu_on_right_click
        true,  // context_menu_override_agent_mouse
    );
    let (base_layout, new_terminal_ids) = base_layout_and_ids.unwrap_or_else(|| {
        (
            TiledPaneLayout::default(),
            vec![(crate::TerminalId::new(1), None)],
        )
    });
    tab.apply_layout(
        base_layout,
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

fn create_new_tab_without_pane_frames(size: Size, default_mode: ModeInfo) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::None;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        (vec![], vec![]),
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_group,
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

/// A base tiled and floating layout, the tiled and floating terminal ids (with their commands)
/// and the plugin ids they are applied with.
type BaseLayoutsAndIds = (
    TiledPaneLayout,
    Vec<FloatingPaneLayout>,
    Vec<(TerminalId, Option<RunCommand>)>,
    Vec<(TerminalId, Option<RunCommand>)>,
    HashMap<RunPluginOrAlias, Vec<yinz_types::internal_ids::PluginId>>,
);

fn create_new_tab_with_swap_layouts(
    size: Size,
    default_mode: ModeInfo,
    swap_layouts: (Vec<SwapTiledLayout>, Vec<SwapFloatingLayout>),
    base_layout_and_ids: Option<BaseLayoutsAndIds>,
    draw_pane_frames: bool,
    stacked_resize: bool,
) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
    let mut senders = ThreadSenders::default().silently_fail_on_send();
    let (mock_plugin_sender, _mock_plugin_receiver): ChannelWithContext<PluginInstruction> =
        channels::unbounded();
    senders.replace_to_plugin(SenderWithContext::new(mock_plugin_sender));
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(stacked_resize));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        if draw_pane_frames {
            PaneFrameStyle::Full
        } else {
            PaneFrameStyle::None
        },
        auto_layout,
        connected_clients,
        session_is_mirrored,
        Some(client_id),
        copy_options,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        swap_layouts,
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_group,
        currently_marking_pane_group,
        advanced_mouse_actions,
        mouse_scroll_resize,
        true,  // mouse_hover_effects
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        true,  // context_menu_override_agent_mouse
    );
    let (
        base_layout,
        base_floating_layout,
        new_terminal_ids,
        new_floating_terminal_ids,
        new_plugin_ids,
    ) = base_layout_and_ids.unwrap_or_default();
    let new_terminal_ids = if new_terminal_ids.is_empty() {
        vec![(TerminalId::new(1), None)]
    } else {
        new_terminal_ids
    };
    tab.apply_layout(
        base_layout,
        base_floating_layout,
        new_terminal_ids,
        new_floating_terminal_ids,
        new_plugin_ids,
        client_id,
        None,
    )
    .unwrap();
    tab
}

fn create_new_tab_with_os_api(size: Size, default_mode: ModeInfo, os_api: &FakeInputOutput) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(os_api.clone());
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        current_group,
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

fn create_new_tab_with_layout(size: Size, default_mode: ModeInfo, layout: &str) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
    let senders = ThreadSenders::default().silently_fail_on_send();
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let layout =
        yinz_config::layout::from_str(layout, "layout_file_name".into(), None, None).unwrap();
    let (tab_layout, floating_panes_layout) = layout.new_tab();
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        current_group,
        currently_marking_pane_group,
        advanced_mouse_actions,
        mouse_scroll_resize,
        true,  // mouse_hover_effects
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        true,  // context_menu_override_agent_mouse
    );
    let pane_ids = tab_layout
        .extract_run_instructions()
        .iter()
        .enumerate()
        .map(|(i, _)| (TerminalId::new(i as u32), None))
        .collect();
    let floating_pane_ids = floating_panes_layout
        .iter()
        .enumerate()
        .map(|(i, _)| (TerminalId::new(i as u32), None))
        .collect();
    tab.apply_layout(
        tab_layout,
        floating_panes_layout,
        pane_ids,
        floating_pane_ids,
        HashMap::new(),
        client_id,
        None,
    )
    .unwrap();
    tab
}

fn create_new_tab_with_mock_pty_writer(
    size: Size,
    default_mode: ModeInfo,
    mock_pty_writer: SenderWithContext<PtyWriteInstruction>,
) -> Tab {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
    let mut senders = ThreadSenders::default().silently_fail_on_send();
    senders.replace_to_pty_writer(mock_pty_writer);
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        current_group,
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

fn create_new_tab_with_sixel_support(
    size: Size,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
) -> Tab {
    // this is like the create_new_tab function but includes stuff needed for sixel,
    // eg. character_cell_size
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());
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
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        current_group,
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

fn read_fixture(fixture_name: &str) -> Vec<u8> {
    let mut path_to_file = std::path::PathBuf::new();
    path_to_file.push("../src");
    path_to_file.push("tests");
    path_to_file.push("fixtures");
    path_to_file.push(fixture_name);
    std::fs::read(path_to_file)
        .unwrap_or_else(|_| panic!("could not read fixture {:?}", fixture_name))
}

use crate::panes::grid::Grid;
use crate::panes::link_handler::LinkHandler;
use insta::assert_snapshot;
use vte;

fn take_snapshot(ansi_instructions: &str, rows: usize, columns: usize, palette: Palette) -> String {
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        rows,
        columns,
        Rc::new(RefCell::new(palette)),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, ansi_instructions.as_bytes());
    format!("{:?}", grid)
}

fn take_snapshot_with_sixel(
    ansi_instructions: &str,
    rows: usize,
    columns: usize,
    palette: Palette,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
) -> String {
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        rows,
        columns,
        Rc::new(RefCell::new(palette)),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, ansi_instructions.as_bytes());
    format!("{:?}", grid)
}

fn take_snapshot_and_cursor_position(
    ansi_instructions: &str,
    rows: usize,
    columns: usize,
    palette: Palette,
) -> (String, Option<(usize, usize)>) {
    // snapshot, x_coordinates, y_coordinates
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        rows,
        columns,
        Rc::new(RefCell::new(palette)),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, ansi_instructions.as_bytes());
    let coords = grid
        .cursor_coordinates()
        .and_then(|(x, y, visible)| if visible { Some((x, y)) } else { None });
    (format!("{:?}", grid), coords)
}

fn setup_tab_with_focused_floating_pane(
    size: Size,
    client_id: ClientId,
    frame_style: PaneFrameStyle,
) -> Tab {
    let mut tab = create_new_tab(size, ModeInfo::default());
    tab.set_pane_frames(frame_style);
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId::new(2),
        Vec::from("\n\n\n                   I am a floating pane".as_bytes()),
    )
    .unwrap();
    tab
}

fn two_adjacent_stacks_layout() -> (TiledPaneLayout, Vec<(TerminalId, Option<RunCommand>)>) {
    let base_layout = r#"
        layout {
            pane split_direction="horizontal" {
                pane stacked=true {
                    pane focus=true
                    pane
                    pane
                }
                pane stacked=true {
                    pane
                    pane
                    pane
                }
            }
        }
    "#;
    let (base_layout, _base_floating_layout) =
        yinz_config::kdl::layout::from_kdl(base_layout, Some("file_name.kdl".into()), None, None)
            .unwrap()
            .template
            .unwrap();
    let new_terminal_ids = vec![
        (TerminalId::new(1), None),
        (TerminalId::new(2), None),
        (TerminalId::new(3), None),
        (TerminalId::new(4), None),
        (TerminalId::new(5), None),
        (TerminalId::new(6), None),
    ];
    (base_layout, new_terminal_ids)
}

fn stack_member_pane_count(tab: &Tab) -> usize {
    tab.get_all_pane_ids()
        .into_iter()
        .filter(|id| tab.pane_is_stack_list_member(id))
        .count()
}

// =====================================================================
// Plugin Highlight Mouse Integration Tests
// =====================================================================

use {
    yinz_types::data::HighlightLayer, yinz_types::data::HighlightStyle,
    yinz_types::data::RegexHighlight,
};

fn create_new_tab_with_plugin_receiver(
    size: Size,
    default_mode: ModeInfo,
) -> (Tab, Receiver<(PluginInstruction, ErrorContext)>) {
    create_new_tab_with_plugin_receiver_and_os(size, default_mode, Box::default())
}

fn create_new_tab_with_plugin_receiver_and_os(
    size: Size,
    default_mode: ModeInfo,
    os_api: Box<FakeInputOutput>,
) -> (Tab, Receiver<(PluginInstruction, ErrorContext)>) {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let mut senders = ThreadSenders::default().silently_fail_on_send();
    let (mock_plugin_sender, mock_plugin_receiver): ChannelWithContext<PluginInstruction> =
        channels::unbounded();
    senders.replace_to_plugin(SenderWithContext::new(mock_plugin_sender));
    let max_panes = None;
    let mode_info = default_mode;
    let style = Style::default();
    let draw_pane_frames = PaneFrameStyle::Full;
    let auto_layout = true;
    let client_id = ClientId::new(1);
    let session_is_mirrored = true;
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
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
        (vec![], vec![]),
        PathBuf::from("my_default_shell"),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
        current_group,
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
    (tab, mock_plugin_receiver)
}

// ========================================================================
// OSC 99 Desktop Notification Integration Tests
// ========================================================================

/// Creates a Tab with a real `to_server` sender so that `ServerInstruction::Render`
/// calls from `forward_desktop_notifications` can be captured on the receiver.
/// A tab with a real `to_screen` sender, so what it hands the screen (desktop notifications,
/// for one) can be read off the receiver.
fn create_new_tab_with_screen_receiver(
    size: Size,
    default_mode: ModeInfo,
) -> (Tab, Receiver<(ScreenInstruction, ErrorContext)>) {
    set_session_name("test".into());
    let index = 0;
    let position = 0;
    let name = String::new();
    let os_api = Box::new(FakeInputOutput::default());

    let (screen_sender, screen_receiver) = channels::unbounded();
    let screen_sender = SenderWithContext::new(screen_sender);
    let mut senders = ThreadSenders::default().silently_fail_on_send();
    senders.to_screen = Some(screen_sender);

    let max_panes = None;
    let client_id = ClientId::new(1);
    let mut connected_clients = AttachedClients::default();
    connected_clients.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients));
    let character_cell_info = Rc::new(RefCell::new(None));
    let stacked_resize = Rc::new(RefCell::new(true));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let copy_options = CopyOptions::default();
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let current_group = Rc::new(RefCell::new(PaneGroups::new(ThreadSenders::default())));
    let currently_marking_pane_group = Rc::new(RefCell::new(HashMap::new()));
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
        Style::default(),
        default_mode,
        PaneFrameStyle::Full,
        true, // auto_layout
        connected_clients,
        true, // session_is_mirrored
        Some(client_id),
        copy_options,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        (vec![], vec![]),
        PathBuf::from("my_default_shell"),
        false, // debug
        true,  // arrow_fonts
        true,  // styled_underlines
        true,  // osc8_hyperlinks
        false, // explicitly_disable_kitty_keyboard_protocol
        None,
        current_group,
        currently_marking_pane_group,
        true,  // advanced_mouse_actions
        true,  // mouse_scroll_resize
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
    (tab, screen_receiver)
}

/// Helper: render every OSC 99 notification the tab handed the screen the way an OSC 99 host
/// would receive it (identifier namespaced to the pane), concatenated.
fn collect_forwarded_osc99(receiver: &Receiver<(ScreenInstruction, ErrorContext)>) -> String {
    use crate::panes::grid::{namespace_notification_id, PendingNotification};
    let mut output = String::new();
    while let Ok((instruction, _)) = receiver.try_recv() {
        if let ScreenInstruction::ForwardDesktopNotifications {
            pane_id,
            notifications,
        } = instruction
        {
            for notification in notifications {
                if let PendingNotification::Osc99 {
                    payload,
                    terminator,
                    wants_report,
                    ..
                } = notification
                {
                    let (metadata, rest) = match payload.find(';') {
                        Some(idx) => (
                            payload.get(..idx).unwrap_or_default(),
                            payload.get(idx..).unwrap_or_default(),
                        ),
                        None => (payload.as_str(), ""),
                    };
                    let namespaced_metadata =
                        namespace_notification_id(metadata, pane_id, wants_report);
                    output.push_str(&format!(
                        "\u{1b}]99;{}{}{}",
                        namespaced_metadata, rest, terminator
                    ));
                }
            }
        }
    }
    output
}

// ---------------------------------------------------------------------------
// Suppressed panes
// ---------------------------------------------------------------------------
//
// `SuppressedPanes` is keyed by the suppressed pane's *own* id, and the pane
// holding its place in the grid — if any — is recorded in `restored_by` rather
// than in the key. That is what lets a pane be suppressed twice over (a
// stack-list member with a scrollback editor open on it, then clicked away from)
// without either half being lost, and it is what replaced an explicit re-homing
// loop whose whole job was to avoid dropping a pane on a key collision. What
// used to be defended by that loop is now meant to hold by construction, so it
// is worth asking of arbitrary sequences of the operations that put panes in and
// out of there.
//
// The model is deliberately thin: exactly one pane goes when one is closed, and
// nothing else ever leaves. Every other operation moves a pane between the grid
// and `suppressed_panes` — which is a move, not a disappearance, however many
// times it happens to the same pane.

struct SuppressedPaneModel {
    tab: Tab,
    /// Every pane created and not yet closed. The oracle for "no pane was lost".
    alive: std::collections::HashSet<PaneId>,
    next_id: u32,
    /// Held only so the plugin channel keeps its receiving end.
    _plugin_receiver: Receiver<(PluginInstruction, ErrorContext)>,
}

impl SuppressedPaneModel {
    const CLIENT_ID: ClientId = crate::ClientId::new(1);

    fn new() -> Self {
        let mut tab = create_new_tab_with_stacked_pane_list(
            Size {
                cols: 121,
                rows: 40,
            },
            ModeInfo::default(),
            true,
            None,
        );
        // Plugin panes are suppressed by the same map as terminal ones and are
        // the usual occupants of it, so they belong in the sequence - but making
        // one needs somewhere to send to.
        let (plugin_sender, plugin_receiver): ChannelWithContext<PluginInstruction> =
            channels::unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(plugin_sender));
        SuppressedPaneModel {
            tab,
            alive: std::collections::HashSet::from([PaneId::Terminal(crate::TerminalId::new(1))]),
            next_id: 2,
            _plugin_receiver: plugin_receiver,
        }
    }

    fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Panes that hold a place in the grid, which is what most operations need.
    fn on_screen(&self) -> Vec<PaneId> {
        let mut ids = self.tab.get_static_and_floating_pane_ids();
        ids.sort_unstable();
        ids
    }

    /// Panes in the grid proper. A stack is a shape the tiled layout takes, so
    /// these are the only panes there is any such thing as stacking under.
    fn tiled(&self) -> Vec<PaneId> {
        let mut ids: Vec<PaneId> = self.tab.get_tiled_panes().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids
    }

    fn suppressed(&self) -> Vec<PaneId> {
        let mut ids: Vec<PaneId> = self.tab.suppressed_panes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Record a just-created pane, if the tab actually took it.
    ///
    /// Creation is allowed to come to nothing: `add_tiled_pane` and
    /// `add_floating_pane` drop the box and report success when the tab has no
    /// room for another pane, which is a documented outcome for a pane being
    /// *created* (see `accept_extracted_pane`, which exists because it is not an
    /// acceptable outcome for a pane being moved). So what goes in the model is
    /// what the tab took, not what it was offered - the subject here is panes
    /// going missing afterwards, one suppression at a time.
    fn record_if_taken(&mut self, pane_id: PaneId) {
        if self.tab.has_pane_with_pid(&pane_id) {
            self.alive.insert(pane_id);
        }
    }

    /// Draw one of `choices`, or skip the rule when there is nothing to choose.
    ///
    /// Sorted by the caller so the choice is made from a stable list: the sets
    /// come out of a `HashMap`, and an index into an arbitrarily ordered list
    /// would mean a shrunk counterexample did not replay.
    fn pick(tc: &hegel::TestCase, choices: &[PaneId]) -> PaneId {
        tc.assume(!choices.is_empty());
        let index = tc.draw(
            hegel::generators::integers::<usize>()
                .min_value(0)
                .max_value(choices.len().saturating_sub(1)),
        );
        choices[index]
    }
}

#[hegel::state_machine]
impl SuppressedPaneModel {
    /// A new pane, placed however panes get placed. `Stacked` is the one that
    /// builds a stacked pane list, which parks its hidden members.
    #[rule]
    fn open_a_pane(&mut self, tc: hegel::TestCase) {
        let pane_id = if tc.draw(hegel::generators::booleans()) {
            PaneId::Terminal(crate::TerminalId::new(self.fresh_id()))
        } else {
            PaneId::Plugin(crate::PluginId::new(self.fresh_id()))
        };
        let placement = match tc.draw(
            hegel::generators::integers::<usize>()
                .min_value(0)
                .max_value(2),
        ) {
            0 => NewPanePlacement::default(),
            1 => NewPanePlacement::Floating(None),
            // Named, and named a tiled pane. A stack is a shape the tiled layout
            // takes, so there is no such thing as stacking under a floating pane
            // or under nothing at all - and `Tab` parks the pane in both cases
            // rather than refusing it, which is worth knowing but is not what
            // this is asking about.
            _ => NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(Self::pick(&tc, &self.tiled())),
                borderless: None,
            },
        };
        self.tab
            .new_pane(
                pane_id,
                None,
                None,
                false,
                true,
                placement,
                Some(Self::CLIENT_ID),
                None,
            )
            .unwrap();
        self.record_if_taken(pane_id);
    }

    /// A pane replaced in place: the pane that was there is parked *behind* the
    /// one that replaced it - `restored_by: Some(_)` with `is_scrollback_editor`
    /// false - so closing the replacement swaps it back in. The other stand-in
    /// flavour, and the one whose close semantics this branch changed.
    #[rule]
    fn replace_a_pane_in_place(&mut self, tc: hegel::TestCase) {
        let replaced = Self::pick(&tc, &self.on_screen());
        let pane_id = PaneId::Terminal(crate::TerminalId::new(self.fresh_id()));
        self.tab
            .new_pane(
                pane_id,
                None,
                None,
                false,
                true,
                NewPanePlacement::InPlace {
                    pane_id_to_replace: Some(replaced),
                    close_replaced_pane: false,
                    borderless: None,
                },
                Some(Self::CLIENT_ID),
                None,
            )
            .unwrap();
        self.record_if_taken(pane_id);
    }

    /// A scrollback editor: a stand-in like the one above, and the only one
    /// flagged as such - `is_scrollback_editor` is what tells a pane whose
    /// replacement has its own reason to be on screen from a pane being read.
    #[rule]
    fn open_a_scrollback_editor(&mut self, tc: hegel::TestCase) {
        let on_screen: Vec<PaneId> = self
            .on_screen()
            .into_iter()
            .filter(|pane_id| matches!(pane_id, PaneId::Terminal(_)))
            .collect();
        let edited = Self::pick(&tc, &on_screen);
        let editor = PaneId::Terminal(crate::TerminalId::new(self.fresh_id()));
        self.tab
            .replace_pane_with_editor_pane(editor, edited)
            .unwrap();
        self.record_if_taken(editor);
    }

    /// What a plugin's `hide_self` does: park a pane with nothing standing in
    /// for it.
    #[rule]
    fn hide_a_pane(&mut self, tc: hegel::TestCase) {
        let pane_id = Self::pick(&tc, &self.on_screen());
        self.tab.suppress_pane(pane_id, Some(Self::CLIENT_ID));
    }

    /// `show_self`, and the only way a parked pane comes back on its own.
    #[rule]
    fn show_a_pane(&mut self, tc: hegel::TestCase) {
        let pane_id = Self::pick(&tc, &self.suppressed());
        let float_if_hidden = tc.draw(hegel::generators::booleans());
        self.tab.unsuppress_pane(pane_id, float_if_hidden);
    }

    /// Selecting a member of a stacked pane list, which swaps the visible member
    /// out and the chosen one in.
    #[rule]
    fn select_a_hidden_stack_list_member(&mut self, tc: hegel::TestCase) {
        let members: Vec<PaneId> = self
            .suppressed()
            .into_iter()
            .filter(|pane_id| self.tab.pane_is_hidden_stack_list_member(pane_id))
            .collect();
        let pane_id = Self::pick(&tc, &members);
        assert!(
            self.tab
                .focus_hidden_stack_list_member(pane_id, Self::CLIENT_ID),
            "a hidden member should be selectable",
        );
    }

    #[rule]
    fn toggle_a_pane_between_floating_and_embedded(&mut self, tc: hegel::TestCase) {
        let pane_id = Self::pick(&tc, &self.on_screen());
        let was_floating = self.tab.pane_id_is_floating(&pane_id);
        if self
            .tab
            .toggle_pane_embed_or_floating_for_pane_id(pane_id, Some(Self::CLIENT_ID))
            .is_err()
        {
            // The last tiled pane and an embedding without room must stay where they were.
            assert_eq!(self.tab.pane_id_is_floating(&pane_id), was_floating);
        }
    }

    /// Closing takes exactly the pane it was asked for, wherever that pane was.
    ///
    /// This is the rule the model exists for: closing a stand-in swaps the pane
    /// behind it back into the grid, closing a stack list's visible member
    /// promotes a hidden one, and closing a parked pane just drops it - three
    /// different amounts of bookkeeping, all of which have to end up one pane
    /// lighter and no more.
    #[rule]
    fn close_a_pane(&mut self, tc: hegel::TestCase) {
        let mut alive: Vec<PaneId> = self.alive.iter().copied().collect();
        alive.sort_unstable();
        let pane_id = Self::pick(&tc, &alive);
        // Asked before the close, because afterwards there is nothing left to ask
        // it of. Conservation alone would be satisfied by a pane that is parked
        // forever: still in the tab, still findable, never on screen again and
        // with nothing that could put it there. Closing a stand-in is the moment
        // that has to be true, and it is true for both flavours - a scrollback
        // editor and a pane replaced in place both go through
        // `replace_pane_with_suppressed_pane`.
        let restored = self.tab.stands_in_for_a_pane(pane_id);
        self.tab.close_pane(pane_id, false, None);
        self.alive.remove(&pane_id);
        if let Some(restored) = restored {
            assert!(
                self.on_screen().contains(&restored),
                "closing {:?} left {:?} parked with nothing to bring it back",
                pane_id,
                restored,
            );
        }
    }

    /// Every entry is filed under the id of the pane it holds.
    ///
    /// The invariant the rewrite is named for: before it, the key was the pane
    /// doing the suppressing, so the same pane could be filed under two
    /// different ids over its life and found by neither.
    #[invariant]
    fn a_suppressed_pane_is_filed_under_its_own_id(&self, _: hegel::TestCase) {
        for (pane_id, suppressed) in self.tab.get_suppressed_panes() {
            assert_eq!(
                suppressed.pane.pid(),
                *pane_id,
                "suppressed pane {:?} is filed under {:?}",
                suppressed.pane.pid(),
                pane_id,
            );
        }
    }

    /// No two panes are waiting on the same stand-in.
    ///
    /// Closing a stand-in swaps back whatever names it, so a second claimant
    /// would be a pane that can never be restored - it would sit in the map
    /// forever while its slot went to the other one.
    #[invariant]
    fn a_stand_in_holds_at_most_one_panes_place(&self, _: hegel::TestCase) {
        let mut stand_ins = std::collections::HashSet::new();
        for (pane_id, suppressed) in self.tab.get_suppressed_panes() {
            if let Some(stand_in) = suppressed.restored_by {
                assert!(
                    stand_ins.insert(stand_in.pane_id),
                    "{:?} and another pane are both waiting on {:?}",
                    pane_id,
                    stand_in.pane_id,
                );
            }
        }
    }

    /// A pane hidden behind a stand-in is never also a stack-list member.
    ///
    /// The stand-in takes over the membership - `track_stack_list_member_id_swap`
    /// runs at every `suppress_pane_behind` call site - and this is what makes
    /// "hidden, but a stack-list member" the whole of the exception a tab warp
    /// makes: a pane with a stand-in has no slot of its own in the layout the tab
    /// travels as, so a warp that took it would release a supervisor nobody at the
    /// far end is holding. Nothing in `check_tab_can_be_warped` can enforce it;
    /// it is enforced here or nowhere.
    #[invariant]
    fn a_pane_hidden_behind_a_stand_in_is_not_a_stack_list_member(&self, _: hegel::TestCase) {
        for (pane_id, suppressed) in self.tab.get_suppressed_panes() {
            if suppressed.restored_by.is_some() {
                assert!(
                    !self.tab.pane_is_stack_list_member(pane_id),
                    "{:?} is hidden behind a stand-in and is still a list member",
                    pane_id,
                );
            }
        }
    }

    /// A pane is either in the grid or suppressed, never both.
    ///
    /// Both at once means two `Box<dyn Pane>` answering to one id: the grid
    /// renders one while every lookup that falls through to `suppressed_panes`
    /// finds the other.
    #[invariant]
    fn a_pane_is_never_both_on_screen_and_suppressed(&self, _: hegel::TestCase) {
        for pane_id in self.tab.suppressed_panes.keys() {
            assert!(
                !self.tab.has_non_suppressed_pane_with_pid(pane_id),
                "{:?} is in the grid and suppressed at the same time",
                pane_id,
            );
        }
    }

    /// The tab holds exactly the panes that were opened and not closed.
    ///
    /// Losing one is the failure that has no symptom at the time: the pane stops
    /// being rendered and stops being findable, and its process goes on running
    /// with nothing left that could ever close it.
    #[invariant]
    fn every_pane_opened_and_not_closed_is_still_here(&self, _: hegel::TestCase) {
        let mut held = self.tab.get_all_pane_ids();
        held.sort_unstable();
        let mut deduped = held.clone();
        deduped.dedup();
        assert_eq!(held, deduped, "a pane is in the tab twice: {:?}", held);

        let mut expected: Vec<PaneId> = self.alive.iter().copied().collect();
        expected.sort_unstable();
        assert_eq!(held, expected);
    }
}

mod borderless_pinned;
mod centered_agents;
mod cursor;
mod floating;
mod focus_follows_mouse;
mod highlights;
mod host_focus;
mod layout;
mod mouse;
mod osc99;
mod pane_content;
mod pane_creation;
mod pane_tabs;
mod rendering;
mod stack_list_headers;
mod stacked_panes;
mod swap_layouts;
mod translucency;
mod visibility;
