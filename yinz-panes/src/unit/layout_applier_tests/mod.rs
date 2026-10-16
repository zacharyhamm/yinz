use crate::layout_applier::LayoutApplier;
use crate::os_input_output::AsyncReader;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::panes::{FloatingPanes, TiledPanes};
use crate::panes::{LinkHandler, PaneId};
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::{os_input_output::ServerOsApi, thread_bus::ThreadSenders, ClientId};
use insta::assert_snapshot;
use interprocess::local_socket::Stream as LocalSocketStream;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::PathBuf;
use std::rc::Rc;
use yinz_runtime::{
    channels::{self, ChannelWithContext, Receiver, SenderWithContext},
    errors::prelude::*,
};
use yinz_types::input::options::PaneFrameStyle;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TerminalId;
use yinz_types::multiplayer::AttachedClients;
use yinz_types::pane_size::{Size, SizeInPixels, Viewport};
use yinz_types::style::{Palette, Style};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::ServerToClientMsg, yinz_types::data::ModeInfo,
    yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction,
    yinz_types::input::layout::FloatingPaneLayout, yinz_types::input::layout::Run,
    yinz_types::input::layout::TiledPaneLayout,
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

/// Parse KDL layout string and extract tiled and floating layouts
fn parse_kdl_layout(kdl_str: &str) -> (TiledPaneLayout, Vec<FloatingPaneLayout>) {
    let layout =
        yinz_config::kdl::layout::from_kdl(kdl_str, Some("test_layout".into()), None, None)
            .expect("Failed to parse KDL layout");
    layout.new_tab()
}

/// Creates all the fixtures needed for LayoutApplier tests
#[allow(clippy::type_complexity)]
fn create_layout_applier_fixtures(
    size: Size,
) -> (
    Rc<RefCell<Viewport>>,
    ThreadSenders,
    Rc<RefCell<SixelImageStore>>,
    Rc<RefCell<LinkHandler>>,
    Rc<RefCell<Palette>>,
    Rc<RefCell<HashMap<usize, String>>>,
    Rc<RefCell<Option<SizeInPixels>>>,
    Rc<RefCell<AttachedClients>>,
    Style,
    Rc<RefCell<Size>>,
    TiledPanes,
    FloatingPanes,
    PaneFrameStyle,
    Option<PaneId>,
    Box<dyn ServerOsApi>,
    bool,
    bool,
    bool,
    bool,
    bool,
) {
    let viewport = Rc::new(RefCell::new(Viewport {
        x: 0,
        y: 0,
        rows: size.rows,
        cols: size.cols,
    }));

    let (mock_plugin_sender, _mock_plugin_receiver) = channels::unbounded();
    let mut senders = ThreadSenders::default().silently_fail_on_send();
    senders.replace_to_plugin(SenderWithContext::new(mock_plugin_sender));
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(None));

    let client_id = ClientId::new(1);
    let mut connected_clients_map = AttachedClients::default();
    connected_clients_map.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients_map));

    let style = Style::default();
    let display_area = Rc::new(RefCell::new(size));

    let os_api = Box::new(FakeInputOutput {});

    // Create TiledPanes
    let connected_clients_set = Rc::new(RefCell::new(HashSet::from([client_id])));
    let mode_info = Rc::new(RefCell::new(HashMap::new()));
    let stacked_resize = Rc::new(RefCell::new(false));
    let reserved_top_rows = Rc::new(RefCell::new(HashMap::new()));
    let fullscreen_covers_ui = Rc::new(RefCell::new(false));
    let session_is_mirrored = true;
    let draw_pane_frames = PaneFrameStyle::Full;
    let default_mode_info = ModeInfo::default();

    let tiled_panes = TiledPanes::new(
        display_area.clone(),
        viewport.clone(),
        connected_clients_set.clone(),
        connected_clients.clone(),
        mode_info.clone(),
        character_cell_size.clone(),
        stacked_resize,
        reserved_top_rows,
        fullscreen_covers_ui.clone(),
        session_is_mirrored,
        draw_pane_frames,
        default_mode_info.clone(),
        style,
        os_api.box_clone(),
        senders.clone(),
    );

    // Create FloatingPanes
    let floating_panes = FloatingPanes::new(
        display_area.clone(),
        viewport.clone(),
        connected_clients_set,
        connected_clients.clone(),
        mode_info,
        character_cell_size.clone(),
        fullscreen_covers_ui,
        draw_pane_frames,
        session_is_mirrored,
        default_mode_info,
        style,
        os_api.box_clone(),
        senders.clone(),
    );

    let focus_pane_id = None;
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;

    (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        tiled_panes,
        floating_panes,
        draw_pane_frames,
        focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    )
}

/// Creates fixtures with receivers for verifying messages sent to pty and plugin threads
#[allow(clippy::type_complexity)]
fn create_layout_applier_fixtures_with_receivers(
    size: Size,
) -> (
    Rc<RefCell<Viewport>>,
    ThreadSenders,
    Rc<RefCell<SixelImageStore>>,
    Rc<RefCell<LinkHandler>>,
    Rc<RefCell<Palette>>,
    Rc<RefCell<HashMap<usize, String>>>,
    Rc<RefCell<Option<SizeInPixels>>>,
    Rc<RefCell<AttachedClients>>,
    Style,
    Rc<RefCell<Size>>,
    TiledPanes,
    FloatingPanes,
    PaneFrameStyle,
    Option<PaneId>,
    Box<dyn ServerOsApi>,
    bool,
    bool,
    bool,
    bool,
    bool,
    Receiver<(PtyInstruction, yinz_types::diagnostics::ErrorContext)>,
    Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
) {
    let viewport = Rc::new(RefCell::new(Viewport {
        x: 0,
        y: 0,
        rows: size.rows,
        cols: size.cols,
    }));

    let (mock_pty_sender, mock_pty_receiver): ChannelWithContext<PtyInstruction> =
        channels::unbounded();
    let (mock_plugin_sender, mock_plugin_receiver): ChannelWithContext<PluginInstruction> =
        channels::unbounded();

    let mut senders = ThreadSenders::default().silently_fail_on_send();
    senders.replace_to_pty(SenderWithContext::new(mock_pty_sender));
    senders.replace_to_plugin(SenderWithContext::new(mock_plugin_sender));

    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(None));

    let client_id = ClientId::new(1);
    let mut connected_clients_map = AttachedClients::default();
    connected_clients_map.insert(client_id);
    let connected_clients = Rc::new(RefCell::new(connected_clients_map));

    let style = Style::default();
    let display_area = Rc::new(RefCell::new(size));

    let os_api = Box::new(FakeInputOutput {});

    // Create TiledPanes
    let connected_clients_set = Rc::new(RefCell::new(HashSet::from([client_id])));
    let mode_info = Rc::new(RefCell::new(HashMap::new()));
    let stacked_resize = Rc::new(RefCell::new(false));
    let reserved_top_rows = Rc::new(RefCell::new(HashMap::new()));
    let fullscreen_covers_ui = Rc::new(RefCell::new(false));
    let session_is_mirrored = true;
    let draw_pane_frames = PaneFrameStyle::Full;
    let default_mode_info = ModeInfo::default();

    let tiled_panes = TiledPanes::new(
        display_area.clone(),
        viewport.clone(),
        connected_clients_set.clone(),
        connected_clients.clone(),
        mode_info.clone(),
        character_cell_size.clone(),
        stacked_resize,
        reserved_top_rows,
        fullscreen_covers_ui.clone(),
        session_is_mirrored,
        draw_pane_frames,
        default_mode_info.clone(),
        style,
        os_api.box_clone(),
        senders.clone(),
    );

    // Create FloatingPanes
    let floating_panes = FloatingPanes::new(
        display_area.clone(),
        viewport.clone(),
        connected_clients_set,
        connected_clients.clone(),
        mode_info,
        character_cell_size.clone(),
        fullscreen_covers_ui,
        draw_pane_frames,
        session_is_mirrored,
        default_mode_info,
        style,
        os_api.box_clone(),
        senders.clone(),
    );

    let focus_pane_id = None;
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;

    (
        viewport,
        senders,
        sixel_image_store,
        link_handler,
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        character_cell_size,
        connected_clients,
        style,
        display_area,
        tiled_panes,
        floating_panes,
        draw_pane_frames,
        focus_pane_id,
        os_api,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        mock_pty_receiver,
        mock_plugin_receiver,
    )
}

/// Takes a snapshot of the current pane state for assertion
fn take_pane_state_snapshot(
    tiled_panes: &TiledPanes,
    floating_panes: &FloatingPanes,
    focus_pane_id: &Option<PaneId>,
    viewport: &Rc<RefCell<Viewport>>,
    display_area: &Rc<RefCell<Size>>,
) -> String {
    let mut output = String::new();

    // Viewport info
    let viewport_state = viewport.borrow();
    writeln!(
        &mut output,
        "VIEWPORT: x={}, y={}, cols={}, rows={}",
        viewport_state.x, viewport_state.y, viewport_state.cols, viewport_state.rows
    )
    .unwrap();

    let display_state = display_area.borrow();
    writeln!(
        &mut output,
        "DISPLAY: cols={}, rows={}",
        display_state.cols, display_state.rows
    )
    .unwrap();

    // Focus state
    writeln!(&mut output, "FOCUS: {:?}", focus_pane_id).unwrap();

    writeln!(&mut output).unwrap();

    // Tiled panes
    writeln!(&mut output, "TILED PANES ({})", tiled_panes.panes.len()).unwrap();
    let mut tiled_list: Vec<_> = tiled_panes.get_panes().collect();
    tiled_list.sort_by_key(|(id, _)| **id);

    for (pane_id, pane) in tiled_list {
        let geom = pane.position_and_size();
        let run = pane.invoked_with();
        let selectable = pane.selectable();

        writeln!(&mut output, "  {:?}:", pane_id).unwrap();
        writeln!(
            &mut output,
            "    geom: x={}, y={}, cols={}, rows={}",
            geom.x,
            geom.y,
            geom.cols.as_usize(),
            geom.rows.as_usize()
        )
        .unwrap();

        if let Some(logical_pos) = geom.logical_position {
            writeln!(&mut output, "    logical_position: {}", logical_pos).unwrap();
        }

        if let Some(stack_id) = geom.stacked {
            writeln!(&mut output, "    stacked: {}", stack_id).unwrap();
        }

        writeln!(&mut output, "    run: {}", format_run_instruction(run)).unwrap();

        writeln!(&mut output, "    selectable: {}", selectable).unwrap();
        writeln!(&mut output, "    title: {}", pane.current_title()).unwrap();
        writeln!(&mut output, "    borderless: {}", pane.borderless()).unwrap();

        writeln!(&mut output).unwrap();
    }

    // Floating panes
    if floating_panes.pane_ids().count() > 0 {
        writeln!(
            &mut output,
            "FLOATING PANES ({})",
            floating_panes.pane_ids().count()
        )
        .unwrap();
        let mut floating_list: Vec<_> = floating_panes.get_panes().collect();
        floating_list.sort_by_key(|(id, _)| **id);

        for (pane_id, pane) in floating_list {
            let geom = pane.position_and_size();
            let run = pane.invoked_with();

            writeln!(&mut output, "  {:?}:", pane_id).unwrap();
            writeln!(
                &mut output,
                "    geom: x={}, y={}, cols={}, rows={}",
                geom.x,
                geom.y,
                geom.cols.as_usize(),
                geom.rows.as_usize()
            )
            .unwrap();

            if let Some(logical_pos) = geom.logical_position {
                writeln!(&mut output, "    logical_position: {}", logical_pos).unwrap();
            }

            writeln!(&mut output, "    run: {}", format_run_instruction(run)).unwrap();
            writeln!(&mut output, "    pinned: {}", geom.is_pinned).unwrap();
            writeln!(&mut output, "    selectable: {}", pane.selectable()).unwrap();
            writeln!(&mut output, "    title: {}", pane.current_title()).unwrap();
            writeln!(&mut output, "    borderless: {}", pane.borderless()).unwrap();
            writeln!(&mut output).unwrap();
        }
    }

    output
}

/// Format a Run instruction as a human-readable string
fn format_run_instruction(run: &Option<Run>) -> String {
    match run {
        None => "None".to_string(),
        Some(Run::Command(cmd)) => {
            let mut s = format!("Command({})", cmd.command.display());
            if !cmd.args.is_empty() {
                s.push_str(&format!(" args={:?}", cmd.args));
            }
            if let Some(cwd) = &cmd.cwd {
                s.push_str(&format!(" cwd={:?}", cwd));
            }
            s
        },
        Some(Run::Plugin(plugin)) => {
            format!("Plugin({})", plugin.location_string())
        },
        Some(Run::Cwd(path)) => format!("Cwd({:?})", path),
        Some(Run::EditFile(path, line, cwd)) => {
            format!("EditFile({:?}, line={:?}, cwd={:?})", path, line, cwd)
        },
    }
}

/// Collect all close pane messages from the pty receiver
fn collect_close_pane_messages(
    pty_receiver: &Receiver<(PtyInstruction, yinz_types::diagnostics::ErrorContext)>,
) -> Vec<PaneId> {
    let mut closed_panes = Vec::new();
    while let Ok((instruction, _)) = pty_receiver.try_recv() {
        if let PtyInstruction::ClosePane(pane_id, _) = instruction {
            closed_panes.push(pane_id);
        }
    }
    closed_panes
}

/// Collect all unload plugin messages from the plugin receiver
fn collect_unload_plugin_messages(
    plugin_receiver: &Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
) -> Vec<yinz_types::internal_ids::PluginId> {
    let mut unloaded_plugins = Vec::new();
    while let Ok((instruction, _)) = plugin_receiver.try_recv() {
        if let PluginInstruction::Unload(plugin_id) = instruction {
            unloaded_plugins.push(plugin_id);
        }
    }
    unloaded_plugins
}

mod apply;
mod override_floating;
mod override_retain;
mod override_tiled;
mod reapply;
mod rejection;

mod viewport;
