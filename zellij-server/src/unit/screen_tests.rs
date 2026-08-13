use super::{screen_thread_main, CopyOptions, Screen, ScreenInstruction};
use crate::global_panes::GlobalPaneManager;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::{PaneFactory, PaneId};
use crate::route::NotificationEnd;
use crate::{
    channels::SenderWithContext, os_input_output::ServerOsApi, route::route_action,
    thread_bus::Bus, ClientId, ServerInstruction, SessionMetaData, ThreadSenders,
};
use insta::assert_snapshot;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use zellij_utils::cli::CliAction;
use zellij_utils::data::{Event, EventType, Mouse, Resize, Style, Styling, WebSharing};
use zellij_utils::errors::{prelude::*, ErrorContext};
use zellij_utils::input::actions::Action;
use zellij_utils::input::command::{RunCommand, RunCommandAction, TerminalAction};
use zellij_utils::input::config::Config;
use zellij_utils::input::keybinds::Keybinds;
use zellij_utils::input::layout::{
    FloatingPaneLayout, GlobalPaneLayout, Layout, PercentOrFixed, PluginAlias,
    PluginUserConfiguration, Run, RunPlugin, RunPluginLocation, RunPluginOrAlias, SplitDirection,
    TiledPaneLayout,
};
use zellij_utils::input::mouse::MouseEvent;
use zellij_utils::input::options::{
    NestedSessionHandling, NotificationPosition, Options, PaneFrameStyle,
    DEFAULT_NOTIFICATION_TIMEOUT_SECS, DEFAULT_WORD_SEPARATORS,
};
use zellij_utils::ipc::IpcReceiverWithContext;
use zellij_utils::pane_size::{Size, SizeInPixels};
use zellij_utils::position::Position;

use crate::background_jobs::BackgroundJob;
use crate::os_input_output::{AsyncReader, NullAsyncReader};
use crate::pty_writer::PtyWriteInstruction;
use std::collections::HashSet;
use std::env::set_var;
use std::sync::{Arc, Mutex};

use crate::{
    plugins::{PluginInstruction, PluginRenderAsset},
    pty::{pty_thread_main, ClientTabIndexOrPaneId, Pty, PtyInstruction},
    ui::loading_indication::LoadingIndication,
};
use zellij_utils::ipc::PixelDimensions;

use interprocess::local_socket::Stream as LocalSocketStream;
use zellij_utils::{
    channels::{self, ChannelWithContext, Receiver},
    data::{Direction, FloatingPaneCoordinates, InputMode, ModeInfo, NewPanePlacement, Palette},
    ipc::{ClientAttributes, ClientToServerMsg, ServerToClientMsg},
};

use crate::panes::grid::Grid;
use crate::panes::link_handler::LinkHandler;
use crate::panes::sixel::SixelImageStore;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use zellij_utils::data::{PaneContents, PaneRenderReport};
use zellij_utils::ipc::ExitReason;

fn take_snapshot_and_cursor_coordinates(
    ansi_instructions: &str,
    grid: &mut Grid,
) -> (Option<(usize, usize)>, String) {
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(grid, ansi_instructions.as_bytes());
    let coords = grid
        .cursor_coordinates()
        .and_then(|(x, y, visible)| if visible { Some((x, y)) } else { None });
    (coords, format!("{:?}", grid))
}

fn grid_for_render_snapshot(screen_size: Size) -> Grid {
    Grid::new(
        screen_size.rows,
        screen_size.cols,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            width: 8,
            height: 21,
        }))),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    )
}

fn take_snapshots_and_cursor_coordinates_from_render_events<'a>(
    all_events: impl Iterator<Item = &'a ServerInstruction>,
    screen_size: Size,
) -> Vec<(Option<(usize, usize)>, String)> {
    let mut grid = grid_for_render_snapshot(screen_size);
    let snapshots: Vec<(Option<(usize, usize)>, String)> = all_events
        .filter_map(|server_instruction| {
            match server_instruction {
                ServerInstruction::Render(output) => {
                    if let Some(output) = output {
                        // note this only takes a snapshot of the first client!
                        let raw_snapshot = output.get(&1).unwrap();
                        let snapshot =
                            take_snapshot_and_cursor_coordinates(raw_snapshot, &mut grid);
                        Some(snapshot)
                    } else {
                        None
                    }
                },
                _ => None,
            }
        })
        .collect();
    snapshots
}

fn send_cli_action_to_server(
    session_metadata: &SessionMetaData,
    cli_action: CliAction,
    client_id: ClientId,
) {
    let get_current_dir = || PathBuf::from(".");
    let actions = Action::actions_from_cli(cli_action, Box::new(get_current_dir), None).unwrap();
    let senders = session_metadata.senders.clone();
    let default_shell = None;
    let default_mode = session_metadata
        .session_configuration
        .get_client_configuration(&client_id)
        .options
        .default_mode
        .unwrap_or(InputMode::Normal);
    for action in actions {
        route_action(
            action,
            client_id,
            None,
            None,
            senders.clone(),
            default_shell.clone(),
            None,
            default_mode,
            None,
        )
        .unwrap();
    }
}

fn route_arbitrary_action_to_server(
    session_metadata: &SessionMetaData,
    action: Action,
    client_id: ClientId,
) {
    let senders = session_metadata.senders.clone();
    let default_mode = session_metadata
        .session_configuration
        .get_client_configuration(&client_id)
        .options
        .default_mode
        .unwrap_or(InputMode::Normal);
    route_action(
        action,
        client_id,
        None,
        None,
        senders,
        None,
        None,
        default_mode,
        None,
    )
    .unwrap();
}

#[derive(Clone, Default)]
struct FakeInputOutput {
    fake_filesystem: Arc<Mutex<HashMap<String, String>>>,
    server_to_client_messages: Arc<Mutex<HashMap<ClientId, Vec<ServerToClientMsg>>>>,
}

impl ServerOsApi for FakeInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        _terminal_id: u32,
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
    ) -> Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        Ok((42, Box::new(NullAsyncReader), None))
    }
    fn write_to_tty_stdin(&self, _id: u32, _buf: &[u8]) -> Result<usize> {
        unimplemented!()
    }
    fn tcdrain(&self, _id: u32) -> Result<()> {
        unimplemented!()
    }
    fn kill(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
    fn force_kill(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()> {
        self.server_to_client_messages
            .lock()
            .unwrap()
            .entry(client_id)
            .or_insert_with(Vec::new)
            .push(msg);
        Ok(())
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
    fn get_cwd(&self, _pid: u32) -> Option<PathBuf> {
        unimplemented!()
    }
    fn write_to_file(&mut self, contents: String, filename: Option<String>) -> Result<()> {
        if let Some(filename) = filename {
            self.fake_filesystem
                .lock()
                .unwrap()
                .insert(filename, contents);
        }
        Ok(())
    }
    fn re_run_command_in_terminal(
        &self,
        _terminal_id: u32,
        _run_command: RunCommand,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)> {
        unimplemented!()
    }
    fn clear_terminal_id(&self, _terminal_id: u32) -> Result<()> {
        unimplemented!()
    }
    fn send_sigint(&self, _pid: u32) -> Result<()> {
        unimplemented!()
    }
}

fn create_new_screen(
    size: Size,
    advanced_mouse_actions: bool,
    mouse_hover_effects: bool,
) -> Screen {
    create_new_screen_with_kitty_graphics(size, advanced_mouse_actions, mouse_hover_effects, true)
}

fn create_new_screen_with_kitty_graphics(
    size: Size,
    advanced_mouse_actions: bool,
    mouse_hover_effects: bool,
    support_kitty_graphics_protocol: bool,
) -> Screen {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mut mode_info = ModeInfo::default();
    mode_info.session_name = Some("zellij-test".into());
    let draw_pane_frames = PaneFrameStyle::None;
    let auto_layout = true;
    let session_is_mirrored = true;
    let copy_options = CopyOptions::default();
    let default_layout = Box::new(Layout::default());
    let default_layout_name = None;
    let default_shell = PathBuf::from("my_default_shell");
    let session_serialization = true;
    let serialize_pane_viewport = false;
    let scrollback_lines_to_serialize = None;
    let layout_dir = None;

    let debug = false;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let arrow_fonts = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let stacked_resize = true;
    let web_sharing = WebSharing::Off;
    let web_server_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    let web_server_port = 8080;
    let visual_bell = true;
    let mouse_scroll_resize = true;
    let screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        draw_pane_frames,
        auto_layout,
        session_is_mirrored,
        copy_options,
        debug,
        default_layout,
        default_layout_name,
        default_shell,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        styled_underlines,
        osc8_hyperlinks,
        arrow_fonts,
        layout_dir,
        explicitly_disable_kitty_keyboard_protocol,
        support_kitty_graphics_protocol,
        stacked_resize,
        false,
        None,
        false,
        web_sharing,
        advanced_mouse_actions,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        mouse_scroll_resize,
        mouse_hover_effects,
        visual_bell,
        false, // focus_follows_mouse
        false, // mouse_click_through
        web_server_ip,
        web_server_port,
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    seed_first_client_size(screen, size)
}

fn seed_first_client_size(mut screen: Screen, size: Size) -> Screen {
    screen.set_client_size(1, size);
    screen
}

struct MockScreen {
    pub main_client_id: u16,
    pub pty_receiver: Option<Receiver<(PtyInstruction, ErrorContext)>>,
    pub pty_writer_receiver: Option<Receiver<(PtyWriteInstruction, ErrorContext)>>,
    #[allow(dead_code)]
    pub background_jobs_receiver: Option<Receiver<(BackgroundJob, ErrorContext)>>,
    pub received_background_jobs: Arc<Mutex<Vec<BackgroundJob>>>,
    pub screen_receiver: Option<Receiver<(ScreenInstruction, ErrorContext)>>,
    pub server_receiver: Option<Receiver<(ServerInstruction, ErrorContext)>>,
    pub plugin_receiver: Option<Receiver<(PluginInstruction, ErrorContext)>>,
    pub to_screen: SenderWithContext<ScreenInstruction>,
    pub to_pty: SenderWithContext<PtyInstruction>,
    pub to_plugin: SenderWithContext<PluginInstruction>,
    pub to_server: SenderWithContext<ServerInstruction>,
    pub to_pty_writer: SenderWithContext<PtyWriteInstruction>,
    pub to_background_jobs: SenderWithContext<BackgroundJob>,
    pub os_input: FakeInputOutput,
    pub client_attributes: ClientAttributes,
    #[allow(dead_code)]
    pub config_options: Options,
    pub session_metadata: SessionMetaData,
    pub config: Config,
    default_layout: Box<Layout>,
    advanced_mouse_actions: bool,
    last_opened_tab_index: Option<usize>,
}

impl MockScreen {
    pub fn run(
        &mut self,
        initial_layout: Option<TiledPaneLayout>,
        initial_floating_panes_layout: Vec<FloatingPaneLayout>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
        let mut config = self.config.clone();
        config.options.advanced_mouse_actions = Some(self.advanced_mouse_actions);
        let client_attributes = self.client_attributes.clone();
        let default_layout = self.default_layout.clone();
        let screen_bus = Bus::new(
            vec![self.screen_receiver.take().unwrap()],
            Some(&self.to_screen.clone()),
            Some(&self.to_pty.clone()),
            Some(&self.to_plugin.clone()),
            Some(&self.to_server.clone()),
            Some(&self.to_pty_writer.clone()),
            Some(&self.to_background_jobs.clone()),
            Some(Box::new(self.os_input.clone())),
        )
        .should_silently_fail();
        let debug = false;
        let screen_thread = std::thread::Builder::new()
            .name("screen_thread".to_string())
            .spawn(move || {
                set_var("ZELLIJ_SESSION_NAME", "zellij-test");
                screen_thread_main(
                    screen_bus,
                    None,
                    client_attributes,
                    config,
                    debug,
                    default_layout,
                )
                .expect("TEST")
            })
            .unwrap();
        let pane_layout = initial_layout.unwrap_or_default();
        let pane_count = pane_layout.extract_run_instructions().len();
        let floating_pane_count = initial_floating_panes_layout.len();
        let mut pane_ids = vec![];
        let mut floating_pane_ids = vec![];
        let mut plugin_ids = HashMap::new();
        plugin_ids.insert(
            RunPluginOrAlias::from_url("file:/path/to/fake/plugin", &None, None, None).unwrap(),
            vec![1],
        );
        for i in 0..pane_count {
            pane_ids.push((i as u32, None));
        }
        for i in 0..floating_pane_count {
            floating_pane_ids.push((i as u32, None));
        }
        let default_shell = None;
        let tab_name = None;
        let tab_index = self.last_opened_tab_index.map(|l| l + 1).unwrap_or(0);
        let should_change_focus_to_new_tab = true;
        std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async
                                                                   // render
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(pane_layout.clone()),
            initial_floating_panes_layout.clone(),
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            (self.main_client_id, false),
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            pane_layout,
            initial_floating_panes_layout,
            pane_ids,
            floating_pane_ids,
            plugin_ids,
            tab_index,
            true,
            (self.main_client_id, false),
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
        std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
        screen_thread
    }
    // same as the above function, but starts a plugin with a plugin alias
    pub fn run_with_alias(
        &mut self,
        initial_layout: Option<TiledPaneLayout>,
        initial_floating_panes_layout: Vec<FloatingPaneLayout>,
    ) -> std::thread::JoinHandle<()> {
        let config = self.config.clone();
        let client_attributes = self.client_attributes.clone();
        let screen_bus = Bus::new(
            vec![self.screen_receiver.take().unwrap()],
            Some(&self.to_screen.clone()),
            Some(&self.to_pty.clone()),
            Some(&self.to_plugin.clone()),
            Some(&self.to_server.clone()),
            Some(&self.to_pty_writer.clone()),
            Some(&self.to_background_jobs.clone()),
            Some(Box::new(self.os_input.clone())),
        )
        .should_silently_fail();
        let debug = false;
        let screen_thread = std::thread::Builder::new()
            .name("screen_thread".to_string())
            .spawn(move || {
                set_var("ZELLIJ_SESSION_NAME", "zellij-test");
                screen_thread_main(
                    screen_bus,
                    None,
                    client_attributes,
                    config,
                    debug,
                    Box::new(Layout::default()),
                )
                .expect("TEST")
            })
            .unwrap();
        let pane_layout = initial_layout.unwrap_or_default();
        let pane_count = pane_layout.extract_run_instructions().len();
        let floating_pane_count = initial_floating_panes_layout.len();
        let mut pane_ids = vec![];
        let mut floating_pane_ids = vec![];
        let mut plugin_ids = HashMap::new();
        plugin_ids.insert(
            RunPluginOrAlias::Alias(PluginAlias {
                name: "fixture_plugin_for_tests".to_owned(),
                configuration: Some(Default::default()),
                run_plugin: Some(RunPlugin {
                    location: RunPluginLocation::parse("file:/path/to/fake/plugin", None).unwrap(),
                    configuration: PluginUserConfiguration::default(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            vec![1],
        );
        for i in 0..pane_count {
            pane_ids.push((i as u32, None));
        }
        for i in 0..floating_pane_count {
            floating_pane_ids.push((i as u32, None));
        }
        let default_shell = None;
        let tab_name = None;
        let tab_index = self.last_opened_tab_index.map(|l| l + 1).unwrap_or(0);
        let should_change_focus_to_new_tab = true;
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(pane_layout.clone()),
            initial_floating_panes_layout.clone(),
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            (self.main_client_id, false),
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            pane_layout,
            initial_floating_panes_layout,
            pane_ids,
            floating_pane_ids,
            plugin_ids,
            tab_index,
            true,
            (self.main_client_id, false),
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
        screen_thread
    }
    pub fn new_tab(&mut self, tab_layout: TiledPaneLayout) {
        let pane_count = tab_layout.extract_run_instructions().len();
        let mut pane_ids = vec![];
        let plugin_ids = HashMap::new();
        let default_shell = None;
        let tab_name = None;
        let tab_index = self.last_opened_tab_index.map(|l| l + 1).unwrap_or(0);
        for i in 0..pane_count {
            pane_ids.push((i as u32, None));
        }
        let should_change_focus_to_new_tab = true;
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(tab_layout.clone()),
            vec![], // floating_panes_layout
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            (self.main_client_id, false),
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            tab_layout,
            vec![], // floating_panes_layout
            pane_ids,
            vec![], // floating panes ids
            plugin_ids,
            0,
            true,
            (self.main_client_id, false),
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
    }
    pub fn new_tab_with_plugins(&mut self, plugin_pane_ids: Vec<u32>) {
        // Build a layout where each child is a plugin pane
        let fake_plugin_url = "file:/path/to/fake/plugin";
        let run_plugin = RunPluginOrAlias::from_url(fake_plugin_url, &None, None, None).unwrap();
        let mut tab_layout = TiledPaneLayout::default();
        tab_layout.children_split_direction = SplitDirection::Vertical;
        tab_layout.children = plugin_pane_ids
            .iter()
            .map(|_| {
                let mut child = TiledPaneLayout::default();
                child.run = Some(Run::Plugin(run_plugin.clone()));
                child
            })
            .collect();
        let pane_ids = vec![]; // no terminal panes
        let mut plugin_ids = HashMap::new();
        plugin_ids.insert(run_plugin, plugin_pane_ids);
        let default_shell = None;
        let tab_name = None;
        let tab_index = self.last_opened_tab_index.map(|l| l + 1).unwrap_or(0);
        let should_change_focus_to_new_tab = true;
        let _ = self.to_screen.send(ScreenInstruction::NewTab(
            None,
            default_shell,
            Some(tab_layout.clone()),
            vec![], // floating_panes_layout
            tab_name,
            (Some(vec![]), Some(vec![])), // swap layouts (None would fall back to Screen::default_layout)
            None,                         // initial_panes
            false,
            should_change_focus_to_new_tab,
            (self.main_client_id, false),
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            tab_layout,
            vec![], // floating_panes_layout
            pane_ids,
            vec![], // floating panes ids
            plugin_ids,
            tab_index,
            true,
            (self.main_client_id, false),
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
    }
    pub fn teardown(&mut self, threads: Vec<std::thread::JoinHandle<()>>) {
        let _ = self.to_pty.send(PtyInstruction::Exit);
        let _ = self.to_pty_writer.send(PtyWriteInstruction::Exit);
        let _ = self.to_screen.send(ScreenInstruction::Exit);
        let _ = self.to_server.send(ServerInstruction::KillSession);
        let _ = self.to_plugin.send(PluginInstruction::Exit);
        let _ = self.to_background_jobs.send(BackgroundJob::Exit);
        for thread in threads {
            let _ = thread.join();
        }
    }
    pub fn clone_session_metadata(&self) -> SessionMetaData {
        // hack that only clones the clonable parts of SessionMetaData
        SessionMetaData {
            senders: self.session_metadata.senders.clone(),
            default_shell: self.session_metadata.default_shell.clone(),
            screen_thread: None,
            pty_thread: None,
            plugin_thread: None,
            pty_writer_thread: None,
            background_jobs_thread: None,
            session_recorder_thread: None,
            session_configuration: self.session_metadata.session_configuration.clone(),
            current_input_modes: self.session_metadata.current_input_modes.clone(),
            web_sharing: WebSharing::Off,
            config_file_path: self.session_metadata.config_file_path.clone(),
            key_passthrough_clients: self.session_metadata.key_passthrough_clients.clone(),
        }
    }
}

impl MockScreen {
    pub fn new(size: Size) -> Self {
        let (to_server, server_receiver): ChannelWithContext<ServerInstruction> =
            channels::bounded(50);
        let to_server = SenderWithContext::new(to_server);

        let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> =
            channels::unbounded();
        let to_screen = SenderWithContext::new(to_screen);

        let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> =
            channels::unbounded();
        let to_plugin = SenderWithContext::new(to_plugin);
        let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
        let to_pty = SenderWithContext::new(to_pty);

        let (to_pty_writer, pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
            channels::unbounded();
        let to_pty_writer = SenderWithContext::new(to_pty_writer);

        let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
            channels::unbounded();
        let to_background_jobs = SenderWithContext::new(to_background_jobs);

        let client_attributes = ClientAttributes {
            size,
            ..Default::default()
        };

        let session_metadata = SessionMetaData {
            senders: ThreadSenders {
                to_screen: Some(to_screen.clone()),
                to_pty: Some(to_pty.clone()),
                to_plugin: Some(to_plugin.clone()),
                to_pty_writer: Some(to_pty_writer.clone()),
                to_background_jobs: Some(to_background_jobs.clone()),
                to_server: Some(to_server.clone()),
                session_recorder: None,
                should_silently_fail: true,
            },
            default_shell: None,
            screen_thread: None,
            pty_thread: None,
            plugin_thread: None,
            pty_writer_thread: None,
            background_jobs_thread: None,
            session_recorder_thread: None,
            session_configuration: Default::default(),
            current_input_modes: HashMap::new(),
            web_sharing: WebSharing::Off,
            config_file_path: None,
            key_passthrough_clients: Default::default(),
        };

        let os_input = FakeInputOutput::default();
        let config_options = Options::default();
        let mut config = Config::default();
        config.options.pane_frame_style = Some(PaneFrameStyle::Full);
        config.options.stacked_pane_list = Some(false);
        let main_client_id = 1;

        let _ = to_screen.send(ScreenInstruction::RecomputeTabSize(main_client_id, size));

        let received_background_jobs = Arc::new(Mutex::new(vec![]));
        std::thread::Builder::new()
            .name("background_jobs_thread".to_string())
            .spawn({
                let to_screen = to_screen.clone();
                let received_background_jobs = received_background_jobs.clone();
                move || loop {
                    let (event, _err_ctx) = background_jobs_receiver
                        .recv()
                        .expect("failed to receive event on channel");
                    received_background_jobs.lock().unwrap().push(event.clone());
                    match event {
                        BackgroundJob::RenderToClients => {
                            let _ = to_screen.send(ScreenInstruction::RenderToClients);
                        },
                        BackgroundJob::Exit => {
                            break;
                        },
                        _ => {},
                    }
                }
            })
            .unwrap();
        MockScreen {
            main_client_id,
            pty_receiver: Some(pty_receiver),
            pty_writer_receiver: Some(pty_writer_receiver),
            background_jobs_receiver: None,
            received_background_jobs,
            screen_receiver: Some(screen_receiver),
            server_receiver: Some(server_receiver),
            plugin_receiver: Some(plugin_receiver),
            to_screen,
            to_pty,
            to_plugin,
            to_server,
            to_pty_writer,
            to_background_jobs,
            os_input,
            client_attributes,
            config_options,
            session_metadata,
            last_opened_tab_index: None,
            config,
            default_layout: Box::new(Layout::default()),
            advanced_mouse_actions: true,
        }
    }
    pub fn set_advanced_hover_effects(&mut self, advanced_mouse_actions: bool) {
        self.advanced_mouse_actions = advanced_mouse_actions;
    }
    pub fn drop_all_pty_messages(&mut self) {
        let pty_receiver = self.pty_receiver.take();
        std::thread::Builder::new()
            .name("pty_thread".to_string())
            .spawn({
                move || {
                    if let Some(pty_receiver) = pty_receiver {
                        loop {
                            let (event, _err_ctx) = pty_receiver
                                .recv()
                                .expect("failed to receive event on channel");
                            match event {
                                PtyInstruction::Exit => {
                                    break;
                                },
                                _ => {
                                    // here the event will be dropped - we do this so that the completion_tx will drop and release the
                                    // test actions
                                },
                            }
                        }
                    }
                }
            })
            .unwrap();
    }
}

macro_rules! log_actions_in_thread {
    ( $arc_mutex_log:expr, $exit_event:path, $receiver:expr ) => {
        std::thread::Builder::new()
            .name("pty_writer_thread".to_string())
            .spawn({
                let log = $arc_mutex_log.clone();
                move || loop {
                    let (event, _err_ctx) = $receiver
                        .recv()
                        .expect("failed to receive event on channel");
                    match event {
                        $exit_event => {
                            log.lock().unwrap().push(event.clone());
                            break;
                        },
                        _ => {
                            log.lock().unwrap().push(event.clone());
                        },
                    }
                }
            })
            .unwrap()
    };
}

fn new_tab(screen: &mut Screen, pid: u32, tab_index: usize) {
    let client_id = 1;
    let new_terminal_ids = vec![(pid, None)];
    let new_plugin_ids = HashMap::new();
    screen
        .new_tab(tab_index, (vec![], vec![]), None, Some(client_id))
        .expect("TEST");
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![], // floating panes layout
            new_terminal_ids,
            vec![], // new floating terminal ids
            new_plugin_ids,
            tab_index,
            true,
            (client_id, false),
            None,
        )
        .expect("TEST");
}

/// `CwdChanged` announces a move once. The manifest says where every pane is
/// every time, which is what puts right a plugin that was not listening yet.
#[test]
fn a_pane_manifest_carries_the_pane_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen
        .terminal_cwds
        .insert(1, std::path::PathBuf::from("/work/hocus"));

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    let pane = manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == 1 && !pane.is_plugin)
        .expect("the terminal pane is in the manifest");
    assert_eq!(pane.cwd.as_deref(), Some("/work/hocus"));
}

/// A plugin is not a process in a folder, and a plugin id shares its number
/// space with a terminal id — so an unguarded lookup would hand it one.
#[test]
fn a_plugin_pane_has_no_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen
        .terminal_cwds
        .insert(1, std::path::PathBuf::from("/work/hocus"));

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    assert!(
        manifest
            .panes
            .values()
            .flatten()
            .chain(manifest.global_panes.iter())
            .filter(|pane| pane.is_plugin)
            .all(|pane| pane.cwd.is_none()),
        "no plugin pane was given a folder"
    );
}

#[test]
fn global_pane_renders_once_for_clients_on_different_tabs() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    let mut global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        3,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    global_pane.handle_pty_bytes(b"GLOBAL".to_vec());
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(10),
            },
            false,
        )
        .unwrap();
    screen
        .global_panes
        .focus_pane(PaneId::Terminal(99), 1)
        .unwrap();
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    let (render_instruction, _) = server_receiver.recv().expect("render instruction");
    let ServerInstruction::Render(Some(mut rendered_clients)) = render_instruction else {
        panic!("expected client render output")
    };
    let mut client_1_grid = grid_for_render_snapshot(size);
    let mut client_2_grid = grid_for_render_snapshot(size);
    let (client_1_cursor, client_1_snapshot) = take_snapshot_and_cursor_coordinates(
        &rendered_clients.remove(&1).expect("client 1 output"),
        &mut client_1_grid,
    );
    let (client_2_cursor, client_2_snapshot) = take_snapshot_and_cursor_coordinates(
        &rendered_clients.remove(&2).expect("client 2 output"),
        &mut client_2_grid,
    );
    assert_eq!(client_1_cursor, Some((1, 0)));
    assert_ne!(client_2_cursor, Some((1, 0)));

    let normalized_snapshot =
        format!("client 1:\n{client_1_snapshot}\nclient 2:\n{client_2_snapshot}")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
    assert_snapshot!(
        normalized_snapshot,
        @r###"
    client 1:
    00 (C): GLOBAL   │
    01 (C):          │
    02 (C):          │
    03 (C):          │
    04 (C):          │
    05 (C):          │
    06 (C):          │
    07 (C):          │
    08 (C):          │
    09 (C):          │

    client 2:
    00 (C): GLOBAL   │
    01 (C):          │
    02 (C):          │
    03 (C):          │
    04 (C):          │
    05 (C):          │
    06 (C):          │
    07 (C):          │
    08 (C):          │
    09 (C):          │
    "###
    );
    for tab in screen.tabs.values() {
        assert_eq!(tab.get_display_area(), Size { cols: 30, rows: 10 });
        assert_eq!(tab.get_viewport().x, 10);
        assert!(!tab.get_all_pane_ids().contains(&PaneId::Terminal(99)));
    }
}

#[test]
fn closing_the_last_pane_with_a_visible_global_pane_exits_the_session() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 12);
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {} // drain the first frame

    // the user exits their last shell
    assert!(screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .extract_pane(PaneId::Terminal(1), true)
        .is_some());
    screen.render_to_clients().expect("TEST");

    assert!(
        screen.tabs.is_empty(),
        "a visible global pane must not keep an emptied tab alive"
    );
    assert!(
        std::iter::from_fn(|| server_receiver.try_recv().ok())
            .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))),
        "expected ServerInstruction::Render(None) to exit the session"
    );
}

#[test]
fn global_pane_render_does_not_move_the_cursor_of_a_client_focused_on_a_tab_pane() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let mut global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        3,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    global_pane.handle_pty_bytes(b"GLOBAL".to_vec());
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(10),
            },
            false,
        )
        .unwrap();
    // client 1 is left focused on its tab pane, not on the global pane

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    let next_client_1_render = || loop {
        let (instruction, _) = server_receiver.recv().expect("render instruction");
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            return rendered_clients.remove(&1).expect("client 1 output");
        }
    };

    // first frame: both the tab pane and the global pane render
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .handle_pty_bytes(1, b"tab pane".to_vec())
        .unwrap();
    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let cursor_in_tab_pane =
        take_snapshot_and_cursor_coordinates(&next_client_1_render(), &mut grid).0;
    assert!(
        cursor_in_tab_pane.is_some(),
        "the client's cursor should be visible in its tab pane"
    );

    // second frame: only the global pane has anything new to render
    screen.global_panes.handle_pty_bytes(99, b"MORE".to_vec());
    screen.render_to_clients().expect("TEST");
    let second_frame = next_client_1_render();
    let cursor_after_global_pane_render =
        take_snapshot_and_cursor_coordinates(&second_frame, &mut grid).0;

    assert_eq!(
        cursor_after_global_pane_render, cursor_in_tab_pane,
        "a global pane rendering must not strand the cursor at the end of its own contents"
    );
    assert!(
        second_frame.contains("\u{1b}[?25l"),
        "the cursor should be hidden while the global pane's contents are drawn"
    );
}

fn insert_left_global_pane(screen: &mut Screen, cols: usize) {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(cols),
            },
            false,
        )
        .unwrap();
}

fn assert_tiled_panes_are_bounded_by_left_global_pane(screen: &Screen, global_cols: usize) {
    let screen_cols = screen.surface_size().cols;
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area_origin(), (global_cols, 0));
    assert_eq!(tab.get_viewport().x, global_cols);
    let geoms: Vec<_> = tab
        .get_tiled_panes()
        .map(|(id, p)| (*id, p.position_and_size()))
        .collect();
    assert!(geoms.len() > 1, "expected more than one tiled pane");
    for (id, geom) in &geoms {
        assert!(
            geom.x >= global_cols,
            "pane {:?} at x {} is underneath the global pane (which is {} columns wide)",
            id,
            geom.x,
            global_cols
        );
        assert!(
            geom.x + geom.cols.as_usize() <= screen_cols,
            "pane {:?} overflows the screen: {} + {} > {}",
            id,
            geom.x,
            geom.cols.as_usize(),
            screen_cols
        );
    }
    let rightmost_edge = geoms
        .iter()
        .map(|(_, geom)| geom.x + geom.cols.as_usize())
        .max()
        .unwrap();
    assert_eq!(
        rightmost_edge, screen_cols,
        "tiled panes do not reach the right edge of the screen"
    );
}

/// The tab's ui strips (tab bar, status bar) span the whole screen and the left global pane fills
/// only the rows between them.
#[test]
fn tab_chrome_spans_the_full_width_beside_a_left_global_pane() {
    let client_id = 1;
    let global_cols = 10;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    // a non-selectable pane on the bottom edge is chrome, same as the status-bar plugin
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Down),
                borderless: Some(true),
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(2), false);

    insert_left_global_pane(&mut screen, global_cols);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    screen.render_to_clients().unwrap();

    let tab = screen.get_active_tab(client_id).unwrap();
    let (chrome_top, chrome_bottom) = tab.chrome_rows();
    assert_eq!(chrome_top, 0);
    assert!(
        chrome_bottom > 0,
        "the tab has no bottom chrome to speak of"
    );
    let widened_chrome = tab
        .get_tiled_panes()
        .find(|(id, _)| **id == PaneId::Terminal(2))
        .map(|(_, pane)| pane.position_and_size())
        .unwrap();
    assert_eq!(
        (widened_chrome.x, widened_chrome.cols.as_usize()),
        (0, screen.surface_size().cols),
        "the chrome pane does not span the full screen width"
    );

    let global_pane_geom = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (global_pane_geom.y, global_pane_geom.rows.as_usize()),
        (0, screen.surface_size().rows - chrome_bottom),
        "the global pane covers rows belonging to the tab's chrome"
    );
    assert_eq!(global_pane_geom.x, 0);
    assert_eq!(global_pane_geom.cols.as_usize(), global_cols);
}

/// Add a non-selectable borderless pane on the bottom edge of `tab_id`, standing in for the
/// status-bar plugin.
fn add_chrome_pane(screen: &mut Screen, tab_id: usize, pane_id: u32, client_id: ClientId) {
    let tab = screen.tabs.get_mut(&tab_id).unwrap();
    tab.new_pane(
        PaneId::Terminal(pane_id),
        None,
        None,
        false,
        true,
        NewPanePlacement::Tiled {
            direction: Some(Direction::Down),
            borderless: Some(true),
        },
        Some(client_id),
        None,
    )
    .unwrap();
    tab.set_pane_selectable(PaneId::Terminal(pane_id), false);
}

/// A tab opened *after* a global pane is already docked is built over the full surface and then
/// narrowed to the center between the global panes, so its chrome has to be re-widened for it as
/// well - and has to stay that way when the tabs are switched between.
#[test]
fn tab_chrome_spans_the_full_width_in_every_tab() {
    let client_id = 1;
    let global_cols = 10;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_chrome_pane(&mut screen, 0, 2, client_id);

    insert_left_global_pane(&mut screen, global_cols);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    screen.render_to_clients().unwrap();

    new_tab(&mut screen, 3, 1);
    add_chrome_pane(&mut screen, 1, 4, client_id);
    screen.render_to_clients().unwrap();

    let surface_cols = screen.surface_size().cols;
    let chrome_geom = |screen: &Screen, tab_id: usize, pane_id: u32| {
        screen
            .tabs
            .get(&tab_id)
            .unwrap()
            .get_tiled_panes()
            .find(|(id, _)| **id == PaneId::Terminal(pane_id))
            .map(|(_, pane)| pane.position_and_size())
            .unwrap()
    };
    for (tab_id, pane_id) in [(0, 2), (1, 4)] {
        let geom = chrome_geom(&screen, tab_id, pane_id);
        assert_eq!(
            (geom.x, geom.cols.as_usize()),
            (0, surface_cols),
            "the chrome of tab {} does not span the full screen width",
            tab_id
        );
    }

    // and switching back to the first tab does not strand either of them
    screen.go_to_tab(1, client_id).unwrap();
    screen.render_to_clients().unwrap();
    for (tab_id, pane_id) in [(0, 2), (1, 4)] {
        let geom = chrome_geom(&screen, tab_id, pane_id);
        assert_eq!(
            (geom.x, geom.cols.as_usize()),
            (0, surface_cols),
            "the chrome of tab {} does not span the full screen width after switching tabs",
            tab_id
        );
    }
}

fn global_pane_cols(screen: &Screen) -> usize {
    screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size()
        .cols
        .as_usize()
}

#[test]
fn resizing_a_focused_global_pane_only_moves_the_border_it_has() {
    use zellij_utils::data::{Direction, Resize, ResizeStrategy};

    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);
    screen.focus_global_pane(global_pane_id, client_id).unwrap();
    assert_eq!(global_pane_cols(&screen), 10);

    // a left global pane has no horizontal border to move up or down
    screen
        .resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, Some(Direction::Up)),
            global_pane_id,
        )
        .unwrap();
    assert_eq!(global_pane_cols(&screen), 10);

    screen
        .resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
            global_pane_id,
        )
        .unwrap();
    assert_eq!(global_pane_cols(&screen), 11);

    screen
        .resize_pane_with_id(ResizeStrategy::new(Resize::Decrease, None), global_pane_id)
        .unwrap();
    assert_eq!(global_pane_cols(&screen), 10);
}

#[test]
fn resizing_the_tab_pane_beside_a_global_pane_moves_their_shared_border() {
    use zellij_utils::data::{Direction, Resize, ResizeStrategy};

    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .focus_pane_on_edge(Direction::Left, client_id);

    // growing the tab pane towards the global pane shrinks it
    assert!(screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
        )
        .unwrap());
    assert_eq!(global_pane_cols(&screen), 9);

    assert!(screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Decrease, Some(Direction::Left)),
        )
        .unwrap());
    assert_eq!(global_pane_cols(&screen), 10);

    // there is no global pane on the right, and a directionless resize stays tab-local
    assert!(!screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .unwrap());
    assert!(!screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Increase, None),
        )
        .unwrap());
    assert_eq!(global_pane_cols(&screen), 10);
}

#[test]
fn dragging_the_border_of_a_global_pane_resizes_it() {
    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    // the separator cell of a left global pane is its own last column
    let border_column = 9;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(5, border_column)),
        client_id,
    );
    // the press starts a drag rather than focusing the pane
    assert_eq!(screen.global_panes.focused_pane_id(client_id), None);

    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(5, 14)),
        client_id,
    );
    assert_eq!(global_pane_cols(&screen), 15);
    assert_tiled_panes_are_bounded_by_left_global_pane(&screen, 15);

    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(5, 14)),
        client_id,
    );
    // once released, moving over the tab area no longer resizes anything
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(5, 30)),
        client_id,
    );
    assert_eq!(global_pane_cols(&screen), 15);
}

#[test]
fn a_click_on_tab_chrome_beside_a_global_pane_is_not_a_border_drag() {
    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    // a non-selectable pane on the bottom edge is chrome, same as the status-bar plugin. It draws
    // no boundary, so the row it shares with the global pane below it is a click, not a border
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Down),
                borderless: Some(true),
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(2), false);
    add_global_terminal(&mut screen, 42);
    screen
        .resize_to_screen(Size { cols: 40, rows: 12 })
        .unwrap();
    screen.render_to_clients().unwrap();

    let global_pane_geom = |screen: &Screen| {
        screen
            .global_panes
            .pane(PaneId::Terminal(42))
            .unwrap()
            .position_and_size()
    };
    let border_line = global_pane_geom(&screen).y as i32 - 1;
    let rows_before = global_pane_geom(&screen).rows.as_usize();
    // the row is the global pane's border, it is only the chrome on top of it that saves the click
    assert!(screen
        .global_panes
        .border_pane_id_at(&Position::new(border_line, 5))
        .is_some());
    assert!(screen
        .get_active_tab(client_id)
        .unwrap()
        .position_is_on_ui_pane(&Position::new(border_line, 5)));

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(border_line, 5)),
        client_id,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(border_line - 3, 5)),
        client_id,
    );
    assert_eq!(global_pane_geom(&screen).rows.as_usize(), rows_before);
}

#[test]
fn adding_a_tiled_pane_stays_beside_a_left_global_pane() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    insert_left_global_pane(&mut screen, 10);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();

    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();

    assert_tiled_panes_are_bounded_by_left_global_pane(&screen, 10);
}

/// A tab with two tiled panes (1 and 2) plus one global pane (99) on the left edge. Focus is left
/// on the newly created pane 2, which is the last pane of the tab's cycle.
fn screen_with_two_tiled_panes_and_a_global_pane() -> Screen {
    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    insert_left_global_pane(&mut screen, 10);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client_id),
            None,
        )
        .unwrap();
    screen
}

#[test]
fn focus_cycle_walks_through_global_panes_and_back_into_the_tab() {
    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(2))
    );

    // the global panes form a segment appended to the end of the tab's cycle
    screen.cycle_focus(client_id, true);
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));
    screen.cycle_focus(client_id, true);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(1)),
        "cycling past the last global pane should wrap back into the tab"
    );
    screen.cycle_focus(client_id, true);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(2))
    );

    // and backwards is the exact mirror
    screen.cycle_focus(client_id, false);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(1))
    );
    screen.cycle_focus(client_id, false);
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));
    screen.cycle_focus(client_id, false);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(2))
    );
}

#[test]
fn move_focus_crosses_into_and_out_of_a_left_global_pane() {
    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);

    // travelling right lands on the leftmost pane, so it has nowhere left to go inside the tab
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .focus_pane_on_edge(Direction::Right, client_id);
    let leftmost_pane_id = screen.get_active_pane_id(&client_id).unwrap();
    let moved = screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .move_focus_left(client_id)
        .unwrap();
    assert!(!moved, "expected to be at the tab's left edge");

    assert!(screen.enter_global_pane_in_direction(Direction::Left, client_id));
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));

    // moving further left is the session boundary - focus stays put
    assert!(screen.global_pane_handles_direction(Direction::Left, client_id));
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));

    // moving right returns to the tab pane nearest the global pane
    assert!(screen.global_pane_handles_direction(Direction::Right, client_id));
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(leftmost_pane_id)
    );
}

#[test]
fn adding_a_tiled_pane_with_swap_layouts_stays_beside_a_left_global_pane() {
    // with swap layouts the new pane goes through the layout applier rather than the constraint
    // system, which positions panes in the tab's display area from scratch
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    use std::collections::BTreeMap;
    use zellij_utils::input::layout::LayoutConstraint;

    let swap_tiled_layouts = vec![(
        BTreeMap::from([(LayoutConstraint::NoConstraint, TiledPaneLayout::default())]),
        Some("swap".to_owned()),
    )];
    screen
        .new_tab(0, (swap_tiled_layouts, vec![]), None, Some(1))
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(1, None)],
            vec![],
            HashMap::new(),
            0,
            true,
            (1, false),
            None,
        )
        .unwrap();
    insert_left_global_pane(&mut screen, 10);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();

    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();

    assert_tiled_panes_are_bounded_by_left_global_pane(&screen, 10);
}

#[test]
fn screen_resizes_tabs_directly_to_the_global_center_without_accumulating_origin() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(10),
            },
            false,
        )
        .unwrap();

    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area_origin(), (10, 0));
    assert_eq!(tab.get_display_area(), Size { cols: 40, rows: 12 });
    assert_eq!(tab.get_pane_with_id(PaneId::Terminal(1)).unwrap().x(), 10);

    screen
        .resize_to_screen(Size { cols: 60, rows: 14 })
        .unwrap();
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area_origin(), (10, 0));
    assert_eq!(tab.get_display_area(), Size { cols: 50, rows: 14 });
    assert_eq!(tab.get_pane_with_id(PaneId::Terminal(1)).unwrap().x(), 10);
}

fn insert_global_pane_on_edge(
    screen: &mut Screen,
    id: u32,
    edge: zellij_utils::data::GlobalPaneEdge,
    cols: usize,
) {
    use zellij_utils::data::GlobalPanePlacement;

    let global_pane = PaneFactory::terminal(
        id,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge,
                size: PercentOrFixed::Fixed(cols),
            },
            false,
        )
        .unwrap();
}

#[test]
fn opposite_edge_global_panes_reserve_boundary_space_for_tab_edge_panes() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Left, 8);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 8);

    screen.render_to_clients().expect("TEST");

    let tab = screen.tabs.get(&0).unwrap();
    let pane = tab.get_pane_with_id(PaneId::Terminal(1)).unwrap();
    // the pane abuts the right global pane, so its last column is drawn as a boundary and must be
    // reserved from its content (and pty size)
    assert_eq!(
        pane.get_content_offset().right,
        1,
        "tab pane abutting a right global pane must reserve a boundary column"
    );
    assert_eq!(pane.get_content_columns(), pane.cols() - 1);
}

#[test]
fn right_only_global_pane_reserves_boundary_space_for_tab_edge_panes() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 8);

    screen.render_to_clients().expect("TEST");

    let tab = screen.tabs.get(&0).unwrap();
    let pane = tab.get_pane_with_id(PaneId::Terminal(1)).unwrap();
    assert_eq!(
        pane.get_content_offset().right,
        1,
        "tab pane abutting a right global pane must reserve a boundary column"
    );
    assert_eq!(pane.get_content_columns(), pane.cols() - 1);
}

#[test]
fn opposite_vertical_edge_global_panes_reserve_boundary_space_for_tab_edge_panes() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Top, 4);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Bottom, 4);

    screen.render_to_clients().expect("TEST");

    let tab = screen.tabs.get(&0).unwrap();
    let pane = tab.get_pane_with_id(PaneId::Terminal(1)).unwrap();
    // the pane abuts the bottom global pane, so its last row is drawn as a boundary and must be
    // reserved from its content (and pty size)
    assert_eq!(
        pane.get_content_offset().bottom,
        1,
        "tab pane abutting a bottom global pane must reserve a boundary row"
    );
    assert_eq!(pane.get_content_rows(), pane.rows() - 1);
}

#[test]
fn hidden_and_reshown_global_pane_keeps_boundary_reservation() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 8);
    screen.render_to_clients().expect("TEST");

    screen.hide_pane(PaneId::Terminal(99)).expect("TEST");
    screen.render_to_clients().expect("TEST");
    let pane = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(1))
        .unwrap();
    assert_eq!(pane.get_content_offset().right, 0);

    screen.show_pane(PaneId::Terminal(99)).expect("TEST");
    screen.render_to_clients().expect("TEST");
    let pane = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(1))
        .unwrap();
    assert_eq!(
        pane.get_content_offset().right,
        1,
        "tab pane abutting the reshown right global pane must reserve a boundary column"
    );
}

#[test]
fn newly_added_client_sees_existing_global_panes() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let mut global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    global_pane.handle_pty_bytes(b"GLOBAL ONLY".to_vec());
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(12),
            },
            false,
        )
        .unwrap();

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    let _ = server_receiver.recv().expect("initial render");

    screen.add_client(2, false).expect("TEST");
    screen.render_to_clients().expect("TEST");

    let mut client_2_output = None;
    while let Ok((instruction, _)) = server_receiver.try_recv() {
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            if let Some(output) = rendered_clients.remove(&2) {
                client_2_output = Some(output);
            }
        }
    }
    let client_2_output = client_2_output.expect("client 2 should receive a render");
    let mut grid = grid_for_render_snapshot(size);
    let snapshot = take_snapshot_and_cursor_coordinates(&client_2_output, &mut grid).1;
    assert!(
        snapshot.contains("GLOBAL ONLY"),
        "newly added client should see the global pane contents, got:\n{snapshot}"
    );
}

#[test]
fn global_pane_fake_cursor_does_not_move_other_clients_cursor() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    let global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            false,
        )
        .unwrap();
    screen
        .focus_pane_with_id(PaneId::Terminal(99), false, false, 1)
        .expect("TEST");

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");

    let (render_instruction, _) = server_receiver.recv().expect("render instruction");
    let ServerInstruction::Render(Some(rendered_clients)) = render_instruction else {
        panic!("expected client render output")
    };

    // client 1 is focused on the global pane: their cursor must be inside it
    let mut client_1_grid = grid_for_render_snapshot(size);
    let (client_1_cursor, _) = take_snapshot_and_cursor_coordinates(
        rendered_clients.get(&1).expect("client 1 output"),
        &mut client_1_grid,
    );
    assert!(
        client_1_cursor.is_some_and(|(_x, y)| y >= 7),
        "client 1 cursor should be inside the bottom global pane, got: {client_1_cursor:?}"
    );

    // client 2 is focused on the tab pane: the fake cursor drawn for client 1 inside the global
    // pane must not have moved client 2's host cursor out of the tab area
    let mut client_2_grid = grid_for_render_snapshot(size);
    let (client_2_cursor, _) = take_snapshot_and_cursor_coordinates(
        rendered_clients.get(&2).expect("client 2 output"),
        &mut client_2_grid,
    );
    assert!(
        client_2_cursor.is_some_and(|(_x, y)| y < 7),
        "client 2 cursor should stay inside the tab area, got: {client_2_cursor:?}"
    );
}

#[test]
fn global_terminal_creation_preserves_placement_from_pty_to_screen() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Bottom,
        size: PercentOrFixed::Fixed(3),
    };
    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_bus = Bus::new(
        vec![mock_screen.pty_receiver.take().unwrap()],
        Some(&mock_screen.to_screen),
        Some(&mock_screen.to_pty),
        Some(&mock_screen.to_plugin),
        Some(&mock_screen.to_server),
        Some(&mock_screen.to_pty_writer),
        Some(&mock_screen.to_background_jobs),
        Some(Box::new(mock_screen.os_input.clone())),
    );
    let pty = Pty::new(pty_bus, false, None, None);
    let pty_thread = std::thread::spawn(move || {
        pty_thread_main(pty, Box::new(Layout::default())).expect("pty thread")
    });

    route_arbitrary_action_to_server(
        &mock_screen.clone_session_metadata(),
        Action::NewPane {
            direction: None,
            pane_name: Some("global terminal".to_owned()),
            start_suppressed: false,
            global_placement: Some(placement),
        },
        1,
    );

    let screen_receiver = mock_screen.screen_receiver.take().unwrap();
    let instruction = loop {
        let (instruction, _) = screen_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("global pane screen instruction");
        if matches!(instruction, ScreenInstruction::NewGlobalPane { .. }) {
            break instruction;
        }
    };
    match instruction {
        ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(42),
            placement: actual,
            ..
        } => assert_eq!(actual, placement),
        other => panic!("expected global terminal creation, got {other:?}"),
    }

    let _ = mock_screen.to_pty.send(PtyInstruction::Exit);
    let _ = mock_screen.to_background_jobs.send(BackgroundJob::Exit);
    pty_thread.join().unwrap();
}

#[test]
fn global_new_tiled_pane_route_preserves_command_shell_and_spawn_context() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(8),
    };
    let default_shell = TerminalAction::RunCommand(
        RunCommandAction {
            command: PathBuf::from("default-shell"),
            ..Default::default()
        }
        .into(),
    );
    let actions = [
        Action::NewTiledPane {
            direction: Some(Direction::Right),
            command: Some(RunCommandAction {
                command: PathBuf::from("global-command"),
                ..Default::default()
            }),
            pane_name: Some("command global".to_owned()),
            near_current_pane: false,
            no_focus: false,
            borderless: Some(true),
            tab_id: None,
            global_placement: Some(placement),
        },
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: Some("shell global".to_owned()),
            near_current_pane: false,
            no_focus: true,
            borderless: Some(false),
            tab_id: None,
            global_placement: Some(placement),
        },
    ];
    let invalid_actions = [
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: None,
            near_current_pane: true,
            no_focus: false,
            borderless: None,
            tab_id: None,
            global_placement: Some(placement),
        },
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(2),
            global_placement: Some(placement),
        },
    ];
    let route_results: Vec<_> = actions
        .into_iter()
        .map(|action| {
            route_action(
                action,
                1,
                None,
                None,
                mock_screen.session_metadata.senders.clone(),
                Some(default_shell.clone()),
                None,
                InputMode::Normal,
                None,
            )
        })
        .collect();
    let invalid_route_results: Vec<_> = invalid_actions
        .into_iter()
        .map(|action| {
            route_action(
                action,
                1,
                None,
                None,
                mock_screen.session_metadata.senders.clone(),
                Some(default_shell.clone()),
                None,
                InputMode::Normal,
                None,
            )
        })
        .collect();
    let spawned: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    mock_screen.teardown(vec![]);

    assert!(route_results.iter().all(Result::is_ok));
    assert!(invalid_route_results[0]
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("near-current-pane"));
    assert!(invalid_route_results[1]
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("cannot target a tab"));
    assert_eq!(spawned.len(), 2);
    assert!(matches!(
        &spawned[0],
        PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(command)),
            Some(name),
            _,
            _,
            ClientTabIndexOrPaneId::ClientId(1),
            _,
            _,
            Some(context),
        ) if command.command == PathBuf::from("global-command")
            && name == "command global"
            && context.placement == placement
            && context.borderless
            && context.should_focus
    ));
    assert!(matches!(
        &spawned[1],
        PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(command)),
            Some(name),
            _,
            _,
            ClientTabIndexOrPaneId::ClientIdNoFocus(1),
            _,
            _,
            Some(context),
        ) if command.command == PathBuf::from("default-shell")
            && name == "shell global"
            && context.placement == placement
            && !context.borderless
            && !context.should_focus
    ));
}

#[test]
fn global_insertion_failure_reports_error_cleans_up_once_and_keeps_screen_alive() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(41),
            initial_pane_title: Some("invalid global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(10),
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(failed_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    let failed = failed_rx.blocking_recv().unwrap();

    let (succeeded_tx, succeeded_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(42),
            initial_pane_title: Some("valid global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(succeeded_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    let succeeded = succeeded_rx.blocking_recv().unwrap();

    let (duplicate_tx, duplicate_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(42),
            initial_pane_title: Some("duplicate global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(duplicate_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    let duplicate = duplicate_rx.blocking_recv().unwrap();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ClosePane(
            PaneId::Terminal(42),
            None,
            Some(NotificationEnd::new(closed_tx)),
            None,
        ))
        .unwrap();
    closed_rx.blocking_recv().unwrap();
    let pty_instructions: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    let invalid_cleanup_count = pty_instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::ClosePane(PaneId::Terminal(41), None)
            )
        })
        .count();
    let existing_backend_cleanup_count = pty_instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::ClosePane(PaneId::Terminal(42), None)
            )
        })
        .count();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(failed.exit_status, Some(1));
    assert_eq!(failed.affected_pane_id, None);
    assert!(failed.error_message.is_some());
    assert_eq!(invalid_cleanup_count, 1);
    assert_eq!(existing_backend_cleanup_count, 1);
    assert_eq!(succeeded.affected_pane_id, Some(PaneId::Terminal(42)));
    assert_eq!(duplicate.exit_status, Some(1));
    assert_eq!(duplicate.affected_pane_id, None);
    assert!(duplicate.error_message.is_some());
}

#[test]
fn failed_global_insertion_discards_pending_plugin_events_before_id_reuse() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(88, 1, b"STALE_BYTES".to_vec()),
        ]))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ShowPluginCursor(88, 1, Some((2, 0))))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::StartPluginLoadingIndication(
            88,
            LoadingIndication::new("STALE_START".to_owned()),
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ProgressPluginLoadingOffset(88))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::UpdatePluginLoadingStage(
            88,
            LoadingIndication::new("STALE_STAGE".to_owned()),
        ))
        .unwrap();

    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/88", &None, None, None).unwrap();
    let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(88),
            initial_pane_title: Some("invalid global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin.clone())),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(10),
            },
            borderless: true,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(failed_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    assert_eq!(failed_rx.blocking_recv().unwrap().exit_status, Some(1));

    let (succeeded_tx, succeeded_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(88),
            initial_pane_title: Some("reused global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin)),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            borderless: true,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(succeeded_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    assert_eq!(
        succeeded_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(88))
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let received_server_instructions = received_server_instructions.lock().unwrap();
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.iter(),
        size,
    );
    let (cursor, snapshot) = snapshots.last().expect("final render");
    assert!(!snapshot.contains("STALE_BYTES"), "{snapshot}");
    assert!(!snapshot.contains("STALE_STAGE"), "{snapshot}");
    assert_ne!(*cursor, Some((2, 0)));
}

#[test]
fn early_global_plugin_bytes_replay_before_already_queued_later_bytes() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(90, 1, b"EARLY".to_vec()),
        ]))
        .unwrap();
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(90),
            initial_pane_title: Some("ordered global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(
                RunPluginOrAlias::from_url("file:/path/to/global/plugin/90", &None, None, None)
                    .unwrap(),
            )),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            borderless: true,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(completion_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(90, 1, b"LATER".to_vec()),
        ]))
        .unwrap();

    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(90))
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let received_server_instructions = received_server_instructions.lock().unwrap();
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.iter(),
        size,
    );
    let (_, snapshot) = snapshots.last().expect("final render");
    assert!(snapshot.contains("07 (C): LATER"), "{snapshot}");
    assert!(!snapshot.contains("EARLY"), "{snapshot}");
}

#[test]
fn early_global_plugin_events_are_replayed_after_insertion() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 60, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let loading_start = LoadingIndication::new("EARLY_START".to_owned());
    let loading_stage = LoadingIndication::new("EARLY_STAGE".to_owned());
    std::thread::sleep(std::time::Duration::from_millis(450));

    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(77, 1, b"EARLY_BYTES".to_vec()),
        ]))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ShowPluginCursor(77, 1, Some((2, 0))))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(78, 1, b"loading grid".to_vec()),
        ]))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::StartPluginLoadingIndication(
            78,
            loading_start,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ProgressPluginLoadingOffset(78))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::UpdatePluginLoadingStage(
            78,
            loading_stage,
        ))
        .unwrap();

    for (plugin_id, edge, size, should_focus) in [
        (77, GlobalPaneEdge::Left, 18, true),
        (78, GlobalPaneEdge::Bottom, 3, false),
    ] {
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        mock_screen
            .to_screen
            .send(ScreenInstruction::NewGlobalPane {
                pane_id: PaneId::Plugin(plugin_id),
                initial_pane_title: Some(format!("global plugin {plugin_id}")),
                hold_for_command: None,
                invoked_with: Some(Run::Plugin(
                    RunPluginOrAlias::from_url(
                        &format!("file:/path/to/global/plugin/{plugin_id}"),
                        &None,
                        None,
                        None,
                    )
                    .unwrap(),
                )),
                placement: GlobalPanePlacement {
                    edge,
                    size: PercentOrFixed::Fixed(size),
                },
                borderless: true,
                initially_hidden: false,
                should_focus,
                client_id: Some(1),
                completion_tx: Some(NotificationEnd::new(completion_tx)),
                set_blocking: false,
                declaration_order: None,
            })
            .unwrap();
        assert_eq!(
            completion_rx.blocking_recv().unwrap().affected_pane_id,
            Some(PaneId::Plugin(plugin_id))
        );
        if plugin_id == 77 {
            mock_screen
                .to_screen
                .send(ScreenInstruction::RenderToClients)
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let unblocked_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::UnblockCliPipes(assets) => Some(
                assets
                    .into_iter()
                    .map(|asset| asset.plugin_id)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let received_server_instructions = received_server_instructions.lock().unwrap();
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.iter(),
        size,
    );
    let (_, snapshot) = snapshots.last().expect("final render");
    assert_eq!(unblocked_plugin_ids, vec![77, 78]);
    assert!(
        snapshots
            .iter()
            .any(|(_, snapshot)| snapshot.contains("EARLY_BYTES")),
        "{snapshots:?}"
    );
    assert!(snapshot.contains("EARLY_STAGE."), "{snapshot}");
    assert!(
        snapshots.iter().any(|(cursor, _)| *cursor == Some((2, 0))),
        "{snapshots:?}"
    );
}

#[test]
fn plugin_bytes_with_no_pane_still_unblock_cli_pipes() {
    // regression: unblocking used to be gated on finding a pane to draw the bytes into, so a
    // plugin that rendered without one (a background plugin, or one whose pane was closed
    // mid-render) never unblocked and anything piping to it hung forever
    let size = Size { cols: 60, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(450));

    // no pane exists for plugin 99, and none is ever created for it
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(99, 1, b"ORPHAN_BYTES".to_vec()),
        ]))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let unblocked_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::UnblockCliPipes(assets) => Some(
                assets
                    .into_iter()
                    .map(|asset| asset.plugin_id)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    mock_screen.teardown(vec![server_thread, screen_thread]);

    assert!(
        unblocked_plugin_ids.contains(&99),
        "a plugin that rendered without a pane must still unblock, got {unblocked_plugin_ids:?}"
    );
}

#[test]
fn closed_or_rejected_global_plugins_unload_exactly_once_through_pty() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_bus = Bus::new(
        vec![mock_screen.pty_receiver.take().unwrap()],
        Some(&mock_screen.to_screen),
        Some(&mock_screen.to_pty),
        Some(&mock_screen.to_plugin),
        Some(&mock_screen.to_server),
        Some(&mock_screen.to_pty_writer),
        Some(&mock_screen.to_background_jobs),
        Some(Box::new(mock_screen.os_input.clone())),
    );
    let pty = Pty::new(pty_bus, false, None, None);
    let pty_thread = std::thread::spawn(move || {
        pty_thread_main(pty, Box::new(Layout::default())).expect("pty thread")
    });
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(77),
            initial_pane_title: Some("global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin)),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(completion_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(77))
    );

    mock_screen
        .to_screen
        .send(ScreenInstruction::ClosePane(
            PaneId::Plugin(77),
            None,
            None,
            None,
        ))
        .unwrap();

    let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(78),
            initial_pane_title: Some("invalid global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(
                RunPluginOrAlias::from_url("file:/path/to/global/plugin/78", &None, None, None)
                    .unwrap(),
            )),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(10),
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(failed_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    let failed = failed_rx.blocking_recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let unloaded_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::Unload(plugin_id) => Some(plugin_id),
            _ => None,
        })
        .collect();
    mock_screen.teardown(vec![screen_thread, pty_thread]);

    assert_eq!(failed.exit_status, Some(1));
    assert_eq!(failed.affected_pane_id, None);
    assert_eq!(unloaded_plugin_ids, vec![77, 78]);
}

fn add_matching_global_plugin(
    screen: &mut Screen,
    plugin_id: u32,
    run_plugin: RunPluginOrAlias,
    hidden: bool,
) {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let pane = PaneFactory::plugin(
        plugin_id,
        Default::default(),
        plugin_sender,
        "global plugin".to_owned(),
        "global plugin".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().keys().copied().collect(),
        screen.style,
        Some(Run::Plugin(run_plugin)),
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    screen
        .global_panes
        .insert(
            pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(5),
            },
            hidden,
        )
        .unwrap();
    screen
        .global_panes
        .resize(zellij_utils::pane_size::Viewport {
            x: 0,
            y: 0,
            cols: screen.surface_size().cols,
            rows: screen.surface_size().rows,
        })
        .unwrap();
}

fn add_global_terminal(screen: &mut Screen, terminal_id: u32) {
    add_global_terminal_on_edge(
        screen,
        terminal_id,
        zellij_utils::data::GlobalPaneEdge::Bottom,
    )
}

fn add_global_terminal_on_edge(
    screen: &mut Screen,
    terminal_id: u32,
    edge: zellij_utils::data::GlobalPaneEdge,
) {
    use zellij_utils::data::GlobalPanePlacement;

    let pane = PaneFactory::terminal(
        terminal_id,
        Default::default(),
        screen.style,
        1,
        "global terminal".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global terminal".to_owned()),
        None,
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
        screen.osc8_hyperlinks,
        screen.explicitly_disable_kitty_keyboard_protocol,
        None,
    );
    screen
        .global_panes
        .insert(
            pane,
            GlobalPanePlacement {
                edge,
                size: PercentOrFixed::Fixed(3),
            },
            false,
        )
        .unwrap();
    let center = screen
        .global_panes
        .resize(screen.surface_size().into())
        .unwrap();
    for tab in screen.tabs.values_mut() {
        tab.resize_whole_tab_to(center).unwrap();
    }
}

#[test]
fn global_pane_manifest_reports_each_global_once_outside_tabs() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    add_global_terminal(&mut screen, 42);
    screen.global_panes.hide(PaneId::Terminal(42)).unwrap();

    let manifest = screen.generate_and_report_pane_state().unwrap();
    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Bottom,
        size: PercentOrFixed::Fixed(3),
    };

    assert_eq!(manifest.panes.len(), 2);
    assert!(manifest.panes.values().flatten().all(|pane| pane.id != 42));
    assert_eq!(manifest.global_panes.len(), 1);
    assert_eq!(manifest.global_panes[0].id, 42);
    assert_eq!(manifest.global_panes[0].global_placement, Some(placement));
    assert!(manifest.global_panes[0].is_hidden);

    let layout_manifest: zellij_utils::session_serialization::GlobalLayoutManifest =
        screen.get_layout_metadata(None, None).into();
    assert_eq!(layout_manifest.global_panes.len(), 1);
    assert_eq!(
        layout_manifest.global_panes[0].global_placement,
        Some(placement)
    );
    assert!(layout_manifest.global_panes[0].is_hidden);
}

fn assert_global_tab_command_rejected(
    mock_screen: &MockScreen,
    command_name: &str,
    build: impl FnOnce(NotificationEnd) -> ScreenInstruction,
) {
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(build(NotificationEnd::new(completion_tx)))
        .unwrap();
    let completion = completion_rx.blocking_recv().unwrap();
    assert_eq!(completion.exit_status, Some(1), "{command_name}");
    assert_eq!(
        completion.error_message.as_deref(),
        Some("global pane has no tab"),
        "{command_name}"
    );
}

#[test]
fn global_tab_only_screen_instructions_report_exact_completion_errors() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let global_id = PaneId::Terminal(42);
    let (created_tx, created_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: global_id,
            initial_pane_title: Some("global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(created_tx)),
            set_blocking: false,
            declaration_order: None,
        })
        .unwrap();
    created_rx.blocking_recv().unwrap();

    assert_global_tab_command_rejected(&mock_screen, "move", |completion| {
        ScreenInstruction::MovePane(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "swap", |completion| {
        ScreenInstruction::MovePaneBackwards(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-down", |completion| {
        ScreenInstruction::MovePaneDown(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-up", |completion| {
        ScreenInstruction::MovePaneUp(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-right", |completion| {
        ScreenInstruction::MovePaneRight(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-left", |completion| {
        ScreenInstruction::MovePaneLeft(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "previous-swap-layout", |completion| {
        ScreenInstruction::PreviousSwapLayout(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "next-swap-layout", |completion| {
        ScreenInstruction::NextSwapLayout(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break", |completion| {
        ScreenInstruction::BreakPane(None, 1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break-right", |completion| {
        ScreenInstruction::BreakPaneRight(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break-left", |completion| {
        ScreenInstruction::BreakPaneLeft(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "floating", |completion| {
        ScreenInstruction::TogglePaneEmbedOrFloating(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "fullscreen", |completion| {
        ScreenInstruction::ToggleActiveTerminalFullscreen(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "no-ui-fullscreen", |completion| {
        ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "pin", |completion| {
        ScreenInstruction::TogglePanePinned(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "stack", |completion| {
        ScreenInstruction::StackPanes(vec![global_id], 1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "coordinates", |completion| {
        ScreenInstruction::ChangeFloatingPanesCoordinates(
            vec![(global_id, FloatingPaneCoordinates::default())],
            Some(completion),
        )
    });
    assert_global_tab_command_rejected(&mock_screen, "move-id", |completion| {
        ScreenInstruction::MovePaneWithPaneIdCli(global_id, None, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-backwards-id", |completion| {
        ScreenInstruction::MovePaneBackwardsWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "fullscreen-id", |completion| {
        ScreenInstruction::ToggleFullscreenWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "no-ui-fullscreen-id", |completion| {
        ScreenInstruction::ToggleNoUiFullscreenWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "floating-id", |completion| {
        ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "pin-id", |completion| {
        ScreenInstruction::TogglePanePinnedWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break-to-new-tab", |completion| {
        ScreenInstruction::BreakPanesToNewTab {
            pane_ids: vec![global_id],
            default_shell: None,
            should_change_focus_to_new_tab: false,
            new_tab_name: None,
            client_id: 1,
            completion_tx: Some(completion),
        }
    });
    assert_global_tab_command_rejected(&mock_screen, "break-to-tab", |completion| {
        ScreenInstruction::BreakPanesToTabWithIndex {
            pane_ids: vec![global_id],
            tab_index: 0,
            should_change_focus_to_new_tab: false,
            client_id: 1,
            completion_tx: Some(completion),
        }
    });
    assert_global_tab_command_rejected(&mock_screen, "break-to-tab-id", |completion| {
        ScreenInstruction::BreakPanesToTabWithId {
            pane_ids: vec![global_id],
            tab_id: 0,
            should_change_focus_to_target_tab: false,
            client_id: 1,
            completion_tx: Some(completion),
        }
    });

    mock_screen.teardown(vec![screen_thread]);
}

#[test]
fn global_pane_commands_resolve_generic_ids_and_reject_tab_only_ids() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement, Resize, ResizeStrategy};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    let global_id = PaneId::Terminal(42);

    screen
        .focus_pane_with_id(global_id, false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(global_id));

    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    screen
        .write_to_pane_id(
            &None,
            b"global input".to_vec(),
            false,
            global_id,
            Some(1),
            None,
        )
        .unwrap();
    assert!(matches!(
        pty_writer_rx.recv().unwrap().0,
        PtyWriteInstruction::Write(bytes, 42, None) if bytes == b"global input"
    ));

    screen
        .with_pane(global_id, |pane| {
            pane.handle_pty_bytes(b"global command output".to_vec());
            pane.rename(b"renamed global".to_vec());
        })
        .unwrap();
    assert!(screen
        .with_pane(global_id, |pane| pane.dump_screen(false, None))
        .unwrap()
        .contains("global command output"));
    assert_eq!(
        screen
            .with_pane(global_id, |pane| pane.current_title())
            .unwrap(),
        "renamed global"
    );

    screen
        .resize_pane_with_id(ResizeStrategy::new(Resize::Increase, None), global_id)
        .unwrap();
    assert_eq!(
        screen.global_pane_placement(global_id).unwrap().size,
        PercentOrFixed::Fixed(4)
    );

    screen.hide_pane(global_id).unwrap();
    assert!(screen.global_panes.is_hidden(global_id).unwrap());
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
    screen.show_pane(global_id).unwrap();
    assert!(!screen.global_panes.is_hidden(global_id).unwrap());

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(6),
    };
    screen
        .set_global_pane_placement(global_id, placement)
        .unwrap();
    assert_eq!(screen.global_pane_placement(global_id), Some(placement));

    let (pty_tx, pty_rx) = channels::unbounded::<(PtyInstruction, ErrorContext)>();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let rerun = RunCommand {
        command: PathBuf::from("rerun-global"),
        ..Default::default()
    };
    screen
        .with_pane(global_id, |pane| pane.hold(Some(0), false, rerun.clone()))
        .unwrap();
    screen.rerun_command_pane_with_id(42, None);
    assert!(matches!(
        pty_rx.recv().unwrap().0,
        PtyInstruction::ReRunCommandInPane(PaneId::Terminal(42), command, None)
            if command == rerun
    ));

    let snapshot = (
        screen.global_pane_placement(global_id),
        screen.global_panes.is_hidden(global_id),
        screen
            .with_pane(global_id, |pane| pane.current_title())
            .unwrap(),
    );
    for command in [
        "move",
        "swap",
        "move-to-tab",
        "break",
        "stack",
        "floating",
        "fullscreen",
        "pin",
    ] {
        assert_eq!(
            screen
                .reject_global_for_tab_operation(global_id)
                .unwrap_err()
                .to_string(),
            "global pane has no tab",
            "{command}"
        );
        assert_eq!(
            (
                screen.global_pane_placement(global_id),
                screen.global_panes.is_hidden(global_id),
                screen
                    .with_pane(global_id, |pane| pane.current_title())
                    .unwrap(),
            ),
            snapshot,
            "{command} mutated the global pane"
        );
    }
}

#[test]
fn global_plugin_commands_use_the_global_first_resolver() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    let global_id = PaneId::Plugin(77);
    while plugin_rx.try_recv().is_ok() {}

    screen
        .focus_pane_with_id(global_id, false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(global_id));
    while plugin_rx.try_recv().is_ok() {}
    screen
        .write_to_pane_id(&None, b"x".to_vec(), false, global_id, Some(1), None)
        .unwrap();
    assert!(matches!(
        plugin_rx.recv().unwrap().0,
        PluginInstruction::Update(updates)
            if updates.iter().any(|(plugin_id, client_id, event)| {
                *plugin_id == Some(77)
                    && *client_id == Some(1)
                    && matches!(event, Event::Key(_))
            })
    ));

    screen
        .with_pane(global_id, |pane| pane.rename(b"renamed plugin".to_vec()))
        .unwrap();
    assert_eq!(
        screen
            .with_pane(global_id, |pane| pane.current_title())
            .unwrap(),
        "renamed plugin"
    );
    screen.hide_pane(global_id).unwrap();
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
    screen.show_pane(global_id).unwrap();

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Top,
        size: PercentOrFixed::Fixed(2),
    };
    screen
        .set_global_pane_placement(global_id, placement)
        .unwrap();
    assert_eq!(screen.global_pane_placement(global_id), Some(placement));
    assert_eq!(
        screen
            .reject_global_for_tab_operation(global_id)
            .unwrap_err()
            .to_string(),
        "global pane has no tab"
    );

    assert!(screen.close_global_pane(global_id, None).unwrap());
    assert!(screen.global_panes.pane(global_id).is_none());
}

#[test]
fn switching_tabs_retains_global_pane_focus() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    add_global_terminal(&mut screen, 42);

    screen
        .focus_pane_with_id(PaneId::Terminal(42), false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(42)));

    screen.switch_active_tab(0, None, true, 1).unwrap();

    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        Some(PaneId::Terminal(42))
    );
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(42)));
    // the destination tab still remembers its own focus for when the global pane is released
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_active_pane_id(1),
        Some(PaneId::Terminal(1))
    );
}

#[test]
fn global_focus_keeps_tab_focus_remembered_but_reports_the_tab_inactive() {
    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    let (pty_tx, pty_rx) = channels::unbounded::<(PtyInstruction, ErrorContext)>();
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);

    screen.render_to_clients().unwrap();
    while server_rx.try_recv().is_ok() {}
    while pty_rx.try_recv().is_ok() {}

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 1)), 1);
    screen.render_to_clients().unwrap();

    let focused_render = server_rx.recv().unwrap().0;
    let focused_cursor = take_snapshots_and_cursor_coordinates_from_render_events(
        [&focused_render].into_iter(),
        size,
    )[0]
    .0;

    assert_eq!(
        screen.tabs.get(&0).unwrap().get_active_pane_id(1),
        Some(PaneId::Terminal(1))
    );
    assert!(pty_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PtyInstruction::UpdateActivePane(None, 1))));

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(1, 1)), 1);
    screen.render_to_clients().unwrap();

    let tab_focused_render = server_rx.recv().unwrap().0;
    let tab_focused_cursor = take_snapshots_and_cursor_coordinates_from_render_events(
        [&tab_focused_render].into_iter(),
        size,
    )[0]
    .0;

    assert_eq!(
        screen.tabs.get(&0).unwrap().get_active_pane_id(1),
        Some(PaneId::Terminal(1))
    );
    assert!(pty_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PtyInstruction::UpdateActivePane(Some(PaneId::Terminal(1)), 1)
    )));
    assert_eq!(focused_cursor, Some((0, 7)));
    assert_eq!(tab_focused_cursor, Some((0, 0)));
}

#[test]
fn mouse_click_focuses_global_pane_and_center_click_restores_tab_focus() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 1)), 1);
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(42)));

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(1, 1)), 1);
    assert_eq!(screen.global_panes.focused_pane_id(1), None);
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
}

#[test]
fn global_terminal_mouse_uses_content_coordinates_and_ignores_frames() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .set_pane_frame_style(PaneFrameStyle::Full)
        .unwrap();
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    screen
        .global_panes
        .handle_pty_bytes(42, b"\x1b[?1000h\x1b[?1006h".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 3)), 1);
    assert!(matches!(
        pty_writer_rx.recv().unwrap().0,
        PtyWriteInstruction::Write(bytes, 42, None) if bytes == b"\x1b[<0;3;1M"
    ));

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 3)), 1);
    assert!(pty_writer_rx.try_recv().is_err());
}

#[test]
fn global_terminal_mouse_drag_uses_the_existing_selection_behavior() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(7, 6)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 6)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("select")
    );
    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))),
            _ => false,
        }));
}

#[test]
fn global_terminal_mouse_wheel_sends_cursor_keys_in_alternate_mode() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    screen
        .global_panes
        .handle_pty_bytes(42, b"\x1b[?1049h".to_vec());

    screen.handle_mouse_event(MouseEvent::new_scroll_up_event(Position::new(7, 1)), 1);

    let writes: Vec<_> = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, 42, None) => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(writes, vec![b"\x1b[A".to_vec(); 3]);
}

#[test]
fn global_plugin_mouse_and_paste_use_the_existing_plugin_events() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen
        .global_panes
        .set_pane_frame_style(PaneFrameStyle::Full)
        .unwrap();
    while plugin_rx.try_recv().is_ok() {}

    for event in [
        MouseEvent::new_left_press_event(Position::new(2, 2)),
        MouseEvent::new_left_motion_event(Position::new(3, 3)),
        MouseEvent::new_left_release_event(Position::new(3, 3)),
        MouseEvent::new_right_press_event(Position::new(2, 2)),
        MouseEvent::new_scroll_up_event(Position::new(2, 2)),
        MouseEvent::new_scroll_down_event(Position::new(2, 2)),
    ] {
        screen.handle_mouse_event(event, 1);
    }
    screen
        .paste_to_pane_id(b"global paste".to_vec(), PaneId::Plugin(77), Some(1), None)
        .unwrap();

    let events: Vec<_> = plugin_rx
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .filter_map(|(plugin_id, client_id, event)| {
                    (plugin_id == Some(77) && client_id == Some(1)).then_some(event)
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();
    assert!(
        events.contains(&Event::Mouse(Mouse::LeftClick(1, 1))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::Hold(2, 2))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::Release(2, 2))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::RightClick(1, 1))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::ScrollUp(3))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::ScrollDown(3))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::PastedText("global paste".to_owned())),
        "{events:?}"
    );
}

/// A plugin opts into text selection with `set_self_mouse_selection_support`, which arrives as
/// `ScreenInstruction::SetMouseSelectionSupport`. That used to search only the tabs, so a global
/// pane - which belongs to no tab - never got the opt-in and its drags stayed click events.
#[test]
fn global_plugin_pane_selection_copies_once_the_plugin_opts_in() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen
        .global_panes
        .handle_plugin_bytes(77, 1, b"abc".to_vec());

    // the lookup the `SetMouseSelectionSupport` handler does - `None` here means the instruction
    // gets parked in `pending_events_waiting_for_tab` and the opt-in is lost
    assert!(
        screen
            .with_pane(PaneId::Plugin(77), |pane| pane
                .set_mouse_selection_support(true))
            .is_some(),
        "the plugin's mouse selection opt-in must reach a global pane"
    );
    while plugin_rx.try_recv().is_ok() {}

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(0, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(0, 3)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(0, 3)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Plugin(77), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("abc")
    );
    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))),
            _ => false,
        }));
}

/// A left global pane reserves its own last column for the tab separator, so a drag to end-of-line
/// crosses it. That cell is "on the frame", and bailing out there used to drop the release and
/// leave the client's mouse captured by the global pane.
#[test]
fn global_terminal_selection_survives_a_release_on_the_frame() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal_on_edge(&mut screen, 42, GlobalPaneEdge::Left);
    screen
        .set_global_pane_placement(
            PaneId::Terminal(42),
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(11),
            },
        )
        .unwrap();
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());
    // 11 columns wide, the last of which is the separator the tab draws into
    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_content_columns())
            .unwrap(),
        10
    );

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(0, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(0, 6)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(0, 10)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("select me")
    );
    // and the client is no longer captured: a motion over the tab area is the tab's again
    assert_eq!(
        screen
            .global_panes
            .pane_id_for_mouse_event(&MouseEvent::new_left_motion_event(Position::new(5, 30)), 1),
        None
    );
}

/// A release outside the pane is clamped into the content box, the same way the tab path clamps it.
#[test]
fn global_terminal_selection_clamps_a_release_outside_the_pane() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"line one\r\nline two".to_vec());

    // the bottom pane starts at row 7 - drag up from its second row and release above it entirely
    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 4)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(2, 4)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some(" one\nline")
    );
}

/// A wheel event is also a `Press`, and used to reset the drag bookkeeping mid-selection.
#[test]
fn global_terminal_wheel_does_not_cancel_a_selection_drag() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_scroll_up_event(Position::new(7, 3)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(7, 9)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 9)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("select me")
    );
}

/// The `Copy` action reads the focused pane, which may be a global one.
#[test]
fn copy_action_copies_the_selection_of_a_focused_global_pane() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen.copy_options.copy_on_select = false;
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 6)), 1);
    while plugin_rx.try_recv().is_ok() {}

    screen.copy_selection(1).unwrap();

    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))),
            _ => false,
        }));
}

/// The status bar drops its "copied to clipboard" message on `InputReceived`, so the event that
/// caused the copy must not send one - every other one must.
#[test]
fn global_terminal_copy_leaves_the_clipboard_message_alone() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());
    // stand in for the status bar: something that wants to hear about this client's input
    screen
        .background_plugin_subscriptions
        .insert((99, 1), HashSet::from([EventType::InputReceived]));

    let input_received_since = |rx: &channels::Receiver<(PluginInstruction, ErrorContext)>| {
        rx.try_iter().any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::InputReceived)),
            _ => false,
        })
    };

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    while plugin_rx.try_recv().is_ok() {}
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 6)), 1);
    assert!(!input_received_since(&plugin_rx), "copy was announced away");

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    assert!(input_received_since(&plugin_rx));
}

#[test]
fn global_terminal_and_plugin_output_is_delivered_to_dock_panes() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/output/plugin", &None, None, None).unwrap();
    add_global_terminal(&mut screen, 42);
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen
        .global_panes
        .resize(screen.surface_size().into())
        .unwrap();

    assert!(screen
        .global_panes
        .handle_pty_bytes(42, b"terminal dock output".to_vec()));
    assert!(screen
        .global_panes
        .handle_plugin_bytes(77, 1, b"plugin dock output".to_vec()));

    let terminal_contents = screen
        .global_panes
        .pane(PaneId::Terminal(42))
        .unwrap()
        .pane_contents(None, false, None)
        .viewport
        .join("\n");
    let plugin_contents = screen
        .global_panes
        .pane(PaneId::Plugin(77))
        .unwrap()
        .pane_contents(Some(1), false, None)
        .viewport
        .join("\n");
    assert!(terminal_contents.contains("terminal dock output"));
    assert!(plugin_contents
        .replace('\n', "")
        .contains("plugin dock output"));
}

#[test]
fn global_process_exit_removes_the_dock_and_restores_tab_geometry() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    assert_eq!(screen.tabs.get(&0).unwrap().size.rows, 7);

    assert!(screen
        .close_global_pane(PaneId::Terminal(42), None)
        .unwrap());

    assert!(screen.global_panes.pane(PaneId::Terminal(42)).is_none());
    assert_eq!(screen.tabs.get(&0).unwrap().size.rows, 10);
}

#[test]
fn global_close_cleans_up_once_before_a_failing_reflow() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    let (pty_tx, pty_rx) = channels::unbounded::<(PtyInstruction, ErrorContext)>();
    screen.global_panes = GlobalPaneManager::new(
        ThreadSenders {
            to_pty_writer: Some(SenderWithContext::new(pty_writer_tx)),
            ..Default::default()
        },
        screen.character_cell_size.clone(),
        PaneFrameStyle::None,
    );
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    screen.bus.senders.should_silently_fail = false;
    add_global_terminal(&mut screen, 42);
    // a second pane, on a free edge and allocated after the first, so removing the first regrows it
    // and the reflow has a resize to send (which the dropped receiver below makes fail)
    add_global_terminal_on_edge(&mut screen, 43, zellij_utils::data::GlobalPaneEdge::Left);
    while pty_writer_rx.try_recv().is_ok() {}
    drop(pty_writer_rx);

    assert!(screen
        .close_global_pane(PaneId::Terminal(42), None)
        .is_err());

    assert!(screen.global_panes.pane(PaneId::Terminal(42)).is_none());
    assert_eq!(
        pty_rx
            .try_iter()
            .filter(|(instruction, _)| matches!(
                instruction,
                PtyInstruction::ClosePane(PaneId::Terminal(42), None)
            ))
            .count(),
        1
    );
}

#[test]
fn resurrection_layout_global_panes_launch_once_in_declaration_order_before_tab_panes() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let terminal_placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Bottom,
        size: PercentOrFixed::Fixed(3),
    };
    let plugin_placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(8),
    };
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/layout/plugin", &None, None, None).unwrap();
    mock_screen.default_layout.global_panes = vec![
        GlobalPaneLayout {
            run: Some(Run::Command(RunCommand {
                command: PathBuf::from("layout-command"),
                ..Default::default()
            })),
            placement: terminal_placement,
            borderless: true,
            focus: false,
            hidden: true,
        },
        GlobalPaneLayout {
            run: Some(Run::Plugin(run_plugin)),
            placement: plugin_placement,
            borderless: false,
            focus: true,
            hidden: false,
        },
    ];
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let pty_instructions: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::SpawnTerminal(..) | PtyInstruction::FillPluginCwd(..)
            )
        })
        .collect();
    let plugin_instructions: Vec<_> = plugin_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .filter(|instruction| {
            matches!(
                instruction,
                PluginInstruction::Load(..) | PluginInstruction::NewTab(..)
            )
        })
        .collect();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(pty_instructions.len(), 1);
    assert!(matches!(
        &pty_instructions[0],
        PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(_)),
            _,
            _,
            _,
            _,
            _,
            _,
            Some(properties),
        ) if properties.placement == terminal_placement
            && properties.borderless
            && properties.initially_hidden
            && !properties.should_focus
            && properties.declaration_order == Some(0)
    ));
    assert_eq!(plugin_instructions.len(), 3);
    assert!(matches!(
        &plugin_instructions[0],
        PluginInstruction::Load(
            _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, Some(properties)
        ) if properties.placement == plugin_placement
            && !properties.borderless
            && !properties.initially_hidden
            && properties.should_focus
            && properties.declaration_order == Some(1)
    ));
    assert!(matches!(
        plugin_instructions[1],
        PluginInstruction::NewTab(..)
    ));
    assert!(matches!(
        plugin_instructions[2],
        PluginInstruction::NewTab(..)
    ));
}

#[test]
fn global_plugin_launch_retains_placement_when_not_loaded() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(9),
    };
    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    route_arbitrary_action_to_server(
        &session_metadata,
        Action::LaunchOrFocusPlugin {
            plugin: RunPluginOrAlias::from_url("file:/path/to/global/plugin", &None, None, None)
                .unwrap(),
            should_float: false,
            move_to_focused_tab: false,
            should_open_in_place: false,
            close_replaced_pane: false,
            skip_cache: false,
            tab_id: None,
            global_placement: Some(placement),
        },
        10,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    assert!(received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .any(|instruction| matches!(
            instruction,
            PtyInstruction::FillPluginCwd(
                _, _, _, _, _, _, _, _, _, _, _, _, _, _, Some(actual)
            ) if actual.placement == placement
        )));
}

#[test]
fn global_hocus_sidebar_restore_reuses_the_hidden_plugin() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/hocus.wasm", &None, None, None).unwrap();
    let global_id = PaneId::Plugin(77);
    add_matching_global_plugin(&mut screen, 77, run_plugin.clone(), false);
    screen
        .focus_pane_with_id(global_id, false, false, 1)
        .unwrap();
    // move the focus back into a tab - switching tabs alone would keep it on the global pane
    screen
        .focus_pane_with_id(PaneId::Terminal(1), false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
    assert!(screen.global_panes.visible_pane_ids().contains(&global_id));

    screen.hide_pane(global_id).unwrap();
    assert!(screen.global_panes.is_hidden(global_id).unwrap());
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let mut completion = Some(NotificationEnd::new(completion_tx));

    assert!(screen
        .focus_global_plugin_pane(&run_plugin, 1, &mut completion)
        .unwrap());
    drop(completion);

    assert_eq!(screen.active_tab_ids[&1], 0);
    assert_eq!(screen.global_panes.plugin_ids(), vec![77]);
    assert!(screen.global_panes.visible_pane_ids().contains(&global_id));
    assert_eq!(screen.global_panes.focused_pane_id(1), Some(global_id));
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(global_id)
    );
    assert!(screen.tabs.values().all(|tab| tab
        .pane_infos()
        .iter()
        .all(|pane| !(pane.is_plugin && pane.id == 77))));
    assert!(screen.tabs.values().all(|tab| tab
        .get_floating_panes()
        .all(|(pane_id, _)| *pane_id != global_id)));
    assert!(!plugin_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PluginInstruction::Load(..))));
}

#[test]
fn global_plugin_launch_focuses_visible_match_without_moving_tabs_or_loading() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin.clone(), false);
    let active_tab = screen.active_tab_ids[&1];
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let mut completion = Some(NotificationEnd::new(completion_tx));

    assert!(screen
        .focus_global_plugin_pane(&run_plugin, 1, &mut completion)
        .unwrap());
    drop(completion);

    assert_eq!(screen.active_tab_ids[&1], active_tab);
    assert_eq!(screen.global_panes.plugin_ids(), vec![77]);
    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        Some(PaneId::Plugin(77))
    );
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(77))
    );
}

#[test]
fn global_plugin_launch_rejects_a_matching_tab_local_plugin() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/fake/plugin", &None, None, None).unwrap();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.run = Some(Run::Plugin(run_plugin.clone()));
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();

    mock_screen
        .to_screen
        .send(ScreenInstruction::LaunchOrFocusPlugin(
            run_plugin,
            false,
            false,
            false,
            false,
            None,
            false,
            1,
            Some(NotificationEnd::new(completion_tx)),
            None,
            Some(GlobalPanePlacement {
                edge: GlobalPaneEdge::Right,
                size: PercentOrFixed::Fixed(8),
            }),
        ))
        .unwrap();
    let completion = completion_rx.blocking_recv().unwrap();
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(completion.exit_status, Some(1));
    assert!(completion.error_message.unwrap().contains("tab-local pane"));
    assert!(!pty_receiver
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PtyInstruction::FillPluginCwd(..))));
}

#[test]
fn open_new_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(screen.tabs.len(), 2, "Screen now has two tabs");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab switched to new tab"
    );
}

#[test]
pub fn switch_to_prev_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen.switch_tab_prev(None, true, 1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab switched to previous tab"
    );
}

#[test]
pub fn switch_to_next_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen.switch_tab_prev(None, true, 1).expect("TEST");
    screen.switch_tab_next(None, true, 1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab switched to next tab"
    );
}

#[test]
pub fn switch_to_tab_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);

    assert_eq!(
        screen
            .switch_active_tab_name("Tab #1".to_string(), 1)
            .expect("TEST"),
        false,
        "Active tab switched to tab by name"
    );
    assert_eq!(
        screen
            .switch_active_tab_name("Tab #2".to_string(), 1)
            .expect("TEST"),
        true,
        "Active tab switched to tab by name"
    );
    assert_eq!(
        screen
            .switch_active_tab_name("Tab #3".to_string(), 1)
            .expect("TEST"),
        true,
        "Active tab switched to tab by name"
    );
}

#[test]
pub fn close_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen.close_tab(1).expect("TEST");

    assert_eq!(screen.tabs.len(), 1, "Only one tab left");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab switched to previous tab"
    );
}

#[test]
pub fn close_the_middle_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    new_tab(&mut screen, 3, 3);
    screen.switch_tab_prev(None, true, 1).expect("TEST");
    screen.close_tab(1).expect("TEST");

    assert_eq!(screen.tabs.len(), 2, "Two tabs left");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab switched to previous tab"
    );
}

#[test]
fn move_focus_left_at_left_screen_edge_changes_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    new_tab(&mut screen, 3, 3);
    screen.switch_tab_prev(None, true, 1).expect("TEST");
    screen.move_focus_left_or_previous_tab(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab switched to previous"
    );
}

#[test]
fn basic_move_of_active_tab_to_left() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    assert_eq!(screen.get_active_tab(1).unwrap().position, 1);

    screen.move_active_tab_to_left(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab moved to left"
    );
}

fn create_fixed_size_screen() -> Screen {
    create_new_screen(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
        true,
    )
}

#[test]
fn move_of_active_tab_to_left_when_there_is_only_one_tab() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    assert_eq!(screen.get_active_tab(1).unwrap().position, 0);

    screen.move_active_tab_to_left(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab moved to left"
    );
}

#[test]
fn move_of_active_tab_to_left_multiple_times() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    assert_eq!(screen.get_active_tab(1).unwrap().position, 2);

    screen.move_active_tab_to_left(1).expect("TEST");
    screen.move_active_tab_to_left(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab moved to left twice"
    );
}

#[test]
fn wrapping_move_of_active_tab_to_left() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    screen.move_focus_left_or_previous_tab(1).expect("TEST");
    screen.move_focus_left_or_previous_tab(1).expect("TEST");
    assert_eq!(screen.get_active_tab(1).unwrap().position, 0);

    screen.move_active_tab_to_left(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab moved to left until wrapped around"
    );
}

#[test]
fn basic_move_of_active_tab_to_right() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.move_focus_left_or_previous_tab(1).expect("TEST");
    assert_eq!(screen.get_active_tab(1).unwrap().position, 0);

    screen.move_active_tab_to_right(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab moved to right"
    );
}

#[test]
fn move_of_active_tab_to_right_when_there_is_only_one_tab() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    assert_eq!(screen.get_active_tab(1).unwrap().position, 0);

    screen.move_active_tab_to_right(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab moved to left"
    );
}

#[test]
fn move_of_active_tab_to_right_multiple_times() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    screen.move_focus_left_or_previous_tab(1).expect("TEST");
    screen.move_focus_left_or_previous_tab(1).expect("TEST");
    assert_eq!(screen.get_active_tab(1).unwrap().position, 0);

    screen.move_active_tab_to_right(1).expect("TEST");
    screen.move_active_tab_to_right(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab moved to right twice"
    );
}

#[test]
fn wrapping_move_of_active_tab_to_right() {
    let mut screen = create_fixed_size_screen();
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    assert_eq!(screen.get_active_tab(1).unwrap().position, 2);

    screen.move_active_tab_to_right(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab moved to right until wrapped around"
    );
}

#[test]
fn tab_id_remains_stable_after_switch() {
    // Test that tab IDs remain stable when switching tabs, only positions change
    let mut screen = create_fixed_size_screen();

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);

    // Verify initial state: IDs should be 0, 1, 2
    let initial_tab_ids: Vec<usize> = screen.tabs.keys().copied().collect();
    assert_eq!(
        initial_tab_ids,
        vec![0, 1, 2],
        "Initial tab IDs should be 0, 1, 2"
    );

    // Verify initial positions match IDs
    assert_eq!(screen.tabs.get(&0).unwrap().id, 0);
    assert_eq!(screen.tabs.get(&0).unwrap().position, 0);
    assert_eq!(screen.tabs.get(&1).unwrap().id, 1);
    assert_eq!(screen.tabs.get(&1).unwrap().position, 1);
    assert_eq!(screen.tabs.get(&2).unwrap().id, 2);
    assert_eq!(screen.tabs.get(&2).unwrap().position, 2);

    // Move active tab (position 2, ID 2) to right, which wraps to position 0 and rotates the
    // others right by one (IDs stay stable, only positions change)
    screen.move_active_tab_to_right(1).expect("TEST");

    // Verify BTreeMap keys (IDs) remain unchanged
    let after_switch_tab_ids: Vec<usize> = screen.tabs.keys().copied().collect();
    assert_eq!(
        after_switch_tab_ids,
        vec![0, 1, 2],
        "Tab IDs in BTreeMap should remain 0, 1, 2 after switch"
    );

    // Verify IDs remain stable but positions rotate
    // Tab 0: was at position 0, rotated to position 1
    assert_eq!(
        screen.tabs.get(&0).unwrap().id,
        0,
        "Tab with ID 0 should still have ID 0"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().position,
        1,
        "Tab with ID 0 should now be at position 1"
    );

    // Tab 1: was at position 1, rotated to position 2
    assert_eq!(
        screen.tabs.get(&1).unwrap().id,
        1,
        "Tab with ID 1 should still have ID 1"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().position,
        2,
        "Tab with ID 1 should now be at position 2"
    );

    // Tab 2: was at position 2, rotated over the edge to position 0
    assert_eq!(
        screen.tabs.get(&2).unwrap().id,
        2,
        "Tab with ID 2 should still have ID 2"
    );
    assert_eq!(
        screen.tabs.get(&2).unwrap().position,
        0,
        "Tab with ID 2 should now be at position 0"
    );

    // Verify that lookup by position works correctly after the rotation
    let tab_at_pos_0 = screen.tabs.values().find(|t| t.position == 0).unwrap();
    assert_eq!(tab_at_pos_0.id, 2, "Tab at position 0 should have ID 2");

    let tab_at_pos_1 = screen.tabs.values().find(|t| t.position == 1).unwrap();
    assert_eq!(tab_at_pos_1.id, 0, "Tab at position 1 should have ID 0");

    let tab_at_pos_2 = screen.tabs.values().find(|t| t.position == 2).unwrap();
    assert_eq!(tab_at_pos_2.id, 1, "Tab at position 2 should have ID 1");
}

#[test]
fn move_focus_right_at_right_screen_edge_changes_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    new_tab(&mut screen, 3, 3);
    screen.switch_tab_prev(None, true, 1).expect("TEST");
    screen.move_focus_right_or_next_tab(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab switched to next"
    );
}

#[test]
pub fn toggle_to_previous_tab_simple() {
    let position_and_size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(position_and_size, true, true);

    new_tab(&mut screen, 1, 1);
    new_tab(&mut screen, 2, 2);
    screen.go_to_tab(1, 1).expect("TEST");
    screen.go_to_tab(2, 1).expect("TEST");

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );
}

#[test]
pub fn toggle_to_previous_tab_create_tabs_only() {
    let position_and_size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(position_and_size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);

    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 1],
        "Tab history is invalid"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 2],
        "Tab history is invalid"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 1],
        "Tab history is invalid"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );
}

#[test]
pub fn toggle_to_previous_tab_delete() {
    let position_and_size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(position_and_size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    new_tab(&mut screen, 4, 3);

    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 1, 2],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        3,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 1, 3],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 1, 2],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        3,
        "Active tab toggler to previous tab"
    );

    screen.switch_tab_prev(None, true, 1).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 1, 3],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );
    screen.switch_tab_prev(None, true, 1).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 3, 2],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );

    screen.close_tab(1).expect("TEST");
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 3],
        "Tab history is invalid"
    );
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        1,
        "Active tab toggler to previous tab"
    );

    screen.toggle_tab(1).expect("TEST");
    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        2,
        "Active tab toggler to previous tab"
    );
    assert_eq!(
        screen.tab_history.get(&1).unwrap(),
        &[0, 2],
        "Tab history is invalid"
    );
}

#[test]
fn switch_to_tab_with_fullscreen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 1);
    {
        let active_tab = screen.get_active_tab_mut(1).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(2),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(1),
                None,
            )
            .unwrap();
        active_tab.toggle_active_pane_fullscreen(1);
    }
    new_tab(&mut screen, 2, 2);

    screen.switch_tab_prev(None, true, 1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(1).unwrap().position,
        0,
        "Active tab switched to previous"
    );
    assert_eq!(
        screen
            .get_active_tab(1)
            .unwrap()
            .get_active_pane_id(1)
            .unwrap(),
        PaneId::Terminal(2),
        "Active pane is still the fullscreen pane"
    );
}

#[test]
fn update_screen_pixel_dimensions() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let initial_pixel_dimensions = screen.pixel_dimensions;
    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5,
            }),
            text_area_size: None,
        },
    );
    let pixel_dimensions_after_first_update = screen.pixel_dimensions;
    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                height: 100,
                width: 50,
            }),
        },
    );
    let pixel_dimensions_after_second_update = screen.pixel_dimensions;
    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: None,
        },
    );
    let pixel_dimensions_after_third_update = screen.pixel_dimensions;
    assert_eq!(
        initial_pixel_dimensions,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: None
        },
        "Initial pixel dimensions empty"
    );
    assert_eq!(
        pixel_dimensions_after_first_update,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5
            }),
            text_area_size: None
        },
        "character_cell_size updated properly",
    );
    assert_eq!(
        pixel_dimensions_after_second_update,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5
            }),
            text_area_size: Some(SizeInPixels {
                height: 100,
                width: 50,
            }),
        },
        "text_area_size updated properly without overriding character_cell_size",
    );
    assert_eq!(
        pixel_dimensions_after_third_update,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 10,
                width: 5
            }),
            text_area_size: Some(SizeInPixels {
                height: 100,
                width: 50,
            }),
        },
        "empty update does not delete existing data",
    );
}

#[test]
fn character_cell_size_is_derived_from_the_reporting_client_size() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(size, true, true);
    let client_id = 2;
    screen.set_client_size(
        client_id,
        Size {
            cols: 100,
            rows: 50,
        },
    );

    screen.update_pixel_dimensions(
        client_id,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                height: 1050,
                width: 900,
            }),
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            height: 21,
            width: 9
        }),
        "The reported text area is divided by the grid size of the client that reported it"
    );
}

#[test]
fn character_cell_size_is_not_derived_for_a_client_of_unknown_size() {
    let size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(size, true, true);

    screen.update_pixel_dimensions(
        2,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                height: 1050,
                width: 900,
            }),
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        None,
        "Without the reporting client's grid size the pixel report cannot be interpreted"
    );
}

#[test]
fn attach_after_first_tab_closed() {
    // ensure https://github.com/zellij-org/zellij/issues/1645 is fixed
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    {
        let active_tab = screen.get_active_tab_mut(1).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(2),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(1),
                None,
            )
            .unwrap();
        active_tab.toggle_active_pane_fullscreen(1);
    }
    new_tab(&mut screen, 2, 1);

    screen.close_tab_by_id(0).expect("TEST");
    screen.remove_client(1).expect("TEST");
    screen.add_client(1, false).expect("TEST");
}

#[test]
fn open_new_floating_pane_with_custom_coordinates() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Percent(10)),
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Percent(1)),
                height: Some(PercentOrFixed::Fixed(2)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(active_pane.x(), 12, "x coordinates set properly");
    assert_eq!(active_pane.y(), 5, "y coordinates set properly");
    assert_eq!(active_pane.rows(), 2, "rows set properly");
    assert_eq!(active_pane.cols(), 1, "columns set properly");
}

#[test]
fn open_new_floating_pane_with_custom_coordinates_exceeding_viewport() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(122)),
                y: Some(PercentOrFixed::Fixed(21)),
                width: Some(PercentOrFixed::Fixed(10)),
                height: Some(PercentOrFixed::Fixed(10)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(active_pane.x(), 111, "x coordinates set properly");
    assert_eq!(active_pane.y(), 10, "y coordinates set properly");
    assert_eq!(active_pane.rows(), 10, "rows set properly");
    assert_eq!(active_pane.cols(), 10, "columns set properly");
}

#[test]
fn floating_pane_auto_centers_horizontally_with_only_width() {
    let size = Size {
        cols: 120,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Fixed(60)),
                height: Some(PercentOrFixed::Fixed(10)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(active_pane.x(), 30, "x centered: (120-60)/2 = 30");
    assert_eq!(active_pane.y(), 5, "y explicitly set");
    assert_eq!(active_pane.cols(), 60, "width set");
    assert_eq!(active_pane.rows(), 10, "height set");
}

#[test]
fn floating_pane_auto_centers_vertically_with_only_height() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: None,
                width: Some(PercentOrFixed::Fixed(50)),
                height: Some(PercentOrFixed::Fixed(20)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(active_pane.x(), 10, "x explicitly set");
    assert_eq!(active_pane.y(), 10, "y centered: (40-20)/2 = 10");
    assert_eq!(active_pane.cols(), 50, "width set");
    assert_eq!(active_pane.rows(), 20, "height set");
}

#[test]
fn floating_pane_auto_centers_both_axes_with_only_size() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: None,
                width: Some(PercentOrFixed::Fixed(80)),
                height: Some(PercentOrFixed::Fixed(30)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(active_pane.x(), 20, "x centered: (120-80)/2 = 20");
    assert_eq!(active_pane.y(), 5, "y centered: (40-30)/2 = 5");
    assert_eq!(active_pane.cols(), 80, "width set");
    assert_eq!(active_pane.rows(), 30, "height set");
}

#[test]
fn floating_pane_respects_explicit_coordinates_with_size() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(15)),
                y: Some(PercentOrFixed::Fixed(8)),
                width: Some(PercentOrFixed::Fixed(80)),
                height: Some(PercentOrFixed::Fixed(30)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(active_pane.x(), 15, "x explicitly set, not centered");
    assert_eq!(active_pane.y(), 8, "y explicitly set, not centered");
    assert_eq!(active_pane.cols(), 80, "width set");
    assert_eq!(active_pane.rows(), 30, "height set");
}

#[test]
fn floating_pane_centers_with_percentage_width() {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: Some(PercentOrFixed::Fixed(5)),
                width: Some(PercentOrFixed::Percent(50)),
                height: Some(PercentOrFixed::Fixed(20)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    let expected_width = ((50.0_f64 / 100.0) * 120.0).floor() as usize;
    let expected_x = (120 - expected_width) / 2;
    assert_eq!(active_pane.cols(), expected_width, "width is 50% of 120");
    assert_eq!(
        active_pane.x(),
        expected_x,
        "x centered based on calculated width"
    );
    assert_eq!(active_pane.y(), 5, "y explicitly set");
}

#[test]
fn floating_pane_centers_large_pane_safely() {
    let size = Size {
        cols: 100,
        rows: 30,
    };
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Floating(Some(FloatingPaneCoordinates {
                x: None,
                y: None,
                width: Some(PercentOrFixed::Fixed(150)),
                height: Some(PercentOrFixed::Fixed(50)),
                pinned: None,
                borderless: Some(false),
            })),
            Some(1),
            None,
        )
        .unwrap();
    let active_pane = active_tab.get_active_pane(1).unwrap();
    assert_eq!(
        active_pane.x(),
        0,
        "x is 0 when pane larger than viewport (saturating_sub)"
    );
    assert_eq!(
        active_pane.y(),
        0,
        "y is 0 when pane larger than viewport (saturating_sub)"
    );
    assert!(active_pane.cols() <= 100, "width clamped to viewport");
    assert!(active_pane.rows() <= 30, "height clamped to viewport");
}

#[test]
pub fn mouse_hover_effect() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = 1;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let hover_mouse_event_1 = MouseEvent::new_buttonless_motion(Position::new(5, 70));
    let _ = mock_screen.to_screen.send(ScreenInstruction::MouseEvent(
        hover_mouse_event_1,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
}

#[test]
pub fn disabled_mouse_hover_effect() {
    let size = Size {
        cols: 130,
        rows: 20,
    };
    let client_id = 1;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.set_advanced_hover_effects(false);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let hover_mouse_event_1 = MouseEvent::new_buttonless_motion(Position::new(5, 70));
    let _ = mock_screen.to_screen.send(ScreenInstruction::MouseEvent(
        hover_mouse_event_1,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
}

#[test]
fn group_panes_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![PaneId::Terminal(2)]),
        "Pane Id added to client's pane group"
    );

    screen.handle_mouse_event(
        MouseEvent::new_left_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
}

#[test]
fn mouse_focus_clears_bell_on_focused_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);

    let new_pane_id = PaneId::Terminal(2);
    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        // Split horizontally: pane 1 on top, pane 2 on bottom; focus moves to pane 2
        active_tab
            .horizontal_split(new_pane_id, None, client_id, None, None)
            .unwrap();
        // Move focus back up to pane 1 so pane 2 is unfocused
        active_tab.move_focus_up(client_id).unwrap();
        // Plant a bell on the unfocused pane 2
        active_tab.handle_pty_bytes(2, vec![7u8]).unwrap();
        active_tab.check_and_handle_bell_notifications(false);
        assert!(
            active_tab.panes_with_pending_bell.contains(&new_pane_id),
            "Pane 2 should have a pending bell before the click"
        );
        assert!(
            active_tab.tab_has_pending_bell,
            "Tab should have a pending bell before the click"
        );
    }

    // Click somewhere in pane 2 (bottom half of the screen)
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(15, 60)),
        client_id,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(15, 60)),
        client_id,
    );

    let active_tab = screen.get_active_tab(client_id).unwrap();
    assert_eq!(
        active_tab.get_active_pane_id(client_id),
        Some(new_pane_id),
        "Mouse click should have focused pane 2"
    );
    assert!(
        !active_tab.panes_with_pending_bell.contains(&new_pane_id),
        "Bell on pane 2 should be cleared after mouse focus"
    );
    assert!(
        !active_tab.tab_has_pending_bell,
        "Tab bell should be cleared after the last pane bell is cleared"
    );
}

#[test]
fn nested_guest_fullscreen_moves_from_one_pane_to_another() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    let pane_one = PaneId::Terminal(1);
    let pane_two = PaneId::Terminal(2);
    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        active_tab
            .horizontal_split(pane_two, None, client_id, None, None)
            .unwrap();
    }

    screen.apply_nested_guest_fullscreen(pane_one, true);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            Some(pane_one),
            "the first guest pane is fullscreen"
        );
        assert!(active_tab.fullscreen_covers_ui());
    }
    assert_eq!(
        screen.nested_fullscreen_panes,
        [pane_one].into_iter().collect(),
        "only the first guest pane is tracked as fullscreen"
    );

    screen.apply_nested_guest_fullscreen(pane_two, true);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            Some(pane_two),
            "fullscreen moved to the second guest pane"
        );
        assert!(active_tab.fullscreen_covers_ui());
    }
    assert_eq!(
        screen.nested_fullscreen_panes,
        [pane_two].into_iter().collect(),
        "the first guest pane is no longer tracked as fullscreen, only the second is"
    );

    screen.apply_nested_guest_fullscreen(pane_two, false);
    {
        let active_tab = screen.get_active_tab(client_id).unwrap();
        assert_eq!(
            active_tab.fullscreen_pane_id(),
            None,
            "unsetting fullscreen restores the tiled layout"
        );
    }
    assert!(
        screen.nested_fullscreen_panes.is_empty(),
        "no guest panes are tracked as fullscreen after unsetting"
    );
}

#[test]
fn group_panes_with_keyboard() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    let _ = screen.toggle_pane_in_group(client_id);

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![PaneId::Terminal(2)]),
        "Pane Id added to client's pane group"
    );

    let _ = screen.toggle_pane_in_group(client_id);

    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Pane Id removed from client's pane group"
    );
}

#[test]
fn group_panes_following_focus() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);

    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        for i in 2..5 {
            active_tab
                .new_pane(
                    PaneId::Terminal(i),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    },
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
    }
    {
        screen.toggle_group_marking(client_id).unwrap();
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(
            screen
                .current_pane_group
                .borrow()
                .clone_inner()
                .get(&client_id),
            Some(&vec![PaneId::Terminal(4), PaneId::Terminal(3)]),
            "Pane Id of focused pane and newly focused pane above added to pane group"
        );
    }
    {
        let _ = screen.toggle_group_marking(client_id);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        let _ = screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(screen.current_pane_group.borrow().clone_inner().get(&client_id), Some(&vec![PaneId::Terminal(4), PaneId::Terminal(3)]), "Pane Id of newly focused pane not added to group after the group marking was toggled off");
    }
}

#[test]
fn break_group_with_mouse() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    new_tab(&mut screen, 1, 0);

    {
        let active_tab = screen.get_active_tab_mut(client_id).unwrap();
        for i in 2..5 {
            active_tab
                .new_pane(
                    PaneId::Terminal(i),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Tiled {
                        direction: None,
                        borderless: None,
                    },
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
    }
    {
        screen.toggle_group_marking(client_id).unwrap();
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .move_focus_up(client_id)
            .unwrap();
        screen.add_active_pane_to_group_if_marking(&client_id);
        assert_eq!(
            screen
                .current_pane_group
                .borrow()
                .clone_inner()
                .get(&client_id),
            Some(&vec![
                PaneId::Terminal(4),
                PaneId::Terminal(3),
                PaneId::Terminal(2)
            ]),
            "Group contains 3 panes"
        );
    }

    screen.handle_mouse_event(
        MouseEvent::new_right_press_with_alt_event(Position::new(2, 80)),
        client_id,
    );
    assert_eq!(
        screen
            .current_pane_group
            .borrow()
            .clone_inner()
            .get(&client_id),
        Some(&vec![]),
        "Group cleared by mouse event"
    );
}

// Following are tests for sending CLI actions
// these tests are only partially relevant to Screen
// and are included here for two reasons:
// 1. The best way to "integration test" these is combining the "screen_thread_main" and
//    "route_action" functions and mocking everything around them
// 2. These inadvertently also test many parts of Screen that are not tested elsewhere

#[test]
pub fn send_cli_write_chars_action_to_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::WriteChars {
        chars: "input from the cli".into(),
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_write_action_to_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::Write {
        bytes: vec![102, 111, 111],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_send_keys_action_to_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl a".to_string(), "x".to_string()],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    // here we assert only the write instructions to make sure they arrived properly and in
    // sequence to the pane
    assert_snapshot!(format!("{:#?}", received_write_instructions));
}

#[test]
pub fn send_cli_ctrl_c_reaches_pty_writer() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl c".to_string()],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    assert_snapshot!(format!("{:#?}", received_write_instructions));
}

#[test]
pub fn send_cli_ctrl_w_reaches_pty_writer() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_action = CliAction::SendKeys {
        keys: vec!["Ctrl w".to_string()],
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    assert_snapshot!(format!("{:#?}", received_write_instructions));
}

#[test]
pub fn send_cli_resize_action_to_screen() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let pty_writer_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let resize_cli_action = CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Left),
        pane_id: None,
    };
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, resize_cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_focus_next_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let focus_next_pane_action = CliAction::FocusNextPane;
    send_cli_action_to_server(&session_metadata, focus_next_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_focus_previous_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let focus_next_pane_action = CliAction::FocusPreviousPane;
    send_cli_action_to_server(&session_metadata, focus_next_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_focus_last_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![
        TiledPaneLayout::default(),
        TiledPaneLayout::default(),
        TiledPaneLayout::default(),
    ];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let move_focus_action = CliAction::MoveFocus {
        direction: Direction::Right,
    };
    let focus_last_pane_action = CliAction::FocusLastPane;
    // move focus 1 -> 2 -> 3
    send_cli_action_to_server(&session_metadata, move_focus_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, move_focus_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
                                                               // move focus 3 -> 2
    send_cli_action_to_server(&session_metadata, focus_last_pane_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
                                                               // move focus 2 -> 3
    send_cli_action_to_server(&session_metadata, focus_last_pane_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_move_focus_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let move_focus_action = CliAction::MoveFocus {
        direction: Direction::Right,
    };
    send_cli_action_to_server(&session_metadata, move_focus_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_move_focus_or_tab_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let move_focus_action = CliAction::MoveFocusOrTab {
        direction: Direction::Right,
    };
    send_cli_action_to_server(&session_metadata, move_focus_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (cursor_coordinates, _snapshot) in snapshots {
        // here we assert he cursor_coordinates to let us know if we switched the pane focus
        assert_snapshot!(format!("{:?}", cursor_coordinates));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_move_pane_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::MovePane {
        direction: Some(Direction::Right),
        pane_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_dump_screen_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/foo")),
        full: true,
        pane_id: None,
        ansi: false,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "fill pane up with something".as_bytes().to_vec(),
    ));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert_snapshot!(format!(
        "{:?}",
        *mock_screen.os_input.fake_filesystem.lock().unwrap()
    ));
}

#[test]
pub fn send_cli_edit_scrollback_action() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::EditScrollback {
        pane_id: None,
        ansi: false,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "fill pane up with something".as_bytes().to_vec(),
    ));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let dumped_file_name = mock_screen
        .os_input
        .fake_filesystem
        .lock()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let mut found_instruction = false;
    for instruction in received_pty_instructions.lock().unwrap().iter() {
        if let PtyInstruction::OpenInPlaceEditor(
            scrollback_contents_file,
            terminal_id,
            client_id,
            _,
        ) = instruction
        {
            assert_eq!(scrollback_contents_file, &PathBuf::from(&dumped_file_name));
            assert_eq!(terminal_id, &Some(1));
            assert_eq!(client_id, &ClientTabIndexOrPaneId::ClientId(1));
            found_instruction = true;
        }
    }
    assert!(found_instruction);
}

#[test]
pub fn send_cli_scroll_up_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::ScrollUp { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send two actions here because only the last line in the pane is empty, so one action
    // won't show in a render
    send_cli_action_to_server(&session_metadata, cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, cli_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_scroll_down_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let scroll_up_cli_action = CliAction::ScrollUp { pane_id: None };
    let scroll_down_cli_action = CliAction::ScrollDown { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // scroll up some
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);

    // scroll down some
    send_cli_action_to_server(&session_metadata, scroll_down_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_down_cli_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let (_cursor_position, last_snapshot) = snapshots.last().unwrap();
    assert_snapshot!(format!("{}", last_snapshot));
}

#[test]
pub fn send_cli_scroll_to_bottom_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let scroll_up_cli_action = CliAction::ScrollUp { pane_id: None };
    let scroll_to_bottom_action = CliAction::ScrollToBottom { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // scroll up some
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);
    send_cli_action_to_server(&session_metadata, scroll_up_cli_action.clone(), client_id);

    // scroll to bottom
    send_cli_action_to_server(
        &session_metadata,
        scroll_to_bottom_action.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let (_cursor_position, last_snapshot) = snapshots.last().unwrap();
    assert_snapshot!(format!("{}", last_snapshot));
}

#[test]
pub fn send_cli_scroll_to_top_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let scroll_to_top_action = CliAction::ScrollToTop { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // scroll to top
    send_cli_action_to_server(&session_metadata, scroll_to_top_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_page_scroll_up_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let page_scroll_up_action = CliAction::PageScrollUp { pane_id: None };
    let mut pane_contents = String::new();
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, page_scroll_up_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_page_scroll_down_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let page_scroll_up_action = CliAction::PageScrollUp { pane_id: None };
    let page_scroll_down_action = CliAction::PageScrollDown { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // scroll up some
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, page_scroll_up_action.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, page_scroll_up_action.clone(), client_id);

    // scroll down
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(&session_metadata, page_scroll_down_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_half_page_scroll_up_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let half_page_scroll_up_action = CliAction::HalfPageScrollUp { pane_id: None };
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, half_page_scroll_up_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_half_page_scroll_down_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let half_page_scroll_up_action = CliAction::HalfPageScrollUp { pane_id: None };
    let half_page_scroll_down_action = CliAction::HalfPageScrollDown { pane_id: None };
    let mut pane_contents = String::new();
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // scroll up some
    send_cli_action_to_server(
        &session_metadata,
        half_page_scroll_up_action.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    send_cli_action_to_server(
        &session_metadata,
        half_page_scroll_up_action.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render

    // scroll down
    send_cli_action_to_server(&session_metadata, half_page_scroll_down_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_toggle_full_screen_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_full_screen_action = CliAction::ToggleFullscreen { pane_id: None };
    send_cli_action_to_server(&session_metadata, toggle_full_screen_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_toggle_pane_frames_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_pane_frames_action = CliAction::TogglePaneFrames;
    send_cli_action_to_server(&session_metadata, toggle_pane_frames_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_toggle_active_tab_sync_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let cli_toggle_active_tab_sync_action = CliAction::ToggleActiveSyncTab { tab_id: None };
    let cli_write_action = CliAction::Write {
        bytes: vec![102, 111, 111],
        pane_id: None,
    };
    send_cli_action_to_server(
        &session_metadata,
        cli_toggle_active_tab_sync_action,
        client_id,
    );
    send_cli_action_to_server(&session_metadata, cli_write_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let received_write_instructions: Vec<_> = received_pty_instructions
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .filter(|i| matches!(i, PtyWriteInstruction::Write(..)))
        .collect();
    // here we should have 2 Write instructions, one for each pane
    assert_snapshot!(format!("{:?}", received_write_instructions));
}

#[test]
pub fn send_cli_new_pane_action_with_default_parameters() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: None,
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn web_new_pane_in_tab_action_targets_requested_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );

    // This is exactly the Action the web control bridge produces for the
    // browser `NewPaneInTab { tab_id }` payload.
    let action = Action::NewTiledPane {
        direction: None,
        command: None,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(0),
        global_placement: None,
    };
    route_arbitrary_action_to_server(&session_metadata, action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawned_with_tab_index =
        received_pty_instructions
            .lock()
            .unwrap()
            .iter()
            .any(|i| match i {
                PtyInstruction::SpawnTerminal(_, _, _, _, client_tab_index_or_pane_id, ..) => {
                    matches!(
                        client_tab_index_or_pane_id,
                        ClientTabIndexOrPaneId::TabIndex(0)
                    )
                },
                _ => false,
            });
    assert!(
        spawned_with_tab_index,
        "NewPaneInTab must spawn a terminal targeting the requested tab index; got {:?}",
        *received_pty_instructions.lock().unwrap()
    );
}

#[test]
pub fn send_cli_new_pane_action_with_split_direction() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: Some(Direction::Right),
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_new_pane_action_with_command_and_cwd() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: Some(Direction::Right),
        command: vec!["htop".into()],
        plugin: None,
        cwd: Some("/some/folder".into()),
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let new_pane_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PtyInstruction::SpawnTerminal(..) => true,
            _ => false,
        })
        .cloned();

    assert_snapshot!(format!("{:?}", new_pane_instruction));
}

#[test]
pub fn send_cli_new_pane_action_with_floating_pane_and_coordinates() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: Some(Direction::Right),
        command: vec!["htop".into()],
        plugin: None,
        cwd: Some("/some/folder".into()),
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: Some("10".to_owned()),
        y: None,
        width: Some("20%".to_owned()),
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_edit_action_with_default_parameters() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/file/to/edit"),
        direction: None,
        line_number: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        borderless: Some(false),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_edit_action_with_line_number() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/file/to/edit"),
        direction: None,
        line_number: Some(100),
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        borderless: Some(false),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_edit_action_with_split_direction() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/file/to/edit"),
        direction: Some(Direction::Down),
        line_number: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        borderless: Some(false),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    assert_snapshot!(format!("{:?}", *received_pty_instructions.lock().unwrap()));
}

#[test]
pub fn send_cli_switch_mode_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let cli_switch_mode = CliAction::SwitchMode {
        input_mode: InputMode::Locked,
    };
    send_cli_action_to_server(&session_metadata, cli_switch_mode, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);

    let switch_mode_action = received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            ServerInstruction::ChangeMode(..) => true,
            _ => false,
        })
        .cloned();

    assert_snapshot!(format!("{:?}", switch_mode_action));
}

#[test]
pub fn send_cli_toggle_pane_embed_or_float() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_pane_embed_or_floating = CliAction::TogglePaneEmbedOrFloating { pane_id: None };
    // first time to float
    send_cli_action_to_server(
        &session_metadata,
        toggle_pane_embed_or_floating.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    // second time to embed
    send_cli_action_to_server(
        &session_metadata,
        toggle_pane_embed_or_floating.clone(),
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let _snapshot_count = snapshots.len();
    let last_three_snapshots = snapshots.clone().into_iter().rev().take(3).rev(); // we do this to
                                                                                  // prevent extra
                                                                                  // renders from
                                                                                  // throwing us
                                                                                  // off
    for (_cursor_coordinates, snapshot) in last_three_snapshots.clone() {
        eprintln!("{}", snapshot);
    }
    for (_cursor_coordinates, snapshot) in last_three_snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
}

#[test]
pub fn send_cli_toggle_floating_panes() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let toggle_pane_embed_or_floating = CliAction::TogglePaneEmbedOrFloating { pane_id: None };
    let toggle_floating_panes = CliAction::ToggleFloatingPanes { tab_id: None };
    // float the focused pane
    send_cli_action_to_server(&session_metadata, toggle_pane_embed_or_floating, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // toggle floating panes (will hide the floated pane from the previous action)
    send_cli_action_to_server(&session_metadata, toggle_floating_panes.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // toggle floating panes (will show the floated pane)
    send_cli_action_to_server(&session_metadata, toggle_floating_panes.clone(), client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_close_pane_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_instruction = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let close_pane_action = CliAction::ClosePane { pane_id: None };
    send_cli_action_to_server(&session_metadata, close_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_instruction, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_new_tab_action_default_params() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let new_tab_action = CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: None,
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: Default::default(),
        start_suspended: Default::default(),
        block_until_exit: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        no_focus: false,
    };
    send_cli_action_to_server(&session_metadata, new_tab_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let received_plugin_instructions = received_plugin_instructions.lock().unwrap();
    let new_tab_action =
        received_plugin_instructions
            .iter()
            .find(|instruction| match instruction {
                PluginInstruction::NewTab(..) => true,
                _ => false,
            });
    assert_snapshot!(format!("{:#?}", new_tab_action));
}

#[test]
pub fn send_cli_new_tab_action_with_name_and_layout() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let new_tab_action = CliAction::NewTab {
        name: Some("my-awesome-tab-name".into()),
        layout: Some(PathBuf::from(format!(
            "{}/src/unit/fixtures/layout-with-three-panes.kdl",
            env!("CARGO_MANIFEST_DIR")
        ))),
        layout_string: None,
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: Default::default(),
        start_suspended: Default::default(),
        block_until_exit: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        no_focus: false,
    };
    send_cli_action_to_server(&session_metadata, new_tab_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let new_tab_instruction = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|i| {
            if let PluginInstruction::NewTab(..) = i {
                return true;
            } else {
                return false;
            }
        })
        .unwrap()
        .clone();
    let output = format!("{:#?}", new_tab_instruction);
    assert_snapshot!(output);
}

#[test]
pub fn send_cli_next_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut second_tab_layout = TiledPaneLayout::default();
    second_tab_layout.children_split_direction = SplitDirection::Horizontal;
    second_tab_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let goto_next_tab = CliAction::GoToNextTab;
    send_cli_action_to_server(&session_metadata, goto_next_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    mock_screen.teardown(vec![server_thread, screen_thread]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_previous_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut second_tab_layout = TiledPaneLayout::default();
    second_tab_layout.children_split_direction = SplitDirection::Horizontal;
    second_tab_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let goto_previous_tab = CliAction::GoToPreviousTab;
    send_cli_action_to_server(&session_metadata, goto_previous_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_goto_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut second_tab_layout = TiledPaneLayout::default();
    second_tab_layout.children_split_direction = SplitDirection::Horizontal;
    second_tab_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let goto_tab = CliAction::GoToTab { index: 1 };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_close_tab_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut second_tab_layout = TiledPaneLayout::default();
    second_tab_layout.children_split_direction = SplitDirection::Horizontal;
    second_tab_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let close_tab = CliAction::CloseTab { tab_id: None };
    send_cli_action_to_server(&session_metadata, close_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_rename_tab() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut second_tab_layout = TiledPaneLayout::default();
    second_tab_layout.children_split_direction = SplitDirection::Horizontal;
    second_tab_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let rename_tab = CliAction::RenameTab {
        name: "new-tab-name".into(),
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, rename_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let plugin_rename_tab_instruction = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PluginInstruction::Update(updates) => updates
                .iter()
                .find(|u| match u {
                    (_, _, Event::TabUpdate(..)) => true,
                    _ => false,
                })
                .is_some(),
            _ => false,
        })
        .cloned();
    assert_snapshot!(format!("{:#?}", plugin_rename_tab_instruction))
}

#[test]
pub fn send_cli_undo_rename_tab() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut second_tab_layout = TiledPaneLayout::default();
    second_tab_layout.children_split_direction = SplitDirection::Horizontal;
    second_tab_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(second_tab_layout);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let rename_tab = CliAction::RenameTab {
        name: "new-tab-name".into(),
        tab_id: None,
    };
    let undo_rename_tab = CliAction::UndoRenameTab { tab_id: None };
    // first rename the tab
    send_cli_action_to_server(&session_metadata, rename_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    // then undo the tab rename to go back to the default name
    send_cli_action_to_server(&session_metadata, undo_rename_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let plugin_undo_rename_tab_instruction = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PluginInstruction::Update(updates) => updates
                .iter()
                .find(|u| match u {
                    (_, _, Event::TabUpdate(..)) => true,
                    _ => false,
                })
                .is_some(),
            _ => false,
        })
        .cloned();
    assert_snapshot!(format!("{:#?}", plugin_undo_rename_tab_instruction))
}

#[test]
pub fn send_cli_query_tab_names_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab(TiledPaneLayout::default());
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let query_tab_names = CliAction::QueryTabNames;
    send_cli_action_to_server(&session_metadata, query_tab_names, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let log_tab_names_instruction = received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            ServerInstruction::Log(..) => true,
            _ => false,
        })
        .cloned();
    assert_snapshot!(format!("{:#?}", log_tab_names_instruction));
}

#[test]
pub fn send_cli_launch_or_focus_plugin_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: true,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        skip_plugin_cache: false,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let pty_fill_plugin_cwd_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PtyInstruction::FillPluginCwd(..) => true,
            _ => false,
        })
        .cloned();

    assert_snapshot!(format!("{:#?}", pty_fill_plugin_cwd_instruction));
}

#[test]
pub fn send_cli_launch_or_focus_plugin_action_when_plugin_is_already_loaded() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    let existing_plugin_pane = TiledPaneLayout {
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
            configuration: Default::default(),
            ..Default::default()
        }))),
        ..Default::default()
    };
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), existing_plugin_pane];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: true,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        skip_plugin_cache: false,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![plugin_thread, server_thread, screen_thread]);

    let plugin_load_instruction_sent = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PluginInstruction::Load(..) => true,
            _ => false,
        })
        .is_some();
    assert!(
        !plugin_load_instruction_sent,
        "Plugin Load instruction should not be sent for an already loaded plugin"
    );
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    assert_eq!(
        snapshot_count, 2,
        "Another render was sent for focusing the already loaded plugin"
    );
    for (cursor_coordinates, _snapshot) in snapshots.iter().skip(1) {
        assert!(
            cursor_coordinates.is_none(),
            "Cursor moved to existing plugin in final snapshot indicating focus changed"
        );
    }
}

#[test]
pub fn send_cli_launch_or_focus_plugin_action_when_plugin_is_already_loaded_for_plugin_alias() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    let existing_plugin_pane = TiledPaneLayout {
        run: Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias {
            name: "fixture_plugin_for_tests".to_owned(),
            configuration: Some(Default::default()),
            run_plugin: Some(RunPlugin {
                _allow_exec_host_cmd: false,
                location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
                configuration: Default::default(),
                ..Default::default()
            }),
            ..Default::default()
        }))),
        ..Default::default()
    };
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), existing_plugin_pane];
    let screen_thread = mock_screen.run_with_alias(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        move_to_focused_tab: true,
        url: "fixture_plugin_for_tests".to_owned(),
        configuration: Default::default(),
        skip_plugin_cache: false,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for actions to be
    mock_screen.teardown(vec![plugin_thread, server_thread, screen_thread]);

    let plugin_load_instruction_sent = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PluginInstruction::Load(..) => true,
            _ => false,
        })
        .is_some();
    assert!(
        !plugin_load_instruction_sent,
        "Plugin Load instruction should not be sent for an already loaded plugin"
    );
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    assert_eq!(
        snapshot_count, 3,
        "Another render was sent for focusing the already loaded plugin"
    );
    for (cursor_coordinates, _snapshot) in snapshots.iter().skip(1) {
        assert!(
            cursor_coordinates.is_none(),
            "Cursor moved to existing plugin in final snapshot indicating focus changed"
        );
    }
}

#[test]
pub fn screen_can_suppress_pane() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SuppressPane(PaneId::Terminal(1), 1));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    let mut pane_to_break_free = TiledPaneLayout::default();
    pane_to_break_free.name = Some("pane_to_break_free".to_owned());
    let mut pane_to_stay = TiledPaneLayout::default();
    pane_to_stay.name = Some("pane_to_stay".to_owned());
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![pane_to_break_free, pane_to_stay];
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![], // floating_panes_layout
        Default::default(),
        vec![], // floating panes ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_cannot_break_last_selectable_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let initial_layout = TiledPaneLayout::default();
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_floating_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    let mut pane_to_break_free = TiledPaneLayout::default();
    pane_to_break_free.name = Some("tiled_pane".to_owned());
    let mut floating_pane = FloatingPaneLayout::default();
    floating_pane.name = Some("floating_pane_to_eject".to_owned());
    let mut floating_panes_layout = vec![floating_pane];
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![pane_to_break_free];
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), floating_panes_layout.clone());
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    floating_panes_layout.get_mut(0).unwrap().already_running = true;
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        floating_panes_layout,
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(200));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(200));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(200));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_multiple_stacked_panes_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let mut stacked_parent = TiledPaneLayout::default();
    stacked_parent.children_are_stacked = true;
    stacked_parent.children = vec![
        TiledPaneLayout {
            name: Some("pane_to_stay".to_owned()),
            ..Default::default()
        },
        TiledPaneLayout {
            name: Some("pane_to_break_1".to_owned()),
            ..Default::default()
        },
        TiledPaneLayout {
            name: Some("pane_to_break_2".to_owned()),
            ..Default::default()
        },
    ];
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children = vec![stacked_parent];

    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render

    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPanesToNewTab {
            pane_ids: vec![PaneId::Terminal(1), PaneId::Terminal(2)],
            default_shell: None,
            should_change_focus_to_new_tab: true,
            new_tab_name: None,
            client_id: 1,
            completion_tx: None,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![],
        Default::default(),
        vec![],
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken panes are in the next tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in &snapshots {
        eprintln!("{}", snapshot);
    }
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_plugin_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    let mut pane_to_break_free = TiledPaneLayout::default();
    pane_to_break_free.name = Some("plugin_pane_to_break_free".to_owned());
    pane_to_break_free.run = Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
        configuration: Default::default(),
        ..Default::default()
    })));
    let mut pane_to_stay = TiledPaneLayout::default();
    pane_to_stay.name = Some("pane_to_stay".to_owned());
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![pane_to_break_free, pane_to_stay];
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![], // floating_panes_layout
        Default::default(),
        vec![], // floating panes ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_floating_plugin_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    let mut pane_to_break_free = TiledPaneLayout::default();
    pane_to_break_free.name = Some("tiled_pane".to_owned());
    let mut floating_pane = FloatingPaneLayout::default();
    floating_pane.name = Some("floating_plugin_pane_to_eject".to_owned());
    floating_pane.run = Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
        configuration: Default::default(),
        ..Default::default()
    })));
    let mut floating_panes_layout = vec![floating_pane];
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![pane_to_break_free];
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), floating_panes_layout.clone());
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    floating_panes_layout.get_mut(0).unwrap().already_running = true;
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        floating_panes_layout,
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_move_pane_to_a_new_tab_right() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    let mut pane_to_break_free = TiledPaneLayout::default();
    pane_to_break_free.name = Some("pane_to_break_free".to_owned());
    let mut pane_to_stay = TiledPaneLayout::default();
    pane_to_stay.name = Some("pane_to_stay".to_owned());
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![pane_to_break_free, pane_to_stay];
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));

    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        Default::default(),
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPaneRight(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_move_pane_to_a_new_tab_left() {
    let size = Size { cols: 80, rows: 20 };
    let mut initial_layout = TiledPaneLayout::default();
    let mut pane_to_break_free = TiledPaneLayout::default();
    pane_to_break_free.name = Some("pane_to_break_free".to_owned());
    let mut pane_to_stay = TiledPaneLayout::default();
    pane_to_stay.name = Some("pane_to_stay".to_owned());
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![pane_to_break_free, pane_to_stay];
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPane(Default::default(), 1, None));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        Default::default(),
        vec![], // tiled pane ids
        vec![], // floating pane ids
        Default::default(),
        1,
        true,
        (1, false),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPaneLeft(1, None));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_stack_panes_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![
        TiledPaneLayout::default(),
        TiledPaneLayout::default(),
        TiledPaneLayout::default(),
        TiledPaneLayout::default(),
        TiledPaneLayout::default(),
    ];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let stack_panes_action = CliAction::StackPanes {
        pane_ids: vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
    };
    send_cli_action_to_server(&session_metadata, stack_panes_action, client_id);
    let render_count = received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instruction| matches!(instruction, ServerInstruction::Render(_)))
        .count();
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    let started_at = std::time::Instant::now();
    while received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instruction| matches!(instruction, ServerInstruction::Render(_)))
        .count()
        == render_count
    {
        assert!(
            started_at.elapsed() < std::time::Duration::from_secs(5),
            "timed out waiting for stacked panes to render"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let (_, snapshot) = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    )
    .into_iter()
    .last()
    .expect("stack panes action should render");
    assert_snapshot!(format!("{}", snapshot));
}

#[test]
pub fn send_cli_change_floating_pane_coordinates_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10; // fake client id should not appear in the screen's state
    let initial_tiled_layout = TiledPaneLayout::default();
    let initial_floating_panes = vec![FloatingPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_tiled_layout), initial_floating_panes);

    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let change_floating_pane_coordinates_action = CliAction::ChangeFloatingPaneCoordinates {
        pane_id: "0".to_owned(),
        x: Some("0".to_owned()),
        y: Some("0".to_owned()),
        width: Some("10".to_owned()),
        height: Some("10".to_owned()),
        pinned: None,
        borderless: Some(false),
    };
    send_cli_action_to_server(
        &session_metadata,
        change_floating_pane_coordinates_action,
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(format!("{}", snapshot));
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn go_to_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 1;
    let mut screen = create_new_screen(size, true, true);

    // Create multiple tabs with known IDs
    new_tab(&mut screen, 1, 0); // ID 0
    new_tab(&mut screen, 2, 1); // ID 1
    new_tab(&mut screen, 3, 2); // ID 2

    // Active tab should be the last one created (ID 2)
    assert_eq!(screen.get_active_tab(client_id).unwrap().id, 2);

    // Switch to tab with ID 0
    if let Some(tab_position) = screen.get_tab_position_by_id(0) {
        screen
            .switch_active_tab(tab_position, None, true, client_id)
            .expect("TEST");
    }

    // Verify active tab is now ID 0
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().id,
        0,
        "Active tab should be tab with ID 0"
    );

    // Switch to tab with ID 1
    if let Some(tab_position) = screen.get_tab_position_by_id(1) {
        screen
            .switch_active_tab(tab_position, None, true, client_id)
            .expect("TEST");
    }

    // Verify active tab is now ID 1
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().id,
        1,
        "Active tab should be tab with ID 1"
    );
}

#[test]
pub fn send_cli_go_to_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    // Create tabs
    mock_screen.new_tab(TiledPaneLayout::default());
    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::GoToTabById { id: 1 };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action caused screen updates (Render instructions sent)
    let render_count = received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instr| matches!(instr, ServerInstruction::Render(_)))
        .count();

    assert!(
        render_count > 0,
        "GoToTabById CLI action should trigger screen renders"
    );
}

#[test]
pub fn rename_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    // Create tabs with known IDs
    new_tab(&mut screen, 1, 0); // ID 0
    new_tab(&mut screen, 2, 1); // ID 1

    // Verify initial tab names
    assert_eq!(screen.get_tab_by_id(0).unwrap().name, "Tab #1");
    assert_eq!(screen.get_tab_by_id(1).unwrap().name, "Tab #2");

    // Rename tab with ID 1
    if let Some(tab) = screen.get_tab_by_id_mut(1) {
        tab.name = "CustomTabName".to_string();
    }

    // Verify the tab name changed
    assert_eq!(
        screen.get_tab_by_id(1).unwrap().name,
        "CustomTabName",
        "Tab with ID 1 should be renamed"
    );

    // Verify other tab name unchanged
    assert_eq!(
        screen.get_tab_by_id(0).unwrap().name,
        "Tab #1",
        "Tab with ID 0 should keep original name"
    );
}

#[test]
pub fn send_cli_rename_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::RenameTabById {
        id: 1,
        name: "TestName".to_string(),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action was processed (no panics means routing worked)
    // The action should complete successfully
    assert!(true, "RenameTabById CLI action completed without errors");
}

#[test]
pub fn close_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    // Create multiple tabs with known IDs
    new_tab(&mut screen, 1, 0); // ID 0
    new_tab(&mut screen, 2, 1); // ID 1
    new_tab(&mut screen, 3, 2); // ID 2

    assert_eq!(screen.tabs.len(), 3, "Should have 3 tabs initially");

    // Verify all tabs exist
    assert!(
        screen.get_tab_by_id(0).is_some(),
        "Tab with ID 0 should exist"
    );
    assert!(
        screen.get_tab_by_id(1).is_some(),
        "Tab with ID 1 should exist"
    );
    assert!(
        screen.get_tab_by_id(2).is_some(),
        "Tab with ID 2 should exist"
    );

    // Close tab with ID 1
    screen.close_tab_by_id(1).expect("TEST");

    assert_eq!(screen.tabs.len(), 2, "Should have 2 tabs after closing one");

    // Verify tab with ID 1 no longer exists
    assert!(
        screen.get_tab_by_id(1).is_none(),
        "Tab with ID 1 should not exist"
    );

    // Verify other tabs still exist
    assert!(
        screen.get_tab_by_id(0).is_some(),
        "Tab with ID 0 should still exist"
    );
    assert!(
        screen.get_tab_by_id(2).is_some(),
        "Tab with ID 2 should still exist"
    );
}

#[test]
pub fn send_cli_close_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    mock_screen.new_tab(TiledPaneLayout::default());
    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::CloseTabById { id: 1 };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action was processed (no panics means routing worked)
    // The action should complete successfully
    assert!(true, "CloseTabById CLI action completed without errors");
}

#[test]
pub fn send_cli_new_pane_in_place_with_close_replaced_pane() {
    // Verify that `--close-replaced-pane` propagates from CLI through to the
    // PtyInstruction::SpawnInPlaceTerminal instruction as `true`.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::NewPane {
        direction: None,
        command: vec!["bash".into()],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawn_in_place_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PtyInstruction::SpawnInPlaceTerminal(..) => true,
            _ => false,
        })
        .cloned();

    assert_snapshot!(format!("{:#?}", spawn_in_place_instruction));
}

#[test]
pub fn send_cli_edit_in_place_with_close_replaced_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::Edit {
        file: PathBuf::from("/some/file.txt"),
        direction: None,
        line_number: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawn_in_place_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PtyInstruction::SpawnInPlaceTerminal(..) => true,
            _ => false,
        })
        .cloned();

    assert_snapshot!(format!("{:#?}", spawn_in_place_instruction));
}

#[test]
pub fn send_cli_launch_or_focus_plugin_in_place_with_close_replaced_pane() {
    // Verify that `--close-replaced-pane` propagates from the `launch-or-focus-plugin --in-place`
    // CLI action through to the PtyInstruction::FillPluginCwd instruction as `true`.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10; // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        move_to_focused_tab: false,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        skip_plugin_cache: false,
        global: false,
        edge: None,
        size: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let fill_plugin_cwd_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find(|instruction| match instruction {
            PtyInstruction::FillPluginCwd(..) => true,
            _ => false,
        })
        .cloned();

    assert_snapshot!(format!("{:#?}", fill_plugin_cwd_instruction));
}

fn create_new_screen_with_message_capture(
    size: Size,
) -> (
    Screen,
    Arc<Mutex<HashMap<ClientId, Vec<ServerToClientMsg>>>>,
) {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    let messages = fake_os_input.server_to_client_messages.clone();
    bus.os_input = Some(Box::new(fake_os_input));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mut mode_info = ModeInfo::default();
    mode_info.session_name = Some("zellij-test".into());
    let draw_pane_frames = PaneFrameStyle::None;
    let auto_layout = true;
    let session_is_mirrored = true;
    let copy_options = CopyOptions::default();
    let default_layout = Box::new(Layout::default());
    let default_layout_name = None;
    let default_shell = PathBuf::from("my_default_shell");
    let session_serialization = true;
    let serialize_pane_viewport = false;
    let scrollback_lines_to_serialize = None;
    let layout_dir = None;
    let debug = false;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let arrow_fonts = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let stacked_resize = true;
    let web_sharing = WebSharing::Off;
    let web_server_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    let web_server_port = 8080;
    let visual_bell = true;
    let screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        draw_pane_frames,
        auto_layout,
        session_is_mirrored,
        copy_options,
        debug,
        default_layout,
        default_layout_name,
        default_shell,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        styled_underlines,
        osc8_hyperlinks,
        arrow_fonts,
        layout_dir,
        explicitly_disable_kitty_keyboard_protocol,
        true, // support_kitty_graphics_protocol
        stacked_resize,
        false,
        None,
        false,
        web_sharing,
        true,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,
        true,
        visual_bell,
        false, // focus_follows_mouse
        false, // mouse_click_through
        web_server_ip,
        web_server_port,
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    (seed_first_client_size(screen, size), messages)
}

#[test]
fn subscriber_receives_initial_delivery() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    assert_eq!(client_msgs.len(), 1);
    match &client_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial,
            scrollback,
            ..
        } => {
            assert!(*is_initial);
            assert!(scrollback.is_none());
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn subscriber_receives_initial_with_scrollback() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        Some(0),
        false,
    );

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    assert_eq!(client_msgs.len(), 1);
    match &client_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial,
            scrollback,
            ..
        } => {
            assert!(*is_initial);
            assert!(scrollback.is_some());
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn subscriber_no_update_on_unchanged_viewport() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    let initial_viewport = {
        let msgs = messages.lock().unwrap();
        let client_msgs = msgs.get(&100).unwrap();
        match &client_msgs[0] {
            ServerToClientMsg::PaneRenderUpdate { viewport, .. } => viewport.clone(),
            _ => panic!("Expected PaneRenderUpdate"),
        }
    };

    let mut pane_map = HashMap::new();
    pane_map.insert(
        zellij_utils::data::PaneId::Terminal(1),
        PaneContents {
            viewport: initial_viewport,
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&pane_map, None);

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    assert_eq!(client_msgs.len(), 1);
}

#[test]
fn subscriber_receives_update_on_changed_viewport() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    let mut pane_map = HashMap::new();
    pane_map.insert(
        zellij_utils::data::PaneId::Terminal(1),
        PaneContents {
            viewport: vec!["changed line".to_string()],
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&pane_map, None);

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    assert_eq!(client_msgs.len(), 2);
    match &client_msgs[1] {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial,
            scrollback,
            viewport,
            ..
        } => {
            assert!(!is_initial);
            assert!(scrollback.is_none());
            assert_eq!(viewport, &vec!["changed line".to_string()]);
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn subscriber_error_for_nonexistent_pane() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(999)],
        None,
        false,
    );

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    assert_eq!(client_msgs.len(), 1);
    match &client_msgs[0] {
        ServerToClientMsg::LogError { lines } => {
            let joined = lines.join(" ");
            assert!(
                joined.contains("not found"),
                "Error message should contain 'not found', got: {}",
                joined
            );
        },
        other => panic!("Expected LogError, got {:?}", other),
    }
    assert!(
        !screen.pane_render_subscribers.contains_key(&100),
        "Subscriber should not be registered for nonexistent pane"
    );
}

#[test]
fn subscriber_state_registered_for_multiple_panes() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    screen.subscribe_to_pane_renders(
        100,
        vec![
            zellij_utils::data::PaneId::Terminal(1),
            zellij_utils::data::PaneId::Terminal(2),
        ],
        None,
        false,
    );

    let sub = screen.pane_render_subscribers.get(&100).unwrap();
    assert_eq!(sub.pane_ids.len(), 2);
    assert!(sub
        .pane_ids
        .contains(&zellij_utils::data::PaneId::Terminal(1)));
    assert!(sub
        .pane_ids
        .contains(&zellij_utils::data::PaneId::Terminal(2)));
}

#[test]
fn multiple_subscribers_receive_updates() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        101,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    let mut pane_map = HashMap::new();
    pane_map.insert(
        zellij_utils::data::PaneId::Terminal(1),
        PaneContents {
            viewport: vec!["new content".to_string()],
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&pane_map, None);

    let msgs = messages.lock().unwrap();
    let client_100_msgs = msgs.get(&100).unwrap();
    let client_101_msgs = msgs.get(&101).unwrap();
    assert_eq!(client_100_msgs.len(), 2);
    assert_eq!(client_101_msgs.len(), 2);
}

#[test]
fn subscriber_removed_on_remove_client() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );
    assert!(screen.pane_render_subscribers.contains_key(&100));

    let _ = screen.remove_client(100);
    assert!(!screen.pane_render_subscribers.contains_key(&100));
}

#[test]
fn subscriber_removed_when_all_panes_closed() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(zellij_utils::data::PaneId::Terminal(1));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: zellij_utils::data::PaneId::Terminal(1),
            }
        )
    });
    let has_exit = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            }
        )
    });
    assert!(has_pane_closed, "Should send SubscribedPaneClosed");
    assert!(has_exit, "Should send Exit when all panes closed");
    assert!(!screen.pane_render_subscribers.contains_key(&100));
}

#[test]
fn subscriber_partial_close() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    screen.subscribe_to_pane_renders(
        100,
        vec![
            zellij_utils::data::PaneId::Terminal(1),
            zellij_utils::data::PaneId::Terminal(2),
        ],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(zellij_utils::data::PaneId::Terminal(1));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: zellij_utils::data::PaneId::Terminal(1),
            }
        )
    });
    let has_exit = client_msgs
        .iter()
        .any(|m| matches!(m, ServerToClientMsg::Exit { .. }));
    assert!(
        has_pane_closed,
        "Should send SubscribedPaneClosed for closed pane"
    );
    assert!(!has_exit, "Should NOT send Exit when panes remain");
    assert!(screen.pane_render_subscribers.contains_key(&100));
    assert_eq!(
        screen
            .pane_render_subscribers
            .get(&100)
            .unwrap()
            .pane_ids
            .len(),
        1
    );
}

#[test]
fn subscriber_full_close_sequence() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    screen.subscribe_to_pane_renders(
        100,
        vec![
            zellij_utils::data::PaneId::Terminal(1),
            zellij_utils::data::PaneId::Terminal(2),
        ],
        None,
        false,
    );

    screen.notify_pane_closed_to_subscribers(zellij_utils::data::PaneId::Terminal(1));
    screen.notify_pane_closed_to_subscribers(zellij_utils::data::PaneId::Terminal(2));

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    let pane_closed_count = client_msgs
        .iter()
        .filter(|m| matches!(m, ServerToClientMsg::SubscribedPaneClosed { .. }))
        .count();
    let exit_count = client_msgs
        .iter()
        .filter(|m| {
            matches!(
                m,
                ServerToClientMsg::Exit {
                    exit_reason: ExitReason::Normal,
                }
            )
        })
        .count();
    assert_eq!(pane_closed_count, 2, "Two SubscribedPaneClosed messages");
    assert_eq!(exit_count, 1, "One Exit message after all panes closed");
    assert!(!screen.pane_render_subscribers.contains_key(&100));
}

#[test]
fn delivery_path_a_and_b_produce_same_content() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    messages.lock().unwrap().get_mut(&100).unwrap().clear();

    let contents = {
        let server_pane_id = PaneId::Terminal(1);
        let mut found_contents = None;
        for tab in screen.tabs.values() {
            if let Some(pane) = tab.get_pane_with_id(server_pane_id) {
                found_contents = Some(pane.pane_contents(None, false, None));
                break;
            }
        }
        found_contents.expect("Pane should exist")
    };

    screen
        .pane_render_subscribers
        .get_mut(&100)
        .unwrap()
        .previous_viewports
        .insert(
            zellij_utils::data::PaneId::Terminal(1),
            vec!["old".to_string()],
        );

    let mut pane_map = HashMap::new();
    pane_map.insert(zellij_utils::data::PaneId::Terminal(1), contents.clone());
    let mut all_pane_contents = HashMap::new();
    all_pane_contents.insert(1 as ClientId, pane_map);
    let report = PaneRenderReport {
        all_pane_contents,
        all_pane_contents_with_ansi: HashMap::new(),
    };
    screen.deliver_to_pane_subscribers_from_report(&report);

    let viewport_a = {
        let msgs = messages.lock().unwrap();
        let client_msgs = msgs.get(&100).unwrap();
        match &client_msgs[0] {
            ServerToClientMsg::PaneRenderUpdate { viewport, .. } => viewport.clone(),
            other => panic!("Expected PaneRenderUpdate, got {:?}", other),
        }
    };

    messages.lock().unwrap().get_mut(&100).unwrap().clear();
    screen
        .pane_render_subscribers
        .get_mut(&100)
        .unwrap()
        .previous_viewports
        .insert(
            zellij_utils::data::PaneId::Terminal(1),
            vec!["old".to_string()],
        );

    screen.deliver_to_pane_subscribers_directly();

    let viewport_b = {
        let msgs = messages.lock().unwrap();
        let client_msgs = msgs.get(&100).unwrap();
        match &client_msgs[0] {
            ServerToClientMsg::PaneRenderUpdate { viewport, .. } => viewport.clone(),
            other => panic!("Expected PaneRenderUpdate, got {:?}", other),
        }
    };

    assert_eq!(
        viewport_a, viewport_b,
        "Both delivery paths should produce identical viewport"
    );
}

#[test]
fn close_tab_notifies_subscribers() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );

    messages.lock().unwrap().get_mut(&100).unwrap().clear();

    let _ = screen.go_to_tab(1, 1);
    let _ = screen.close_tab(1);

    let msgs = messages.lock().unwrap();
    let client_msgs = msgs.get(&100).unwrap();
    let has_pane_closed = client_msgs.iter().any(|m| {
        matches!(
            m,
            ServerToClientMsg::SubscribedPaneClosed {
                pane_id: zellij_utils::data::PaneId::Terminal(1),
            }
        )
    });
    assert!(
        has_pane_closed,
        "Closing tab should notify subscriber of pane closure"
    );
}

#[test]
fn close_pane_notifies_subscribers_via_instruction() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let _session_metadata = mock_screen.clone_session_metadata();

    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];

    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![
                zellij_utils::data::PaneId::Terminal(0),
                zellij_utils::data::PaneId::Terminal(1),
            ],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::CloseFocusedPane(1, None));
    std::thread::sleep(std::time::Duration::from_millis(200));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let client_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();
    let has_pane_closed = client_msgs
        .iter()
        .any(|m| matches!(m, ServerToClientMsg::SubscribedPaneClosed { .. }));
    assert!(
        has_pane_closed,
        "Closing pane via CLI action should notify subscriber. Messages: {:?}",
        client_msgs
    );
}

#[test]
fn integration_pty_bytes_delivered_to_subscriber() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![zellij_utils::data::PaneId::Terminal(0)],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "hello world\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();

    assert!(
        subscriber_msgs.len() >= 2,
        "Should have at least initial + update, got {}",
        subscriber_msgs.len()
    );

    match &subscriber_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { is_initial, .. } => {
            assert!(*is_initial, "First message should be initial");
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }

    let has_hello = subscriber_msgs.iter().any(|m| match m {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial: false,
            viewport,
            ..
        } => viewport.iter().any(|line| line.contains("hello world")),
        _ => false,
    });
    assert!(
        has_hello,
        "Subsequent message should contain 'hello world' in viewport. Messages: {:?}",
        subscriber_msgs
    );
}

#[test]
fn integration_pty_bytes_not_delivered_when_viewport_unchanged() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![zellij_utils::data::PaneId::Terminal(0)],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();

    let render_update_count = subscriber_msgs
        .iter()
        .filter(|m| matches!(m, ServerToClientMsg::PaneRenderUpdate { .. }))
        .count();
    assert_eq!(
        render_update_count, 1,
        "Only the initial delivery should be present, not a duplicate. Got {} messages: {:?}",
        render_update_count, subscriber_msgs
    );
}

#[test]
fn integration_scrollback_from_pre_subscription_pty_bytes() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let mut data = String::new();
    for i in 0..30 {
        data.push_str(&format!("line {}\r\n", i));
    }
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::PtyBytes(0, data.as_bytes().to_vec()));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![zellij_utils::data::PaneId::Terminal(0)],
            scrollback: Some(0),
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "post subscribe line\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();

    let initial_msg = subscriber_msgs.iter().find(|m| {
        matches!(
            m,
            ServerToClientMsg::PaneRenderUpdate {
                is_initial: true,
                ..
            }
        )
    });
    assert!(initial_msg.is_some(), "Should have an initial message");
    match initial_msg.unwrap() {
        ServerToClientMsg::PaneRenderUpdate {
            scrollback,
            is_initial,
            ..
        } => {
            assert!(*is_initial);
            assert!(
                scrollback.is_some(),
                "Initial message should include scrollback"
            );
            let sb = scrollback.as_ref().unwrap();
            assert!(
                !sb.is_empty(),
                "Scrollback should contain pre-subscription lines"
            );
            let has_early_lines = sb.iter().any(|line| line.contains("line 0"));
            assert!(
                has_early_lines,
                "Scrollback should contain early lines. Got: {:?}",
                sb
            );
        },
        _ => unreachable!(),
    }

    let subsequent_updates: Vec<_> = subscriber_msgs
        .iter()
        .filter(|m| {
            matches!(
                m,
                ServerToClientMsg::PaneRenderUpdate {
                    is_initial: false,
                    ..
                }
            )
        })
        .collect();
    for msg in &subsequent_updates {
        match msg {
            ServerToClientMsg::PaneRenderUpdate { scrollback, .. } => {
                assert!(
                    scrollback.is_none(),
                    "Subsequent updates should not include scrollback"
                );
            },
            _ => unreachable!(),
        }
    }
}

#[test]
fn integration_no_scrollback_when_not_requested() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let mut data = String::new();
    for i in 0..30 {
        data.push_str(&format!("line {}\r\n", i));
    }
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::PtyBytes(0, data.as_bytes().to_vec()));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![zellij_utils::data::PaneId::Terminal(0)],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "post subscribe\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();

    for msg in &subscriber_msgs {
        match msg {
            ServerToClientMsg::PaneRenderUpdate { scrollback, .. } => {
                assert!(
                    scrollback.is_none(),
                    "Scrollback should be None when not requested. Got: {:?}",
                    scrollback
                );
            },
            _ => {},
        }
    }
}

#[test]
fn integration_subscriber_survives_after_regular_client_detach() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![zellij_utils::data::PaneId::Terminal(0)],
            scrollback: None,
            ansi: false,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::RemoveClient(1));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "after detach\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();

    let has_after_detach = subscriber_msgs.iter().any(|m| match m {
        ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
            viewport.iter().any(|line| line.contains("after detach"))
        },
        _ => false,
    });
    assert!(
        has_after_detach,
        "Subscriber should receive updates after regular client detach. Messages: {:?}",
        subscriber_msgs
    );
}

// ==========================================
// Category 3: MockScreen end-to-end CLI tests
// ==========================================

#[test]
pub fn send_cli_scroll_up_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollUp {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ScrollUp with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_scroll_down_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollDown {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ScrollDown with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_scroll_to_top_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollToTop {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ScrollToTop with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_scroll_to_bottom_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ScrollToBottom {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ScrollToBottom with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_page_scroll_up_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::PageScrollUp {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "PageScrollUp with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_page_scroll_down_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::PageScrollDown {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "PageScrollDown with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_half_page_scroll_up_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::HalfPageScrollUp {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "HalfPageScrollUp with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_half_page_scroll_down_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::HalfPageScrollDown {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "HalfPageScrollDown with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_resize_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::Resize {
        resize: Resize::Increase,
        direction: Some(Direction::Left),
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "Resize with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_move_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::MovePane {
        direction: Some(Direction::Right),
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "MovePane with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_move_pane_backwards_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::MovePaneBackwards {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "MovePaneBackwards with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_clear_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::Clear {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "Clear with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_edit_scrollback_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::EditScrollback {
        pane_id: Some("terminal_0".to_string()),
        ansi: false,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "EditScrollback with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_toggle_fullscreen_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ToggleFullscreen {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ToggleFullscreen with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_toggle_pane_embed_or_floating_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::TogglePaneEmbedOrFloating {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "TogglePaneEmbedOrFloating with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_close_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ClosePane {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ClosePane with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_rename_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::RenamePane {
        name: "targeted-name".to_string(),
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "RenamePane with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_undo_rename_pane_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::UndoRenamePane {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "UndoRenamePane with pane_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_toggle_pane_pinned_with_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::TogglePanePinned {
        pane_id: Some("terminal_0".to_string()),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "TogglePanePinned with pane_id CLI action completed without errors"
    );
}

// TAB-TARGETING MockScreen tests

#[test]
pub fn send_cli_close_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::CloseTab { tab_id: Some(1) };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "CloseTab with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_rename_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::RenameTab {
        name: "targeted-tab".to_string(),
        tab_id: Some(1),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "RenameTab with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_undo_rename_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::UndoRenameTab { tab_id: Some(0) };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "UndoRenameTab with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_toggle_active_sync_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ToggleActiveSyncTab { tab_id: Some(0) };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ToggleActiveSyncTab with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_toggle_floating_panes_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::ToggleFloatingPanes { tab_id: Some(0) };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "ToggleFloatingPanes with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_previous_swap_layout_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::PreviousSwapLayout { tab_id: Some(0) };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "PreviousSwapLayout with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_next_swap_layout_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::NextSwapLayout { tab_id: Some(0) };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "NextSwapLayout with tab_id CLI action completed without errors"
    );
}

#[test]
pub fn send_cli_move_tab_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.new_tab(TiledPaneLayout::default());
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::MoveTab {
        direction: Direction::Right,
        tab_id: Some(0),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "MoveTab with tab_id CLI action completed without errors"
    );
}

// ==========================================
// Category 4: Direct Screen method tests
// ==========================================

#[test]
pub fn move_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    new_tab(&mut screen, 3, 2);
    let original_pos_0 = screen.get_tab_by_id(0).unwrap().position;
    let original_pos_1 = screen.get_tab_by_id(1).unwrap().position;
    screen.move_tab_by_id(0, Direction::Right).expect("TEST");
    assert_eq!(screen.get_tab_by_id(0).unwrap().position, original_pos_1);
    assert_eq!(screen.get_tab_by_id(1).unwrap().position, original_pos_0);
}

// ==========================================
// Category 5: ANSI flag tests
// ==========================================

#[test]
pub fn send_cli_dump_screen_action_with_ansi() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/foo_ansi")),
        full: true,
        pane_id: None,
        ansi: true,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "\x1b[31mred text\x1b[0m".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let fs = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped_content = fs.values().next().expect("Should have dumped a file");
    assert!(
        dumped_content.contains("\x1b["),
        "Dumped file should contain ANSI escape codes when ansi flag is true. Content: {:?}",
        dumped_content
    );
}

#[test]
pub fn send_cli_dump_screen_action_without_ansi_strips_codes() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/foo_plain")),
        full: true,
        pane_id: None,
        ansi: false,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "\x1b[31mred text\x1b[0m".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let fs = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped_content = fs.values().next().expect("Should have dumped a file");
    assert!(
        !dumped_content.contains("\x1b["),
        "Dumped file should NOT contain ANSI escape codes when ansi flag is false. Content: {:?}",
        dumped_content
    );
}

#[test]
pub fn send_cli_edit_scrollback_action_with_ansi() {
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::EditScrollback {
        pane_id: None,
        ansi: true,
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "\x1b[31mred text\x1b[0m".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let fs = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped_content = fs.values().next().expect("Should have dumped a file");
    assert!(
        dumped_content.contains("\x1b["),
        "Edit scrollback dump should contain ANSI escape codes when ansi flag is true. Content: {:?}",
        dumped_content
    );
}

#[test]
pub fn send_cli_edit_scrollback_with_pane_id_and_ansi() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    let cli_action = CliAction::EditScrollback {
        pane_id: Some("terminal_0".to_string()),
        ansi: true,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert!(
        true,
        "EditScrollback with pane_id and ansi CLI action completed without errors"
    );
}

#[test]
fn subscriber_ansi_flag_preserved_in_subscription() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        101,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        true,
    );

    assert!(
        !screen.pane_render_subscribers.get(&100).unwrap().ansi,
        "Subscriber 100 should have ansi=false"
    );
    assert!(
        screen.pane_render_subscribers.get(&101).unwrap().ansi,
        "Subscriber 101 should have ansi=true"
    );
}

#[test]
fn subscriber_ansi_and_plain_receive_different_content() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);

    // Subscribe plain and ansi subscribers
    screen.subscribe_to_pane_renders(
        100,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        false,
    );
    screen.subscribe_to_pane_renders(
        101,
        vec![zellij_utils::data::PaneId::Terminal(1)],
        None,
        true,
    );

    // Clear initial messages
    messages.lock().unwrap().get_mut(&100).unwrap().clear();
    messages.lock().unwrap().get_mut(&101).unwrap().clear();

    // Reset previous viewports to force delivery
    screen
        .pane_render_subscribers
        .get_mut(&100)
        .unwrap()
        .previous_viewports
        .insert(
            zellij_utils::data::PaneId::Terminal(1),
            vec!["old".to_string()],
        );
    screen
        .pane_render_subscribers
        .get_mut(&101)
        .unwrap()
        .previous_viewports
        .insert(
            zellij_utils::data::PaneId::Terminal(1),
            vec!["old".to_string()],
        );

    // Build plain and ansi maps with different content
    let mut plain_map = HashMap::new();
    plain_map.insert(
        zellij_utils::data::PaneId::Terminal(1),
        PaneContents {
            viewport: vec!["plain text".to_string()],
            ..Default::default()
        },
    );
    let mut ansi_map = HashMap::new();
    ansi_map.insert(
        zellij_utils::data::PaneId::Terminal(1),
        PaneContents {
            viewport: vec!["\x1b[31mred text\x1b[0m".to_string()],
            ..Default::default()
        },
    );
    screen.deliver_subscriber_updates_from_map(&plain_map, Some(&ansi_map));

    let msgs = messages.lock().unwrap();
    let plain_msgs = msgs.get(&100).unwrap();
    let ansi_msgs = msgs.get(&101).unwrap();

    assert_eq!(
        plain_msgs.len(),
        1,
        "Plain subscriber should receive one update"
    );
    assert_eq!(
        ansi_msgs.len(),
        1,
        "Ansi subscriber should receive one update"
    );

    match &plain_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
            assert_eq!(viewport, &vec!["plain text".to_string()]);
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
    match &ansi_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { viewport, .. } => {
            assert_eq!(viewport, &vec!["\x1b[31mred text\x1b[0m".to_string()]);
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }
}

#[test]
fn integration_subscribe_with_ansi_flag() {
    let size = Size { cols: 80, rows: 20 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let screen_thread = mock_screen.run(None, vec![]);

    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::SubscribeToPaneRenders {
            client_id: 100,
            pane_ids: vec![zellij_utils::data::PaneId::Terminal(0)],
            scrollback: None,
            ansi: true,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        "\x1b[31mred text\x1b[0m\r\n".as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);

    let msgs = mock_screen
        .os_input
        .server_to_client_messages
        .lock()
        .unwrap();
    let subscriber_msgs = msgs.get(&100).unwrap_or(&vec![]).clone();

    assert!(
        subscriber_msgs.len() >= 2,
        "Should have at least initial + update, got {}",
        subscriber_msgs.len()
    );

    match &subscriber_msgs[0] {
        ServerToClientMsg::PaneRenderUpdate { is_initial, .. } => {
            assert!(*is_initial, "First message should be initial");
        },
        other => panic!("Expected PaneRenderUpdate, got {:?}", other),
    }

    let has_ansi_content = subscriber_msgs.iter().any(|m| match m {
        ServerToClientMsg::PaneRenderUpdate {
            is_initial: false,
            viewport,
            ..
        } => viewport.iter().any(|line| line.contains("\x1b[")),
        _ => false,
    });
    assert!(
        has_ansi_content,
        "ANSI subscriber should receive viewport lines with ANSI escape codes. Messages: {:?}",
        subscriber_msgs
    );
}

#[test]
pub fn background_plugin_receives_broadcasts_regardless_of_active_tab() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins, queued before run)
    // Tab 1: plugin pane 3 (from new_tab_with_plugins, queued before run)
    // Tab 2: terminal panes only (from run, starts screen thread)
    // After run, client is on tab 2. Switch to tab 0 (plugin 2).
    // Background plugin 99 should also receive updates.
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10;

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![2]);
    mock_screen.new_tab_with_plugins(vec![3]);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);

    // Register background plugin 99 with the main_client_id (1), not the CLI client_id (10)
    let main_client_id = mock_screen.main_client_id;
    let mut bg_subs = HashSet::new();
    bg_subs.insert(EventType::TabUpdate);
    bg_subs.insert(EventType::ModeUpdate);
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::UpdateBackgroundPluginSubscriptions(
            99,
            main_client_id,
            bg_subs,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    // Drain initial setup instructions before the GoToTab action
    std::thread::sleep(std::time::Duration::from_millis(100));
    let instructions_before_switch = received_plugin_instructions.lock().unwrap().len();

    // Switch to tab 0 (1-based index 1 = position 0 = tab with plugin 2)
    let goto_tab = CliAction::GoToTab { index: 1 };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock().unwrap();
    // Only examine instructions sent after the switch
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let mut plugin_ids_that_received_tab_update: Vec<u32> = vec![];
    let mut plugin_ids_that_received_mode_update: Vec<u32> = vec![];
    for instruction in instructions_after_switch.iter() {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                match event {
                    Event::TabUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_tab_update.push(*id);
                        }
                    },
                    Event::ModeUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_mode_update.push(*id);
                        }
                    },
                    _ => {},
                }
            }
        }
    }

    // Plugin 2 (active tab) and plugin 99 (background) should receive updates
    assert!(
        plugin_ids_that_received_tab_update.contains(&2),
        "Active tab plugin 2 should receive TabUpdate, got: {:?}",
        plugin_ids_that_received_tab_update
    );
    assert!(
        plugin_ids_that_received_tab_update.contains(&99),
        "Background plugin 99 should receive TabUpdate, got: {:?}",
        plugin_ids_that_received_tab_update
    );
    // Plugin 3 (inactive tab) should NOT receive updates
    assert!(
        !plugin_ids_that_received_tab_update.contains(&3),
        "Inactive tab plugin 3 should NOT receive TabUpdate, got: {:?}",
        plugin_ids_that_received_tab_update
    );

    // ModeUpdate is sent via update_input_modes() to tab plugins only (not background plugins).
    // Background plugins receive ModeUpdate only via explicit broadcast_mode_update calls.
    // So during tab switch, only the active tab's plugins get ModeUpdate.
    assert!(
        plugin_ids_that_received_mode_update.contains(&2),
        "Active tab plugin 2 should receive ModeUpdate, got: {:?}",
        plugin_ids_that_received_mode_update
    );
    assert!(
        !plugin_ids_that_received_mode_update.contains(&3),
        "Inactive tab plugin 3 should NOT receive ModeUpdate, got: {:?}",
        plugin_ids_that_received_mode_update
    );
}

#[test]
pub fn tab_switch_only_updates_active_tab_plugins() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins)
    // Tab 1: plugin pane 3 (from new_tab_with_plugins)
    // Tab 2: terminal panes only (from run)
    // After run, client is on tab 2. Switch to tab 0 (plugin 2).
    // Only plugin 2 should receive updates; plugin 3 should not.
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10;

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![2]);
    mock_screen.new_tab_with_plugins(vec![3]);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);

    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    // Drain initial setup instructions before the GoToTab action
    std::thread::sleep(std::time::Duration::from_millis(100));
    let instructions_before_switch = received_plugin_instructions.lock().unwrap().len();

    // Switch to tab 0 (1-based index 1 = position 0 = tab with plugin 2)
    let goto_tab = CliAction::GoToTab { index: 1 };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock().unwrap();
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let mut plugin_ids_that_received_updates: Vec<u32> = vec![];
    for instruction in instructions_after_switch.iter() {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                match event {
                    Event::TabUpdate(..) | Event::ModeUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_updates.push(*id);
                        }
                    },
                    _ => {},
                }
            }
        }
    }

    // Only plugin 2 (active tab after switch) should receive TabUpdate/ModeUpdate
    assert!(
        plugin_ids_that_received_updates.contains(&2),
        "Active tab plugin 2 should receive updates, got: {:?}",
        plugin_ids_that_received_updates
    );
    assert!(
        !plugin_ids_that_received_updates.contains(&3),
        "Inactive tab plugin 3 should NOT receive updates, got: {:?}",
        plugin_ids_that_received_updates
    );
}

#[test]
pub fn inactive_tab_plugins_get_fresh_state_on_activation() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins)
    // Tab 1: terminal panes only (from run, client starts here)
    // Switch to tab 0 → plugin 2 becomes active and receives TabUpdate with both tabs.
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10;

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![2]);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);

    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    // Drain initial setup instructions before the GoToTab action
    std::thread::sleep(std::time::Duration::from_millis(100));
    let instructions_before_switch = received_plugin_instructions.lock().unwrap().len();

    // Switch to tab 0 (1-based index 1 = position 0 = tab with plugin 2)
    let goto_tab = CliAction::GoToTab { index: 1 };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock().unwrap();
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let tab_update_for_plugin_2 = instructions_after_switch.iter().find_map(|instruction| {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                if let (Some(2), Event::TabUpdate(tab_infos)) = (pid, event) {
                    return Some(tab_infos.clone());
                }
            }
        }
        None
    });

    assert!(
        tab_update_for_plugin_2.is_some(),
        "Plugin 2 should receive a TabUpdate after becoming active"
    );
    let tab_infos = tab_update_for_plugin_2.unwrap();
    assert!(
        tab_infos.len() >= 2,
        "TabUpdate should contain info for both tabs, got {} tabs",
        tab_infos.len()
    );
    let active_tab = tab_infos.iter().find(|t| t.active);
    assert!(
        active_tab.is_some(),
        "TabUpdate should have an active tab marked"
    );
    // Tab at position 0 should be active after switching to GoToTab index 1 (1-based)
    let active_tab = active_tab.unwrap();
    assert_eq!(
        active_tab.position, 0,
        "The first tab (position 0) should be active, got position {}",
        active_tab.position
    );
}

#[test]
pub fn send_cli_new_tab_action_with_layout_string() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    // Same layout as layout-with-three-panes.kdl but passed as a string
    let new_tab_action = CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: Some("layout {\n    pane\n    pane\n    pane\n}\n".into()),
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: Default::default(),
        start_suspended: Default::default(),
        block_until_exit: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        no_focus: false,
    };
    send_cli_action_to_server(&session_metadata, new_tab_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let new_tab_instruction = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|i| {
            if let PluginInstruction::NewTab(..) = i {
                return true;
            } else {
                return false;
            }
        })
        .unwrap()
        .clone();
    let output = format!("{:#?}", new_tab_instruction);
    assert_snapshot!(output);
}

#[test]
pub fn send_cli_new_tab_action_with_layout_string_and_name() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10;
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_plugin_instructions = Arc::new(Mutex::new(vec![]));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );
    let new_tab_action = CliAction::NewTab {
        name: Some("my-string-layout-tab".into()),
        layout: None,
        layout_string: Some("layout {\n    pane\n    pane\n    pane\n}\n".into()),
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: Default::default(),
        start_suspended: Default::default(),
        block_until_exit: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        no_focus: false,
    };
    send_cli_action_to_server(&session_metadata, new_tab_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![plugin_thread, screen_thread]);
    let new_tab_instruction = received_plugin_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|i| {
            if let PluginInstruction::NewTab(..) = i {
                return true;
            } else {
                return false;
            }
        })
        .unwrap()
        .clone();
    let output = format!("{:#?}", new_tab_instruction);
    assert_snapshot!(output);
}

#[test]
pub fn send_cli_new_pane_action_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: Some(Direction::Right),
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        global: false,
        edge: None,
        size: None,
        tab_id: Some(0),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock().unwrap();
    // Verify that the PTY instruction uses TabIndex(0) instead of ClientId
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(0)"),
        "Expected TabIndex(0) in PTY instructions, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_new_floating_pane_action_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: None,
        command: vec![],
        plugin: None,
        cwd: None,
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        global: false,
        edge: None,
        size: None,
        tab_id: Some(0),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock().unwrap();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(0)"),
        "Expected TabIndex(0) in PTY instructions for floating pane, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_edit_action_with_tab_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_edit_action = CliAction::Edit {
        file: PathBuf::from("/tmp/test.rs"),
        direction: None,
        line_number: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(0),
    };
    send_cli_action_to_server(&session_metadata, cli_edit_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock().unwrap();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(0)"),
        "Expected TabIndex(0) in PTY instructions for edit, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_new_pane_action_with_tab_id_and_direction() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: Some(Direction::Right),
        command: vec![],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: Some(false),
        global: false,
        edge: None,
        size: None,
        tab_id: Some(0),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock().unwrap();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(0)"),
        "Expected TabIndex(0) in PTY instructions with direction, got: {}",
        pty_debug
    );
}

#[test]
pub fn send_cli_new_pane_action_with_tab_id_and_stacked() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_new_pane_action = CliAction::NewPane {
        direction: None,
        command: vec!["ls".into()],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        stacked: true,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        global: false,
        edge: None,
        size: None,
        tab_id: Some(0),
    };
    send_cli_action_to_server(&session_metadata, cli_new_pane_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);
    let pty_instructions = received_pty_instructions.lock().unwrap();
    let pty_debug = format!("{:?}", *pty_instructions);
    assert!(
        pty_debug.contains("TabIndex(0)"),
        "Expected TabIndex(0) in PTY instructions with stacked, got: {}",
        pty_debug
    );
}

#[test]
fn cli_rename_active_pane_via_screen_replaces_name() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let client_id = 1;
    new_tab(&mut screen, 1, 0);

    // First give the pane a name
    let pane_id = PaneId::Terminal(1);
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    }

    // Now rename via the active pane path (what CLI rename without --pane-id does)
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_active_pane("spark".as_bytes().to_vec(), client_id);
    }

    let tab = screen.get_active_tab(client_id).unwrap();
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(
        pane.current_title(),
        "spark",
        "CLI rename should fully replace the name"
    );
}

#[test]
fn cli_rename_active_pane_single_char_via_screen() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let client_id = 1;
    new_tab(&mut screen, 1, 0);

    let pane_id = PaneId::Terminal(1);
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    }

    // Single char rename via active pane path
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_active_pane("x".as_bytes().to_vec(), client_id);
    }

    let tab = screen.get_active_tab(client_id).unwrap();
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(
        pane.current_title(),
        "x",
        "Single-char CLI rename should replace, not append"
    );
}

#[test]
fn cli_rename_focused_pane_single_char_via_rename_active_pane() {
    // Tests that single-char CLI rename via RenameActivePane (the path used
    // by `zellij action rename-pane "x"` without --pane-id) correctly
    // replaces the existing name.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    let client_id = 1;
    new_tab(&mut screen, 1, 0);

    let pane_id = PaneId::Terminal(1);
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_pane_by_pane_id(pane_id, "flame".as_bytes().to_vec());
    }

    // CLI rename single char via RenameActivePane (full replacement)
    if let Ok(tab) = screen.get_active_tab_mut(client_id) {
        let _ = tab.rename_active_pane("x".as_bytes().to_vec(), client_id);
    }

    let tab = screen.get_active_tab(client_id).unwrap();
    let pane = tab.get_pane_with_id(pane_id).unwrap();
    assert_eq!(
        pane.current_title(),
        "x",
        "Single-char CLI rename should replace the name, not append"
    );
}

#[test]
pub fn pty_bytes_and_hold_pane_buffered_before_new_pane() {
    // Regression test: when a command exits very quickly (e.g. `zellij run -- echo hello`),
    // PtyBytes and HoldPane can arrive at the screen thread before NewPane because the async
    // reader and quit_cb run on separate threads. This test verifies that such early-arriving
    // events are buffered and replayed once NewPane is processed.
    let size = Size { cols: 80, rows: 20 };
    let client_id = 10;
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // The initial layout creates pane id 0. We will use pane id 2 for the new pane
    // (id 1 is used by the plugin in the initial layout).
    let new_pane_id = 2;

    // Simulate the race: send PtyBytes for the new pane BEFORE NewPane
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        new_pane_id,
        "hello\r\n".as_bytes().to_vec(),
    ));

    // Send HoldPane before NewPane as well
    let run_command = RunCommand {
        command: PathBuf::from("echo"),
        args: vec!["hello".to_string()],
        hold_on_close: true,
        ..Default::default()
    };
    let _ = mock_screen.to_screen.send(ScreenInstruction::HoldPane(
        PaneId::Terminal(new_pane_id),
        Some(0),
        run_command,
    ));

    // Small sleep to ensure the above messages are processed (and buffered) first
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Now send NewPane — this should replay the buffered PtyBytes and HoldPane
    let _ = mock_screen.to_screen.send(ScreenInstruction::NewPane(
        PaneId::Terminal(new_pane_id),
        Some("echo hello".to_string()),
        None, // hold_for_command
        None, // invoked_with
        NewPanePlacement::default(),
        false, // start_suppressed
        ClientTabIndexOrPaneId::ClientId(client_id),
        None,  // completion_tx
        false, // set_blocking
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Use DumpScreen to verify the pane received the bytes
    let cli_action = CliAction::DumpScreen {
        path: Some(PathBuf::from("/tmp/dump_early_bytes")),
        full: true,
        pane_id: None,
        ansi: false,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let filesystem = mock_screen.os_input.fake_filesystem.lock().unwrap();
    let dumped = filesystem
        .values()
        .next()
        .expect("DumpScreen should have written a file");
    assert!(
        dumped.contains("hello"),
        "Pane should contain the buffered output 'hello', but got: {:?}",
        dumped
    );
}

// =====================================================================
// Host-reply forwarding (CSI 2031)
//
// These tests exercise the token-lifecycle API on `Screen` directly —
// no route.rs, no thread spawn, no client. The harness plugs real
// `to_server` / `to_pty_writer` channels into the Bus so the forward
// dispatch (→ `ServerInstruction::ForwardQueryToHost`) and the reply
// write (→ `PtyWriteInstruction::Write`) can be asserted.
// =====================================================================

struct ForwardCapture {
    server_rx: Receiver<(ServerInstruction, ErrorContext)>,
    pty_writer_rx: Receiver<(PtyWriteInstruction, ErrorContext)>,
}

impl ForwardCapture {
    /// Drain every pending `ServerInstruction::ForwardQueryToHost` and
    /// return them as `(token, query_bytes)` pairs. Other variants are
    /// dropped — the forward path only ever emits this one.
    fn drain_forward_queries(&self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.server_rx.try_recv() {
            if let ServerInstruction::ForwardQueryToHost(token, bytes) = instr {
                out.push((token, bytes));
            }
        }
        out
    }

    /// Drain every pending `PtyWriteInstruction::Write`, returning
    /// `(bytes, terminal_id)` — the two fields the reply path sets.
    fn drain_pty_writes(&self) -> Vec<(Vec<u8>, u32)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.pty_writer_rx.try_recv() {
            if let PtyWriteInstruction::Write(bytes, terminal_id, _) = instr {
                out.push((bytes, terminal_id));
            }
        }
        out
    }

    /// Drain every pending `ServerInstruction::KeyPassthroughChanged`,
    /// returning the `notify_guest` flag for each. Other variants are
    /// dropped.
    fn drain_key_passthrough_notify_flags(&self) -> Vec<bool> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.server_rx.try_recv() {
            if let ServerInstruction::KeyPassthroughChanged(_, _, _, _, _, notify_guest) = instr {
                out.push(notify_guest);
            }
        }
        out
    }
}

fn create_new_screen_with_forward_capture(size: Size) -> (Screen, ForwardCapture) {
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();

    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));

    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mut mode_info = ModeInfo::default();
    mode_info.session_name = Some("zellij-test".into());
    let draw_pane_frames = PaneFrameStyle::None;
    let auto_layout = true;
    let session_is_mirrored = true;
    let copy_options = CopyOptions::default();
    let default_layout = Box::new(Layout::default());
    let default_layout_name = None;
    let default_shell = PathBuf::from("my_default_shell");
    let session_serialization = true;
    let serialize_pane_viewport = false;
    let scrollback_lines_to_serialize = None;
    let layout_dir = None;
    let debug = false;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let arrow_fonts = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let stacked_resize = true;
    let web_sharing = WebSharing::Off;
    let web_server_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    let web_server_port = 8080;
    let visual_bell = true;
    let screen = Screen::new(
        bus,
        &client_attributes,
        max_panes,
        mode_info,
        draw_pane_frames,
        auto_layout,
        session_is_mirrored,
        copy_options,
        debug,
        default_layout,
        default_layout_name,
        default_shell,
        session_serialization,
        serialize_pane_viewport,
        scrollback_lines_to_serialize,
        styled_underlines,
        osc8_hyperlinks,
        arrow_fonts,
        layout_dir,
        explicitly_disable_kitty_keyboard_protocol,
        true, // support_kitty_graphics_protocol
        stacked_resize,
        false,
        None,
        false,
        web_sharing,
        true,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,
        true,
        visual_bell,
        false, // focus_follows_mouse
        false, // mouse_click_through
        web_server_ip,
        web_server_port,
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    (
        seed_first_client_size(screen, size),
        ForwardCapture {
            server_rx,
            pty_writer_rx,
        },
    )
}

// Convenience constructors for the forwarding tests — all callers
// want a fresh `HostQuery` value with the default terminator.
use crate::host_query::{HostQuery, OscTerminator};

fn bg_query() -> HostQuery {
    HostQuery::DefaultBackground {
        terminator: OscTerminator::St,
    }
}
fn fg_query() -> HostQuery {
    HostQuery::DefaultForeground {
        terminator: OscTerminator::St,
    }
}
fn fg_query_bel() -> HostQuery {
    HostQuery::DefaultForeground {
        terminator: OscTerminator::Bel,
    }
}
fn palette_query(index: u8) -> HostQuery {
    HostQuery::PaletteRegister {
        index,
        terminator: OscTerminator::St,
    }
}

#[test]
fn forward_host_query_when_idle_dispatches_immediately() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(42);
    let query = bg_query();

    let token = screen.forward_host_query(pane_id, query.clone());

    assert_eq!(
        screen.forward_in_flight_token,
        Some(token),
        "slot must flip to in-flight for the dispatched token"
    );
    assert_eq!(
        screen
            .pending_forwarded_queries
            .get(&token)
            .map(|e| e.pane_id),
        Some(pane_id),
        "token→pane mapping must be populated"
    );
    let forwards = capture.drain_forward_queries();
    assert_eq!(forwards.len(), 1, "exactly one forward dispatched");
    assert_eq!(
        forwards[0],
        (token, query.to_query_bytes()),
        "wire bytes must be derived from the HostQuery"
    );
}

#[test]
fn forward_host_query_when_busy_queues_instead_of_dispatching() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let first_pane = PaneId::Terminal(1);
    let second_pane = PaneId::Terminal(2);

    let first_token = screen.forward_host_query(first_pane, bg_query());
    let second_token = screen.forward_host_query(second_pane, fg_query());

    // The first call dispatched; the second waits in the queue. Only
    // the first token should be in the map; the second lives in
    // `forward_queue`.
    let forwards = capture.drain_forward_queries();
    assert_eq!(forwards.len(), 1, "second call must not dispatch yet");
    assert_eq!(forwards[0].0, first_token);
    assert_eq!(screen.forward_queue.len(), 1);
    assert_eq!(screen.forward_queue[0].token, second_token);
    assert_eq!(screen.forward_queue[0].pane_id, second_pane);
}

#[test]
fn handle_reply_writes_to_pane_pty_and_releases_slot() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let token = screen.forward_host_query(pane_id, bg_query());
    let _ = capture.drain_forward_queries(); // discard the dispatch

    let reply = b"\x1b]11;rgb:1111/2222/3333\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, reply.clone())
        .expect("handler must not fail");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "one pty write expected");
    assert_eq!(writes[0], (reply, 7));
    assert!(
        screen.forward_in_flight_token.is_none(),
        "slot released so the next queued forward can dispatch"
    );
    assert!(
        screen.pending_forwarded_queries.get(&token).is_none(),
        "token entry must have been removed from the map"
    );
}

#[test]
fn handle_reply_dispatches_next_queued_forward() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let first_pane = PaneId::Terminal(1);
    let second_pane = PaneId::Terminal(2);
    let first_token = screen.forward_host_query(first_pane, bg_query());
    let second_token = screen.forward_host_query(second_pane, fg_query());
    // First dispatch already emitted; drop it.
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(first_token, b"reply".to_vec())
        .expect("ok");

    // The queued second forward must now dispatch, and the map now
    // carries the second token → second pane.
    let forwards = capture.drain_forward_queries();
    assert_eq!(forwards.len(), 1, "next queued forward must dispatch");
    assert_eq!(forwards[0].0, second_token);
    assert!(screen.forward_queue.is_empty());
    assert_eq!(screen.forward_in_flight_token, Some(second_token));
    assert_eq!(
        screen
            .pending_forwarded_queries
            .get(&second_token)
            .map(|e| e.pane_id),
        Some(second_pane)
    );
}

#[test]
fn clear_nested_guest_does_not_notify_guest_focus_lost_on_teardown() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let client_id = 1;
    screen
        .nested_guest_choices
        .insert((client_id, pane_id), super::NestedGuestChoice::Descend);

    screen.clear_nested_guest(pane_id);

    let notify_flags = capture.drain_key_passthrough_notify_flags();
    assert_eq!(
        notify_flags,
        vec![false],
        "teardown clear must emit KeyPassthroughChanged with notify_guest=false so no FocusLost \
         frame is written to the exiting guest pane"
    );
}

#[test]
fn remove_client_notifies_guest_focus_lost_on_live_ascend() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let client_id = 1;
    screen
        .nested_guest_choices
        .insert((client_id, pane_id), super::NestedGuestChoice::Descend);

    screen.remove_client(client_id).expect("remove_client ok");

    let notify_flags = capture.drain_key_passthrough_notify_flags();
    assert_eq!(
        notify_flags,
        vec![true],
        "a live client leaving a still-alive guest must emit notify_guest=true so the guest is \
         told it lost focus"
    );
}

#[test]
fn suspend_nested_guest_preserves_choices_for_later_revival() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let client_id = 1;
    screen
        .nested_guest_choices
        .insert((client_id, pane_id), super::NestedGuestChoice::Descend);
    screen
        .nested_guest_tracker
        .on_announce(pane_id, std::time::Instant::now());

    screen.suspend_nested_guest(pane_id);

    assert!(
        screen
            .nested_guest_choices
            .contains_key(&(client_id, pane_id)),
        "suspend must preserve the client's descend choice so a re-announcing guest can be revived \
         into the exact prior state"
    );
}

#[test]
fn clear_nested_guest_discards_choices_unlike_suspend() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, _capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let client_id = 1;
    screen
        .nested_guest_choices
        .insert((client_id, pane_id), super::NestedGuestChoice::Descend);

    screen.clear_nested_guest(pane_id);

    assert!(
        !screen
            .nested_guest_choices
            .contains_key(&(client_id, pane_id)),
        "a full teardown (Bye/ClosePane) must discard choices, distinguishing it from suspend"
    );
}

#[test]
fn handle_reply_with_unknown_token_is_silent_noop() {
    // Token not in the map AND not the in-flight token: the handler
    // must not panic, must not write any bytes, and crucially must
    // NOT release the slot — the actually-in-flight forward still
    // owns it. (See `late_timeout_after_real_reply_does_not_clobber`
    // for the race scenario this guard prevents.)
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let first_pane = PaneId::Terminal(1);
    let second_pane = PaneId::Terminal(2);
    let first_token = screen.forward_host_query(first_pane, bg_query());
    let _second_token = screen.forward_host_query(second_pane, fg_query());
    let _ = capture.drain_forward_queries();

    // Reply for a stale / unknown token (neither first nor second).
    screen
        .handle_forwarded_reply_from_host(9999, b"dropped".to_vec())
        .expect("unknown tokens must not error");

    assert!(
        capture.drain_pty_writes().is_empty(),
        "unknown token must not produce any pty write"
    );
    assert_eq!(
        screen.forward_in_flight_token,
        Some(first_token),
        "in-flight token must still be the original"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "stale reply must not advance the queue"
    );
}

#[test]
fn late_timeout_after_real_reply_does_not_clobber_next_in_flight() {
    // The race the token-equality guard exists to prevent:
    //   1. dispatch token A (slot in-flight = A; A's timer is sleeping).
    //   2. real reply for A arrives → handler releases slot, dispatches
    //      queued token B → slot in-flight = B; B's timer is sleeping.
    //   3. A's server-side timeout fires after the real reply, sending
    //      an empty `ForwardedReplyFromHost { token: A, reply_bytes: [] }`.
    //
    // Without the guard, step 3 would clear the slot for token B and
    // pop the next queued forward, clobbering an actively-in-flight
    // request. The guard makes the late timeout a no-op.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_a = PaneId::Terminal(1);
    let pane_b = PaneId::Terminal(2);
    let pane_c = PaneId::Terminal(3);
    let token_a = screen.forward_host_query(pane_a, bg_query());
    let token_b = screen.forward_host_query(pane_b, fg_query());
    let token_c = screen.forward_host_query(pane_c, palette_query(5));
    let _ = capture.drain_forward_queries();

    // Real reply for A arrives → slot moves to B.
    screen
        .handle_forwarded_reply_from_host(token_a, b"real-A".to_vec())
        .expect("ok");
    let dispatched = capture.drain_forward_queries();
    assert_eq!(dispatched.len(), 1, "B must dispatch on A's release");
    assert_eq!(dispatched[0].0, token_b);
    assert_eq!(screen.forward_in_flight_token, Some(token_b));
    // Drain the real reply's pty write so the late-timeout assertion
    // below only sees writes (or absence thereof) caused by step 3.
    let real_writes = capture.drain_pty_writes();
    assert_eq!(real_writes.len(), 1, "real reply should write once");

    // Late timeout for A fires (server-side timer woke up after the
    // real reply already advanced the queue).
    screen
        .handle_forwarded_reply_from_host(token_a, Vec::new())
        .expect("late timeout must be a no-op, not an error");

    assert_eq!(
        screen.forward_in_flight_token,
        Some(token_b),
        "B's slot must NOT be released by A's late timeout"
    );
    assert_eq!(
        screen.forward_queue.front().map(|p| p.token),
        Some(token_c),
        "C must still be queued — A's late timeout must not have popped it"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "no spurious dispatch from a late timeout"
    );
    assert!(
        capture.drain_pty_writes().is_empty(),
        "no synthetic write to any pane from a late timeout"
    );
}

#[test]
fn timeout_for_in_flight_token_releases_slot_with_cache_fallback() {
    // The non-racing case: server-side timeout fires while the token
    // is still in flight (no client reply ever arrived — the old-client
    // compatibility path). The handler must synthesize a cache-derived
    // reply for the pane, release the slot, and dispatch the next
    // queued forward. (Identical externally to the empty-reply
    // cache-fallback case, since the timer fires by sending an empty
    // reply for the in-flight token.)
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.update_terminal_background_color("rgb:1010/2020/3030".to_string());
    let pane = PaneId::Terminal(11);
    let queued_pane = PaneId::Terminal(12);
    let token = screen.forward_host_query(pane, bg_query());
    let queued_token = screen.forward_host_query(queued_pane, fg_query());
    let _ = capture.drain_forward_queries();

    // Simulate the timeout firing: empty reply for the in-flight token.
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "cache fallback writes one reply");
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}]11;rgb:1010/2020/3030\u{1b}\\",
    );
    assert_eq!(
        screen.forward_in_flight_token,
        Some(queued_token),
        "queued forward dispatched on slot release"
    );
}

#[test]
fn token_counter_wraps_skipping_sentinel() {
    // `next_forward_token == u32::MAX` → first allocation yields
    // `u32::MAX`, the counter then wraps to 0 which is the reserved
    // sentinel and is skipped, so the next allocation yields 1.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.next_forward_token = u32::MAX;

    let t1 = screen.forward_host_query(PaneId::Terminal(1), bg_query());
    // Clear the in-flight slot so the next call actually allocates
    // (the queueing branch would still allocate, but we want the
    // dispatch branch here).
    screen
        .handle_forwarded_reply_from_host(t1, b"r".to_vec())
        .expect("ok");
    let _ = capture.drain_forward_queries();
    let _ = capture.drain_pty_writes();

    let t2 = screen.forward_host_query(PaneId::Terminal(2), fg_query());
    assert_eq!(t1, u32::MAX, "first token should land on u32::MAX");
    assert_eq!(
        t2, 1,
        "sentinel 0 must be skipped; next allocation wraps to 1"
    );
}

#[test]
fn plugin_pane_reply_is_dropped_without_write() {
    // Plugin panes never emit whitelisted host queries in production
    // code, but if a token → PaneId::Plugin mapping ever lands in the
    // map (via tests or future misuse), the handler must drop the
    // reply rather than routing it to the pty writer (plugin panes
    // don't have a pty).
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.pending_forwarded_queries.insert(
        77,
        super::PendingForwardEntry {
            pane_id: PaneId::Plugin(42),
            query: bg_query(),
        },
    );
    screen.forward_in_flight_token = Some(77);

    screen
        .handle_forwarded_reply_from_host(77, b"\x1b]11;rgb:0/0/0\x1b\\".to_vec())
        .expect("ok");

    assert!(
        capture.drain_pty_writes().is_empty(),
        "plugin-pane token must not produce a pty write"
    );
    assert!(
        screen.pending_forwarded_queries.get(&77).is_none(),
        "map entry must still be cleared even when the reply is dropped"
    );
    assert!(
        screen.forward_in_flight_token.is_none(),
        "slot still released"
    );
}

// =====================================================================
// Cache-fallback synthesis: empty reply → answer from Screen's caches
// =====================================================================

#[test]
fn empty_reply_falls_back_to_cached_background() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(9);
    screen.update_terminal_background_color("rgb:1010/2020/3030".to_string());
    let token = screen.forward_host_query(pane, bg_query());
    let _ = capture.drain_forward_queries();

    // Empty reply — the client couldn't or didn't answer.
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "synthesis must write exactly one reply");
    let (bytes, terminal_id) = &writes[0];
    assert_eq!(*terminal_id, 9);
    assert_eq!(
        std::str::from_utf8(bytes).unwrap(),
        "\u{1b}]11;rgb:1010/2020/3030\u{1b}\\",
    );
}

#[test]
fn empty_reply_falls_back_to_cached_foreground_with_bel_terminator() {
    // Query used BEL; reply must mirror the same terminator.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(1);
    screen.update_terminal_foreground_color("rgb:dcdc/dcdc/dcdc".to_string());
    let token = screen.forward_host_query(pane, fg_query_bel());
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}]10;rgb:dcdc/dcdc/dcdc\u{7}",
    );
}

#[test]
fn empty_reply_falls_back_to_cached_pixel_dimensions() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(1);
    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 19,
                width: 9,
            }),
            text_area_size: Some(SizeInPixels {
                height: 608,
                width: 931,
            }),
        },
    );

    // CSI 14t — text-area pixels.
    let token = screen.forward_host_query(pane, HostQuery::TextAreaPixelSize);
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let writes = capture.drain_pty_writes();
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}[4;608;931t"
    );

    // CSI 16t — cell size.
    let token = screen.forward_host_query(pane, HostQuery::CharacterCellPixelSize);
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let writes = capture.drain_pty_writes();
    assert_eq!(std::str::from_utf8(&writes[0].0).unwrap(), "\u{1b}[6;19;9t");
}

#[test]
fn empty_reply_falls_back_to_cached_palette_register() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(1);
    screen.update_terminal_color_registers(vec![(42, "rgb:abab/cdcd/efef".to_string())]);
    let token = screen.forward_host_query(pane, palette_query(42));
    let _ = capture.drain_forward_queries();

    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        std::str::from_utf8(&writes[0].0).unwrap(),
        "\u{1b}]4;42;rgb:abab/cdcd/efef\u{1b}\\",
    );
}

#[test]
fn empty_reply_with_no_cache_writes_empty() {
    // No background override, no palette, no pixel dims: synthesis
    // returns empty and the pane receives an empty write — the app
    // decides what to do with "host declined".
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(1);

    // Default Palette::fg is EightBit(0), not Rgb — synthesis refuses.
    let token = screen.forward_host_query(pane, fg_query());
    let _ = capture.drain_forward_queries();
    screen
        .handle_forwarded_reply_from_host(token, Vec::new())
        .expect("ok");
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "still writes — the write is empty bytes");
    assert!(
        writes[0].0.is_empty(),
        "no rgb cache → synthesis returns empty"
    );
}

#[test]
fn non_empty_reply_bypasses_synthesis() {
    // A real reply from the host must be passed through verbatim —
    // we must not second-guess it with cached state.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane = PaneId::Terminal(1);
    screen.update_terminal_background_color("rgb:0000/0000/0000".to_string());
    let token = screen.forward_host_query(pane, bg_query());
    let _ = capture.drain_forward_queries();

    let real = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, real.clone())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0].0, real,
        "real reply must be forwarded verbatim, not replaced by cached bg"
    );
}

// =====================================================================
// (CSI 2031 / DSR 997) (dark/light theme changes)
// =====================================================================

struct ThemeCapture {
    plugin_rx: Receiver<(PluginInstruction, ErrorContext)>,
    pty_writer_rx: Receiver<(PtyWriteInstruction, ErrorContext)>,
}

impl ThemeCapture {
    fn drain_plugin_updates(&self) -> Vec<(Option<u32>, Option<ClientId>, Event)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.plugin_rx.try_recv() {
            if let PluginInstruction::Update(updates) = instr {
                out.extend(updates);
            }
        }
        out
    }

    fn drain_plugin_events(&self) -> Vec<Event> {
        self.drain_plugin_updates()
            .into_iter()
            .map(|(_, _, event)| event)
            .collect()
    }
    fn drain_pty_writes(&self) -> Vec<(Vec<u8>, u32)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.pty_writer_rx.try_recv() {
            if let PtyWriteInstruction::Write(bytes, terminal_id, _) = instr {
                out.push((bytes, terminal_id));
            }
        }
        out
    }
}

fn create_new_screen_with_theme_capture(size: Size) -> (Screen, ThemeCapture) {
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();

    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));

    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let mut mode_info = ModeInfo::default();
    mode_info.session_name = Some("zellij-test".into());
    let copy_options = CopyOptions::default();
    let default_layout = Box::new(Layout::default());
    let default_shell = PathBuf::from("my_default_shell");
    let web_sharing = WebSharing::Off;
    let web_server_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    let web_server_port = 8080;
    let screen = Screen::new(
        bus,
        &client_attributes,
        None,
        mode_info,
        PaneFrameStyle::None,
        true,
        true,
        copy_options,
        false,
        default_layout,
        None,
        default_shell,
        true,
        false,
        None,
        true,
        true,
        true,
        None,
        false,
        true,
        true,
        false,
        None,
        false,
        web_sharing,
        true,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,
        true,
        true,
        false,
        false,
        web_server_ip,
        web_server_port,
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    (
        seed_first_client_size(screen, size),
        ThemeCapture {
            plugin_rx,
            pty_writer_rx,
        },
    )
}

fn add_hidden_global_plugin(screen: &mut Screen, plugin_id: u32) {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let pane = PaneFactory::plugin(
        plugin_id,
        Default::default(),
        screen.bus.senders.to_plugin.clone().unwrap(),
        "global plugin".to_owned(),
        "global plugin".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().keys().copied().collect(),
        screen.style,
        None,
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    screen
        .global_panes
        .insert(
            pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(1),
            },
            true,
        )
        .unwrap();
}

#[test]
fn reconfigure_sends_global_plugin_the_updated_client_mode_info_once() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    screen.mode_info.insert(1, ModeInfo::default());
    add_hidden_global_plugin(&mut screen, 77);
    capture.drain_plugin_updates();

    screen
        .reconfigure(
            Keybinds::default(),
            InputMode::Locked,
            Styling::default(),
            true,
            None,
            PaneFrameStyle::None,
            None,
            None,
            false,
            true,
            false,
            true,
            true,
            false,
            None,
            true,
            true,
            true,
            true,
            false,
            false,
            true,
            DEFAULT_WORD_SEPARATORS.to_owned(),
            NestedSessionHandling::default(),
            1,
        )
        .unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(77), Some(1), Event::ModeUpdate(mode_info)) => Some(mode_info),
                _ => None,
            },
        )
        .collect();
    assert_eq!(mode_infos.len(), 1);
    assert_eq!(mode_infos[0].base_mode, Some(InputMode::Locked));
    assert!(mode_infos[0].capabilities.arrow_fonts);
    assert!(mode_infos[0].style.hide_session_name);
}

#[test]
fn group_marking_updates_global_plugin_mode_info_once() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, 0);
    add_hidden_global_plugin(&mut screen, 77);
    capture.drain_plugin_updates();

    screen.toggle_group_marking(1).unwrap();

    let mode_infos: Vec<_> = capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(
            |(plugin_id, client_id, event)| match (plugin_id, client_id, event) {
                (Some(77), Some(1), Event::ModeUpdate(mode_info)) => Some(mode_info),
                _ => None,
            },
        )
        .collect();
    assert_eq!(mode_infos.len(), 1);
    assert_eq!(mode_infos[0].currently_marking_pane_group, Some(true));
}

#[test]
fn host_theme_first_update_emits_plugin_event() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("update ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(zellij_utils::data::HostTerminalThemeMode::Dark)
        )),
        "Event::HostTerminalThemeChanged(Dark) must be fanned out, got: {:?}",
        events
    );
    assert_eq!(
        screen.host_terminal_theme_mode,
        Some(zellij_utils::data::HostTerminalThemeMode::Dark),
        "stored mode must be updated"
    );
}

#[test]
fn host_theme_dedupes_duplicate_mode() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("first ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("second ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.is_empty(),
        "duplicate mode must not re-emit any plugin events, got: {:?}",
        events
    );
}

#[test]
fn host_theme_emits_again_on_mode_flip() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("first ok");
    let _ = capture.drain_plugin_events();

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Light)
        .expect("flip ok");

    let events = capture.drain_plugin_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::HostTerminalThemeChanged(zellij_utils::data::HostTerminalThemeMode::Light)
        )),
        "mode flip must re-emit the plugin event, got: {:?}",
        events
    );
}

#[test]
fn color_palette_mode_query_short_circuits_to_dark_reply() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Dark);

    let token = screen.forward_host_query(PaneId::Terminal(13), HostQuery::ColorPaletteMode);

    assert_eq!(
        token, 0,
        "ColorPaletteMode must return the sentinel token; no real forward was queued"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "must NOT forward to host — Zellij answers from cache"
    );
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), 13));
    assert!(
        screen.forward_in_flight_token.is_none(),
        "slot must remain free; short-circuit does not occupy the queue"
    );
}

#[test]
fn color_palette_mode_query_short_circuits_to_light_reply() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Light);

    let _ = screen.forward_host_query(PaneId::Terminal(4), HostQuery::ColorPaletteMode);

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), 4));
}

#[test]
fn color_palette_mode_query_falls_back_to_own_dark_theme_when_host_mode_unknown() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((0, 0, 0));

    let token = screen.forward_host_query(PaneId::Terminal(1), HostQuery::ColorPaletteMode);

    assert_eq!(
        token, 0,
        "ColorPaletteMode must return the sentinel token; no real forward was queued"
    );
    assert!(
        capture.drain_forward_queries().is_empty(),
        "must NOT forward to host — Zellij answers from its own effective theme"
    );
    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1, "exactly one pty reply expected");
    assert_eq!(writes[0], (b"\x1b[?997;1n".to_vec(), 1));
}

#[test]
fn color_palette_mode_query_falls_back_to_own_light_theme_when_host_mode_unknown() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    assert!(
        screen.host_terminal_theme_mode.is_none(),
        "precondition: no host mode learned yet"
    );
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((255, 255, 255));

    let _ = screen.forward_host_query(PaneId::Terminal(7), HostQuery::ColorPaletteMode);

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (b"\x1b[?997;2n".to_vec(), 7));
}

#[test]
fn color_palette_mode_query_prefers_known_host_mode_over_own_theme() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Light);
    screen.style.colors.text_unselected.background =
        zellij_utils::data::PaletteColor::Rgb((0, 0, 0));

    let _ = screen.forward_host_query(PaneId::Terminal(9), HostQuery::ColorPaletteMode);

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0],
        (b"\x1b[?997;2n".to_vec(), 9),
        "the host's announced mode must win over the local theme fallback"
    );
}

#[test]
fn color_palette_mode_query_skips_plugin_panes() {
    use crate::host_query::HostQuery;
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    screen.host_terminal_theme_mode = Some(zellij_utils::data::HostTerminalThemeMode::Dark);

    let _ = screen.forward_host_query(PaneId::Plugin(99), HostQuery::ColorPaletteMode);

    assert!(
        capture.drain_pty_writes().is_empty(),
        "plugin panes have no VT pty — they get Event::HostTerminalThemeChanged instead"
    );
}

#[test]
fn host_theme_no_pty_writes_when_no_panes_subscribed() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_theme_capture(size);

    screen
        .update_host_terminal_theme_mode(zellij_utils::data::HostTerminalThemeMode::Dark)
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert!(
        writes.is_empty(),
        "no panes exist (or none subscribed via CSI ?2031h), so no DSR forward is queued, got: {:?}",
        writes
    );
}

// =====================================================================
// Pause-on-forward state machine (pane-level)
//
// These tests exercise the per-pane forward-pause flag and the
// always-buffered PTY-input queue that preserves query/reply ordering
// when an app interleaves a sync-replied query (DA1, DSR, DECQRM) with
// a host-forwarded query (OSC 10/11/4, CSI 14t/16t).
//
// Single-buffer model:
//   - `handle_pty_bytes` always appends to `pending_pty_input`.
//   - When `forward_paused` is false, processing immediately drains
//     the queue byte-by-byte until either the queue empties or Grid
//     produces a forward-bound query.
//   - Once Tab arms the pause, subsequent calls just append; the
//     queue grows and waits.
//   - On resume, Tab clears the pause and calls handle_pty_bytes
//     with an empty slice; that triggers a fresh process pass over
//     the queued bytes.
// =====================================================================

use crate::panes::TerminalPane;
use crate::tab::Pane;
use zellij_utils::pane_size::PaneGeom;

fn new_terminal_pane_for_pause_test(pid: u32) -> TerminalPane {
    let mut geom = PaneGeom::default();
    geom.cols.set_inner(20);
    geom.rows.set_inner(10);
    TerminalPane::new(
        pid,
        geom,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            width: 8,
            height: 16,
        }))),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    )
}

#[test]
fn pane_buffers_pty_bytes_while_forward_paused() {
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.arm_forward_pause();
    assert!(pane.is_forward_paused());

    pane.handle_pty_bytes(b"hello".to_vec());
    pane.handle_pty_bytes(b"world".to_vec());

    let drained = pane.drain_pending_pty_input();
    assert_eq!(
        drained,
        b"helloworld".to_vec(),
        "while paused, every byte must accumulate in pending_pty_input \
         instead of being fed to vte"
    );
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "drain must clear the buffer"
    );
}

#[test]
fn pane_forward_in_vte_stops_processing_with_remainder_queued() {
    // App sends `OSC 10;? + after`. When vte's OSC dispatch runs and
    // Grid pushes the forward query, processing must stop, leaving
    // `after` in the queue. Those bytes will be replayed AFTER the
    // host reply has been written.
    let mut pane = new_terminal_pane_for_pause_test(1);
    let mut input = b"\x1b]10;?\x07".to_vec();
    input.extend_from_slice(b"after");
    pane.handle_pty_bytes(input);

    let queries = pane.drain_forwarded_queries();
    assert_eq!(queries.len(), 1, "exactly one forward must be queued");
    assert_eq!(
        queries[0],
        crate::host_query::HostQuery::DefaultForeground {
            terminator: crate::host_query::OscTerminator::Bel,
        }
    );

    let buffered = pane.drain_pending_pty_input();
    assert_eq!(
        buffered,
        b"after".to_vec(),
        "bytes after the forward must remain queued, not rendered"
    );
}

#[test]
fn pane_no_forward_drains_queue_empty() {
    // When the input contains no forward-bound query, processing runs
    // to completion and `pending_pty_input` ends empty.
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.handle_pty_bytes(b"plain text".to_vec());

    assert!(
        pane.drain_forwarded_queries().is_empty(),
        "no forward should have been produced"
    );
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "no forward → queue fully drained by processing"
    );
}

#[test]
fn pane_clear_forward_pause_reports_prior_state() {
    let mut pane = new_terminal_pane_for_pause_test(1);
    assert!(!pane.clear_forward_pause(), "not previously paused");
    pane.arm_forward_pause();
    assert!(pane.clear_forward_pause(), "was paused → returns true");
    assert!(
        !pane.is_forward_paused(),
        "after clearing, the pane must read as un-paused"
    );
}

#[test]
fn pane_paused_resumes_to_drain_queue() {
    // Simulate the resume cycle Tab runs:
    //   1. arm pause, app sends bytes (they accumulate)
    //   2. clear pause, drain queue, re-feed through handle_pty_bytes
    // The DA1 query buffered during step 1 must produce its sync
    // reply during step 2.
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.arm_forward_pause();
    pane.handle_pty_bytes(b"buffered\x1b[c".to_vec());

    // While paused, vte was never fed, so no replies were produced.
    assert!(
        pane.drain_forwarded_queries().is_empty(),
        "vte was not fed → no forwards produced"
    );
    assert!(
        pane.drain_messages_to_pty().is_empty(),
        "vte was not fed → no sync replies produced"
    );

    let was_paused = pane.clear_forward_pause();
    assert!(was_paused, "was paused");
    let buffered = pane.drain_pending_pty_input();
    pane.handle_pty_bytes(buffered);

    let sync_replies = pane.drain_messages_to_pty();
    assert!(
        !sync_replies.is_empty(),
        "after resume, queue is processed and Grid emits the DA1 reply"
    );
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "queue must be fully drained when no forward is in the stream"
    );
}

// =====================================================================
// End-to-end ordering through Screen+Tab integration
//
// These tests construct a real Screen with a real Tab and TerminalPane,
// drive PTY bytes in, and then exercise handle_forwarded_reply_from_host
// to assert the resulting PTY-write order on the captured channel.
// =====================================================================

#[test]
fn forwarded_reply_routes_through_tab_for_unpaused_pane() {
    // When a pane in a Tab receives a forward reply via
    // handle_forwarded_reply_from_host, the bytes must flow through
    // resume_pane_after_forward → Tab → write_to_pane_id_without_preprocessing
    // → PtyWriteInstruction::Write. The channel capture proves the
    // bytes reached the PTY writer with the right terminal id.
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, capture) = create_new_screen_with_forward_capture(size);
    let pane_id = PaneId::Terminal(7);
    let token = screen.forward_host_query(pane_id, bg_query());
    let _ = capture.drain_forward_queries();

    // No tab exists for this pane; the fallback path delivers the
    // bytes directly to the PTY writer. This still proves the
    // routing and stale-token guard interact correctly.
    let reply = b"\x1b]11;rgb:1111/2222/3333\x1b\\".to_vec();
    screen
        .handle_forwarded_reply_from_host(token, reply.clone())
        .expect("ok");

    let writes = capture.drain_pty_writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0], (reply, 7));
}

#[test]
fn paused_pane_with_da1_in_queue_emits_da1_after_resume() {
    // Simulates the user-reported ordering bug: app sends
    // `OSC 10;? + CSI c`. The OSC 10 forward must be dispatched
    // first, then the DA1 reply emitted only after the resume kicks
    // processing of the queued `\x1b[c`.
    let mut pane = new_terminal_pane_for_pause_test(1);
    let mut input = b"\x1b]10;?\x07".to_vec();
    input.extend_from_slice(b"\x1b[c");
    pane.handle_pty_bytes(input);

    // Forward emitted; DA1 still queued behind it.
    let queries = pane.drain_forwarded_queries();
    assert_eq!(queries.len(), 1, "OSC 10 forward emitted first");
    assert!(
        pane.drain_messages_to_pty().is_empty(),
        "DA1 reply must NOT be emitted yet — it is queued behind the forward"
    );

    // Tab arms pause, dispatches forward, eventually receives reply
    // and resumes. Resume = clear pause + drain queue + re-feed.
    pane.arm_forward_pause();
    // (host reply gets written to PTY via Tab; modelled here as no-op)
    pane.clear_forward_pause();
    let buffered = pane.drain_pending_pty_input();
    pane.handle_pty_bytes(buffered);

    let sync_replies = pane.drain_messages_to_pty();
    assert!(
        !sync_replies.is_empty(),
        "after resume, the queued CSI c is processed and Grid emits DA1"
    );
}

#[test]
fn empty_reply_with_paused_pane_drains_buffer_without_phantom_write() {
    // A pane that was paused on a ColorPaletteMode query while
    // host_terminal_theme_mode is unknown must still get unblocked,
    // but the spec requires NO bytes be written. The Tab-level
    // contract: a resume call with an empty payload skips the PTY
    // write yet still clears the pause and re-feeds the queue.
    let mut pane = new_terminal_pane_for_pause_test(1);
    pane.arm_forward_pause();
    pane.handle_pty_bytes(b"after".to_vec());
    assert!(pane.is_forward_paused());

    // Simulate Tab::resume_pane_after_forward with empty reply:
    //   - clear pause
    //   - drain queue + re-feed
    let was_paused = pane.clear_forward_pause();
    assert!(was_paused);
    let buffered = pane.drain_pending_pty_input();
    pane.handle_pty_bytes(buffered);
    assert!(!pane.is_forward_paused());
    assert!(
        pane.drain_pending_pty_input().is_empty(),
        "queue fully consumed by post-resume processing"
    );
}

fn create_non_mirrored_screen(size: Size) -> Screen {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let mut mode_info = ModeInfo::default();
    mode_info.session_name = Some("zellij-test".into());
    let screen = Screen::new(
        bus,
        &client_attributes,
        None, // max_panes
        mode_info,
        PaneFrameStyle::None,
        true,  // auto_layout
        false, // session_is_mirrored
        CopyOptions::default(),
        false, // debug
        Box::new(Layout::default()),
        None, // default_layout_name
        PathBuf::from("my_default_shell"),
        true,  // session_serialization
        false, // serialize_pane_viewport
        None,  // scrollback_lines_to_serialize
        true,  // styled_underlines
        true,  // osc8_hyperlinks
        true,  // arrow_fonts
        None,  // layout_dir
        false, // explicitly_disable_kitty_keyboard_protocol
        true,  // support_kitty_graphics_protocol
        true,  // stacked_resize
        false,
        None,
        false,
        WebSharing::Off,
        true, // advanced_mouse_actions
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,  // mouse_scroll_resize
        true,  // mouse_hover_effects
        true,  // visual_bell
        false, // focus_follows_mouse
        false, // mouse_click_through
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        8080,
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    seed_first_client_size(screen, size)
}

#[test]
fn new_tabs_are_created_at_the_size_of_the_client_creating_them() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(1, client_size);

    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        client_size,
        "A tab is created at the size of the client creating it"
    );
}

#[test]
fn applying_a_layout_to_an_existing_tab_keeps_its_viewer_derived_size() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(1, client_size);
    new_tab(&mut screen, 1, 0);

    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(2, None)],
            vec![],
            HashMap::new(),
            0,
            true,
            (1, false),
            None,
        )
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        client_size,
        "Applying a layout to an existing tab does not resize it away from its viewers"
    );
}

#[test]
fn recompute_tab_size_uses_lone_viewer_size() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);

    let client_id = 1;
    let client_size = Size { cols: 80, rows: 24 };
    screen.set_client_size(client_id, client_size);
    screen.recompute_tab_size(0).expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        client_size,
        "Tab adopts its lone viewer's size"
    );
}

#[test]
fn recompute_tab_size_takes_independent_min_across_axes() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(
        1,
        Size {
            cols: 200,
            rows: 24,
        },
    );
    screen.set_client_size(2, Size { cols: 80, rows: 60 });
    screen.recompute_tab_size(0).expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Tab size = (min cols across viewers, min rows across viewers)"
    );
}

#[test]
fn resize_to_clients_takes_the_min_across_clients_and_grows_back_on_detach() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(
        1,
        Size {
            cols: 100,
            rows: 50,
        },
    );
    screen.set_client_size(
        2,
        Size {
            cols: 120,
            rows: 40,
        },
    );
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.surface_size(),
        Size {
            cols: 100,
            rows: 40
        },
        "Surface size = (min cols across clients, min rows across clients)"
    );

    screen.remove_client(2).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.surface_size(),
        Size {
            cols: 100,
            rows: 50
        },
        "Screen grows back to the remaining client once the constraining one detaches"
    );
}

#[test]
fn resize_to_clients_reflows_global_panes_and_survives_a_render() {
    let mut screen = create_non_mirrored_screen(Size { cols: 50, rows: 12 });
    new_tab(&mut screen, 1, 0);
    insert_left_global_pane(&mut screen, 10);
    screen.set_client_size(1, Size { cols: 50, rows: 12 });
    screen.resize_to_clients().expect("TEST");

    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.resize_to_clients().expect("TEST");

    let global_pane_geom = screen
        .global_panes
        .panes()
        .next()
        .expect("TEST")
        .0
        .position_and_size();
    assert_eq!(
        global_pane_geom.rows.as_usize(),
        10,
        "Global pane reflows to the new screen height"
    );
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area(), Size { cols: 30, rows: 10 });
    assert_eq!(tab.get_display_area_origin(), (10, 0));

    // render_to_clients re-derives every tab's layout each frame, so a resize that did not reach
    // the shared surface would be snapped back here
    screen.render_to_clients().expect("TEST");
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area(), Size { cols: 30, rows: 10 });
    assert_eq!(tab.get_display_area_origin(), (10, 0));
}

#[test]
fn character_cell_size_is_derived_from_the_resized_screen() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, true, true);
    new_tab(&mut screen, 1, 0);

    // a font-size change: the terminal keeps its pixel size but reports fewer, larger cells
    screen.set_client_size(1, Size { cols: 40, rows: 12 });
    screen.resize_to_clients().expect("TEST");
    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 480,
            }),
            character_cell_size: None,
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 20,
            height: 40
        }),
        "Cell size is derived from the new screen size, not the one at session start"
    );
}

#[test]
fn resize_to_clients_keeps_every_tab_within_smallest_client_bounds() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The smaller client's tab uses the session bounds"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "A tab viewed only by the larger client is still restricted to the session bounds"
    );

    screen.set_client_size(
        2,
        Size {
            cols: 240,
            rows: 70,
        },
    );
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert!(screen
        .tabs
        .values()
        .all(|tab| tab.size == Size { cols: 80, rows: 24 }));

    screen.remove_client(1).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    assert!(screen.tabs.values().all(|tab| tab.size
        == Size {
            cols: 240,
            rows: 70
        }));
}

/// Two clients of different widths on different tabs, with a right global pane: every tab stays
/// inside the smallest client bounds, including across a render tick.
#[test]
fn global_panes_stay_within_shared_bounds_for_clients_on_different_tabs() {
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_non_mirrored_screen(Size {
        cols: 200,
        rows: 60,
    });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 10);
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    // client 1 stays on tab 1, client 2 moves to tab 0
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");

    let centers = |screen: &Screen| {
        (
            screen.tabs.get(&0).unwrap().get_display_area(),
            screen.tabs.get(&1).unwrap().get_display_area(),
        )
    };
    let expected = (Size { cols: 70, rows: 24 }, Size { cols: 70, rows: 24 });
    assert_eq!(
        centers(&screen),
        expected,
        "Both tabs use the shared session bounds beside the 10 column pane"
    );

    screen.render_to_clients().expect("TEST");
    assert_eq!(
        centers(&screen),
        expected,
        "A render tick preserves the shared session bounds"
    );

    // the pane itself is one grid at the same shared-session position for both clients
    screen.position_global_panes_for_client(2);
    let geom = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (geom.x, geom.cols.as_usize(), geom.rows.as_usize()),
        (70, 10, 24)
    );
    screen.position_global_panes_for_client(1);
    let geom = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (geom.x, geom.cols.as_usize(), geom.rows.as_usize()),
        (70, 10, 24)
    );
}

/// Global panes on both axes reserve their strips inside the shared session bounds.
#[test]
fn global_panes_on_both_axes_respect_the_shared_session_size() {
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_non_mirrored_screen(Size {
        cols: 200,
        rows: 60,
    });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Top, 3);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 10);
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area(),
        screen.tabs.get(&1).unwrap().get_display_area(),
        "Both tabs use the same surface around the global pane strips"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area(),
        Size { cols: 70, rows: 21 },
    );
}

/// Hit-testing a global pane uses the same session bounds for clients on different tabs.
#[test]
fn global_pane_hit_testing_uses_shared_session_bounds() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::position::Position;

    let mut screen = create_non_mirrored_screen(Size {
        cols: 200,
        rows: 60,
    });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 10);
    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 24,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    for client_id in [1, 2] {
        screen.position_global_panes_for_client(client_id);
        assert_eq!(
            screen.global_panes.pane_id_at(&Position::new(2, 75)),
            Some(PaneId::Terminal(99)),
            "Both clients hit the sidebar inside the shared bounds"
        );
        assert_eq!(
            screen.global_panes.pane_id_at(&Position::new(2, 155)),
            None,
            "Neither client has session content beyond the smaller client's width"
        );
    }
}

/// A read-only watcher observes the session without shrinking its shared surface.
#[test]
fn watcher_size_does_not_constrain_session_bounds() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen.add_watcher_client(2).expect("TEST");
    screen.set_watcher_size(2, Size { cols: 40, rows: 10 });
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "The smaller watcher viewport does not shrink the interactive session"
    );
}

/// A left/right pane's band is inset by the tallest chrome any tab declares, so a tab with less
/// chrome than that is left with strip rows nothing paints - those get filled, once per change.
#[test]
fn a_tab_with_less_chrome_than_the_session_max_gets_its_strip_filled() {
    use crate::global_panes::unowned_strip_rows;

    let client_id = 1;
    let rows = 20;
    let global_cols = 10;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    // a non-selectable pane on the bottom edge is chrome, same as the status-bar plugin - tab 0
    // gets one, tab 1 does not
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Down),
                borderless: Some(true),
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(3), false);
    insert_left_global_pane(&mut screen, global_cols);
    screen.resize_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");

    let (_, chrome_bottom) = screen.tabs.get(&0).unwrap().chrome_rows();
    assert!(chrome_bottom > 0, "tab 0 has no bottom chrome to speak of");
    assert_eq!(
        screen.max_chrome_rows(),
        (0, chrome_bottom),
        "the band is inset by the tallest chrome, which is tab 0's"
    );

    let rows_for = |screen: &Screen, tab_id: usize| {
        // tab 0 has no viewer, so it falls back to the shared surface just as the render tick does
        let tab_size = screen
            .tab_surface_size(tab_id)
            .unwrap_or_else(|| screen.surface_size());
        let layout = screen.global_layout_for(tab_size).unwrap();
        unowned_strip_rows(
            layout.center,
            &screen.strips_from_layout(&layout),
            screen.tabs.get(&tab_id).unwrap().chrome_rows(),
        )
    };
    assert_eq!(
        rows_for(&screen, 0),
        vec![],
        "tab 0's own chrome reaches every row the band leaves it"
    );
    assert_eq!(
        rows_for(&screen, 1),
        (rows - chrome_bottom..rows)
            .map(|y| (0, y, global_cols))
            .collect::<Vec<_>>(),
        "tab 1 has no status bar, so the rows the band gave up for tab 0's are its to fill"
    );

    // client 1 is on tab 1 (the last one created), so it is the one that saw the fill - and it is
    // memoised, or Output would re-emit it on every render tick and never let the session go quiet
    assert_eq!(
        screen.unowned_strip_fill.get(&client_id),
        Some(&rows_for(&screen, 1)),
        "the fill this client was sent is remembered, so the next render is a no-op"
    );
}

#[test]
fn switching_tabs_keeps_source_and_destination_at_session_bounds() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.recompute_tab_size(1).expect("TEST");
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Both clients on tab 1 → it sizes to the smaller"
    );

    screen.switch_active_tab(0, None, true, 1).expect("TEST");

    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Source tab remains inside the smallest client bounds"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab remains inside the smallest client bounds"
    );
}

#[test]
fn break_pane_to_new_tab_recomputes_source_and_destination() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 0 sized to client 1"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 1 is restricted by the smaller client on tab 0"
    );

    {
        let active_tab = screen.get_active_tab_mut(1).unwrap();
        active_tab
            .new_pane(
                PaneId::Terminal(99),
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(1),
                None,
            )
            .unwrap();
    }

    screen
        .break_pane_to_new_tab(Direction::Right, 1)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab shrinks to fit the arriving smaller viewer"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Source tab is empty — recompute is a no-op, last viewer-derived size is preserved"
    );
}

#[test]
fn moving_panes_between_tabs_with_focus_change_recomputes_both() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(1, Size { cols: 80, rows: 24 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 2).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 0 is restricted by the smaller client on tab 1"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Pre-condition: tab 1 sized to client 1"
    );

    let pane_to_move = PaneId::Terminal(42);
    {
        let active_tab = screen.get_active_tab_mut(1).unwrap();
        active_tab
            .new_pane(
                pane_to_move,
                None,
                None,
                false,
                true,
                NewPanePlacement::default(),
                Some(1),
                None,
            )
            .unwrap();
    }

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], 0, true, 1)
        .expect("TEST");
    assert!(
        outcome.succeeded(),
        "Pre-condition: the pane actually moved"
    );

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Destination tab shrinks to min(viewers) once the smaller client arrives"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Source tab is empty — last viewer-derived size is preserved"
    );
}

#[test]
fn detaching_client_grows_vacated_tab_back() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

    screen.set_client_size(
        1,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.set_client_size(2, Size { cols: 80, rows: 24 });
    screen.recompute_tab_size(0).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 24 },
        "Smaller viewer wins while both clients are on the tab"
    );

    screen.remove_client(2).expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "Tab grows back to fit the remaining viewer after the smaller one detaches"
    );
}

#[test]
fn closing_a_tab_resizes_the_tab_it_returns_to() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_new_screen(initial_size, true, true);
    let client_id = 1;
    let small_size = Size { cols: 80, rows: 24 };
    let large_size = Size {
        cols: 160,
        rows: 50,
    };

    screen.set_client_size(client_id, small_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        small_size,
        "Pre-condition: both tabs sized to the small client viewport"
    );

    screen.set_client_size(client_id, large_size);
    screen.recompute_tab_size(1).expect("TEST");
    assert_eq!(
        screen.tabs.get(&1).unwrap().size,
        large_size,
        "Pre-condition: the active tab follows the client resize"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        small_size,
        "Pre-condition: the background tab keeps its stale size until it is activated"
    );

    screen.close_tab_by_id(1).expect("TEST");

    assert_eq!(
        screen.get_active_tab(client_id).unwrap().position,
        0,
        "Focus returns to the previous tab"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        large_size,
        "The tab we return to adopts the current client size"
    );
}

fn add_second_pane_to_active_tab(screen: &mut Screen, pid: u32) {
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    active_tab
        .new_pane(
            PaneId::Terminal(pid),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();
}

fn decode_nested_frame(bytes: &[u8]) -> Option<zellij_utils::nested_session::NestedSessionMessage> {
    use zellij_utils::nested_session::{
        decode_base64, decode_payload, NESTED_FRAME_HEADER, NESTED_FRAME_TERMINATOR,
    };
    let encoded_payload = bytes
        .strip_prefix(NESTED_FRAME_HEADER)?
        .strip_suffix(NESTED_FRAME_TERMINATOR)?;
    decode_payload(&decode_base64(encoded_payload)?)
}

fn guest_announce_message() -> zellij_utils::nested_session::NestedSessionMessage {
    zellij_utils::nested_session::NestedSessionMessage::Announce {
        session_name: "guest-session".to_owned(),
        capabilities: vec![zellij_utils::nested_session::NestedSessionCapability::NestedControl],
    }
}

#[test]
pub fn nested_guest_announce_gets_announce_ack_with_ancestry() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(0),
            message: guest_announce_message(),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let announce_ack = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, 0, None) => decode_nested_frame(bytes),
            _ => None,
        })
        .expect("an announce_ack frame written to the guest pane");
    assert_eq!(
        announce_ack,
        zellij_utils::nested_session::NestedSessionMessage::AnnounceAck {
            ancestry: vec!["zellij-test".to_owned()],
            capabilities: vec![
                zellij_utils::nested_session::NestedSessionCapability::NestedControl
            ],
            descend_keys: vec![],
        }
    );
    assert!(received_background_jobs
        .lock()
        .unwrap()
        .iter()
        .any(|job| matches!(
            job,
            BackgroundJob::StartNestedGuestPing(PaneId::Terminal(0))
        )));
}

#[test]
pub fn nested_guest_announce_in_never_mode_still_completes_handshake() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.nested_session_handling = Some(NestedSessionHandling::Never);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(0),
            message: guest_announce_message(),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let announce_ack = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, 0, None) => decode_nested_frame(bytes),
            _ => None,
        })
        .expect("an announce_ack frame written to the guest pane even in never mode");
    assert!(matches!(
        announce_ack,
        zellij_utils::nested_session::NestedSessionMessage::AnnounceAck { .. }
    ));
    assert!(received_background_jobs
        .lock()
        .unwrap()
        .iter()
        .any(|job| matches!(
            job,
            BackgroundJob::StartNestedGuestPing(PaneId::Terminal(0))
        )));
}

#[test]
pub fn nested_guest_bye_stops_liveness_pings() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut mock_screen = MockScreen::new(size);
    let pty_writer_receiver = mock_screen.pty_writer_receiver.take().unwrap();
    let received_background_jobs = mock_screen.received_background_jobs.clone();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_writer_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyWriteInstruction::Exit,
        pty_writer_receiver
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(0),
            message: guest_announce_message(),
        });
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedSessionMessageFromPane {
            pane_id: PaneId::Terminal(0),
            message: zellij_utils::nested_session::NestedSessionMessage::Bye,
        });
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::NestedGuestPingTick {
            pane_id: PaneId::Terminal(0),
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_writer_thread, screen_thread]);
    let ping_frames_written = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|instruction| match instruction {
            PtyWriteInstruction::Write(bytes, 0, None) => matches!(
                decode_nested_frame(bytes),
                Some(zellij_utils::nested_session::NestedSessionMessage::Ping)
            ),
            _ => false,
        })
        .count();
    assert_eq!(ping_frames_written, 0);
    assert!(received_background_jobs
        .lock()
        .unwrap()
        .iter()
        .any(|job| matches!(job, BackgroundJob::StopNestedGuestPing(PaneId::Terminal(0)))));
}

#[test]
fn kitty_query_replies_ok_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, true);
    assert_eq!(screen.kitty_host_capabilities.borrow().get(&1), Some(&true));
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()]
    );
}

#[test]
fn kitty_query_replies_enotsupported_when_no_capable_client() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, false);
    assert_eq!(
        screen.kitty_host_capabilities.borrow().get(&1),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn kitty_query_is_ignored_when_the_protocol_is_disabled_in_the_config() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen_with_kitty_graphics(size, true, true, false);
    new_tab(&mut screen, 1, 0);
    screen.update_kitty_graphics_support(1, true);
    assert_eq!(
        screen.kitty_host_capabilities.borrow().get(&1),
        Some(&false),
        "a capable host must still be recorded as incapable when the protocol is disabled"
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    assert!(
        active_pane.drain_messages_to_pty().is_empty(),
        "a disabled protocol must not reply to queries at all"
    );
}

#[test]
fn kitty_support_recomputed_on_client_detach() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");
    screen.update_kitty_graphics_support(1, false);
    screen.update_kitty_graphics_support(2, true);
    screen.remove_client(2).expect("TEST");
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec());
    let replies = active_pane.drain_messages_to_pty();
    assert_eq!(replies.len(), 1);
    let reply = String::from_utf8(replies[0].clone()).unwrap();
    assert!(reply.starts_with("\x1b_Gi=31;ENOTSUPPORTED"));
    assert!(reply.ends_with("\x1b\\"));
}

#[test]
fn primary_da_advertises_sixel_when_capable_client_connected() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_sixel_support(1, true);
    assert_eq!(screen.sixel_host_capabilities.borrow().get(&1), Some(&true));
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b[?62;4;52c".to_vec()]
    );
}

#[test]
fn primary_da_omits_sixel_when_no_capable_client() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.update_sixel_support(1, false);
    assert_eq!(
        screen.sixel_host_capabilities.borrow().get(&1),
        Some(&false)
    );
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b[?62;52c".to_vec()]
    );
}

#[test]
fn sixel_support_recomputed_on_client_detach() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");
    screen.update_sixel_support(1, false);
    screen.update_sixel_support(2, true);
    screen.remove_client(2).expect("TEST");
    let active_tab = screen.get_active_tab_mut(1).unwrap();
    let active_pane = active_tab.get_active_pane_mut(1).unwrap();
    active_pane.handle_pty_bytes(b"\x1b[c".to_vec());
    assert_eq!(
        active_pane.drain_messages_to_pty(),
        vec![b"\x1b[?62;52c".to_vec()]
    );
}

#[test]
fn fit_disabled_excludes_web_client_from_min_size() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 20 });
    screen.set_client_size(2, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(2, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 20 },
        "Fit-disabled web client is excluded from the min-size loop; tab stays at desktop size"
    );
}

#[test]
fn fit_disabled_ignores_client_on_another_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(1, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "A client on another tab is not a reference: fit is forced on and the mobile size drives layout"
    );
}

#[test]
fn fit_disabled_tab_repins_on_desktop_resize() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "Pre-condition: shared tab pinned to the initial desktop reference size"
    );

    // Desktop client resizes; the fit-disabled tab must re-pin to its new size.
    screen.set_client_size(
        2,
        Size {
            cols: 120,
            rows: 40,
        },
    );
    screen.resize_to_clients().expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 120,
            rows: 40,
        },
        "Tab re-pins to the new desktop reference size after a desktop resize"
    );
}

#[test]
fn fit_disabled_reverts_when_reference_client_switches_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "Pre-condition: tab is shared, so fit-disabled is honored"
    );

    // The reference client leaves the tab without disconnecting.
    screen.switch_active_tab(1, None, true, 2).expect("TEST");
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        screen.mobile_web_prefs.get(&1).map(|prefs| prefs.fit) == Some(true),
        "Fit reverts once no client shares the tab, without any disconnect"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "The tab shrinks to the mobile size once nobody else is viewing it"
    );
}

#[test]
fn fit_disabled_allowed_when_only_other_client_is_web() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(1, /* is_web_client */ true)
        .expect("TEST");
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 20 });
    screen.set_client_size(2, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(2, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 20 },
        "A web client counts as a reference; fit-disabled is honored and the tab stays at the reference size"
    );
}

#[test]
fn fit_disabled_without_desktop_client_forces_enabled() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(1, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(1, /* single_pane */ false, /* fit */ false)
        .expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "With no desktop client, fit is forced enabled and the mobile size drives layout"
    );
}

#[test]
fn desktop_disconnect_reverts_fit_disabled() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 80, rows: 20 });
    screen.set_client_size(2, Size { cols: 40, rows: 10 });

    screen
        .set_mobile_render_preferences(2, /* single_pane */ false, /* fit */ false)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 80, rows: 20 },
        "Pre-condition: fit-disabled excludes the mobile client"
    );

    screen.remove_client(1).expect("TEST");

    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size { cols: 40, rows: 10 },
        "Desktop disconnect reverts the mobile client to fit-enabled; its size now drives layout"
    );
}

#[test]
fn single_pane_fullscreens_active_pane_and_reverts() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Single-pane fullscreens the active pane"
    );
    assert!(
        screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "Single-pane hides the tab bar and status bar"
    );

    screen
        .set_mobile_render_preferences(client, /* single_pane */ false, /* fit */ true)
        .expect("TEST");
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Disabling single-pane reverts the fullscreen"
    );
}

#[test]
fn focus_pane_by_id_is_per_client_and_does_not_steal_global_focus() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(1, None, true, 2).expect("TEST");

    assert_eq!(
        screen.get_active_pane_id(&2),
        Some(PaneId::Terminal(2)),
        "Pre-condition: client 2 is focused on pane 2 (its own tab)"
    );

    // Client 1 focuses the pane living on client 2's tab.
    screen
        .focus_pane_with_id(PaneId::Terminal(2), false, false, 1)
        .expect("TEST");

    assert_eq!(
        screen.get_active_pane_id(&1),
        Some(PaneId::Terminal(2)),
        "Client 1 now focuses the target pane"
    );
    assert_eq!(
        screen.active_tab_ids.get(&2).copied(),
        Some(1),
        "Client 2's active tab is unchanged by client 1's per-client focus"
    );
    assert_eq!(
        screen.get_active_pane_id(&2),
        Some(PaneId::Terminal(2)),
        "Client 2's focus is not stolen by client 1's per-client focus"
    );
}

#[test]
fn mobile_state_reports_single_pane_off_before_any_preference_is_sent() {
    let size = Size { cols: 80, rows: 20 };
    let (mut screen, messages) = create_new_screen_with_message_capture(size);
    new_tab(&mut screen, 1, 0);
    screen
        .add_client(2, /* is_web_client */ true)
        .expect("TEST");

    screen.log_and_report_session_state().expect("TEST");

    let msgs = messages.lock().unwrap();
    let payload = msgs
        .get(&2)
        .expect("TEST")
        .iter()
        .find_map(|msg| match msg {
            ServerToClientMsg::MobileState { payload } => Some(payload.clone()),
            _ => None,
        })
        .expect("A web client receives a MobileState push");

    assert!(
        !payload.render_prefs.single_pane,
        "A client that never sent preferences has nothing fullscreened, so single-pane must be reported off"
    );
    assert!(
        payload.render_prefs.fit,
        "Such a client participates in the tab min-size rule, so fit is reported on"
    );
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Nothing is fullscreened for a client that never sent preferences"
    );
}

#[test]
fn single_pane_demotes_after_desktop_unfullscreen() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    // Simulate a desktop-initiated un-fullscreen on the same tab.
    screen.tabs.get_mut(&0).unwrap().unset_fullscreen();
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Fullscreen cleared by the simulated desktop action"
    );

    screen.log_and_report_session_state().expect("TEST");

    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Single-pane yields to the un-fullscreen instead of re-asserting it"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&client)
            .map(|prefs| prefs.single_pane),
        Some(false),
        "The client is demoted out of single-pane mode and reports it"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&client)
            .and_then(|prefs| prefs.fullscreened_pane),
        None,
        "The tracked fullscreen pane is cleared on demotion"
    );
}

#[test]
fn single_pane_demotion_disables_fit_when_tab_is_shared() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.set_client_size(1, Size { cols: 40, rows: 10 });
    screen.set_client_size(
        2,
        Size {
            cols: 160,
            rows: 50,
        },
    );
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(1, /* single_pane */ true, /* fit */ true)
        .expect("TEST");

    // Simulate a desktop-initiated un-fullscreen on the shared tab.
    screen.tabs.get_mut(&0).unwrap().unset_fullscreen();
    screen.log_and_report_session_state().expect("TEST");

    assert_eq!(
        screen.mobile_web_prefs.get(&1).map(|prefs| prefs.fit),
        Some(false),
        "Demotion also disables fit so the shared tab is not shrunk to the mobile size"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().size,
        Size {
            cols: 160,
            rows: 50,
        },
        "The shared tab keeps the desktop size after the demotion"
    );
}

#[test]
fn single_pane_demotes_when_desktop_opens_a_pane() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let mobile_client = 1;
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(
            mobile_client,
            /* single_pane */ true,
            /* fit */ true,
        )
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    // The desktop client opens a pane in the shared tab, which drops fullscreen.
    screen
        .get_active_tab_mut(2)
        .unwrap()
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(2),
            None,
        )
        .expect("TEST");
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "The pane the desktop client opened stays visible"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.single_pane),
        Some(false),
        "The mobile client is demoted rather than hiding the new pane"
    );
}

#[test]
fn single_pane_follows_focus_to_newly_added_pane() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(2)),
        "Pre-condition: single-pane fullscreens the current active pane"
    );

    add_second_pane_to_active_tab(&mut screen, 3);
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "A pane added in single-pane mode keeps a pane fullscreened"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(3)),
        "The fullscreen follows focus to the newly added pane"
    );
    assert!(
        screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "The followed fullscreen still hides the tab bar and status bar"
    );
}

#[test]
fn single_pane_demotes_after_desktop_downgrades_to_regular_fullscreen() {
    let initial_size = Size { cols: 80, rows: 20 };
    let mut screen = create_new_screen(initial_size, true, true);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let client = 1;

    screen
        .set_mobile_render_preferences(client, /* single_pane */ true, /* fit */ true)
        .expect("TEST");
    let fullscreen_pane_id = screen.tabs.get(&0).unwrap().fullscreen_pane_id().unwrap();

    // A desktop client toggles regular fullscreen, downgrading the no-ui fullscreen.
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .toggle_pane_fullscreen(fullscreen_pane_id);
    screen.log_and_report_session_state().expect("TEST");

    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "The regular fullscreen the desktop client asked for is kept"
    );
    assert!(
        !screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "Single-pane does not re-assert the no-ui fullscreen"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&client)
            .map(|prefs| prefs.single_pane),
        Some(false),
        "The client is demoted out of single-pane mode"
    );
}

#[test]
fn single_pane_survives_the_mobile_client_opening_its_own_pane_in_a_shared_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    add_second_pane_to_active_tab(&mut screen, 2);
    let mobile_client = 1;
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(
            mobile_client,
            /* single_pane */ true,
            /* fit */ true,
        )
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    // The mobile client opens a pane of its own, which takes its focus.
    screen
        .get_active_tab_mut(mobile_client)
        .unwrap()
        .new_pane(
            PaneId::Terminal(3),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(mobile_client),
            None,
        )
        .expect("TEST");
    screen.log_and_report_session_state().expect("TEST");

    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.single_pane),
        Some(true),
        "A pane the mobile client opened itself does not demote it out of single-pane mode"
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(3)),
        "The fullscreen follows the mobile client to the pane it just opened"
    );
    assert!(
        screen.tabs.get(&0).unwrap().fullscreen_covers_ui(),
        "The followed fullscreen still hides the tab bar and status bar"
    );
    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.fit),
        Some(true),
        "Fit is untouched because there was no demotion"
    );
}

#[test]
fn single_pane_follows_the_mobile_client_into_a_new_tab() {
    let initial_size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(initial_size);
    new_tab(&mut screen, 1, 0);
    let mobile_client = 1;
    screen
        .add_client(2, /* is_web_client */ false)
        .expect("TEST");
    screen.switch_active_tab(0, None, true, 1).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

    screen
        .set_mobile_render_preferences(
            mobile_client,
            /* single_pane */ true,
            /* fit */ true,
        )
        .expect("TEST");
    assert!(
        screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "Pre-condition: single-pane fullscreen active"
    );

    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen
            .mobile_web_prefs
            .get(&mobile_client)
            .map(|prefs| prefs.single_pane),
        Some(true),
        "Opening a tab keeps the mobile client in single-pane mode"
    );
    assert!(
        !screen.tabs.get(&0).unwrap().is_fullscreen_active(),
        "The tab the mobile client left is restored for the desktop client"
    );
    assert_eq!(
        screen.tabs.get(&1).unwrap().fullscreen_pane_id(),
        Some(PaneId::Terminal(2)),
        "The pane of the new tab is fullscreened for the mobile client"
    );
    assert!(
        screen.tabs.get(&1).unwrap().fullscreen_covers_ui(),
        "The new tab's fullscreen still hides the tab bar and status bar"
    );
}

#[test]
fn attaching_web_client_lands_on_the_first_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    assert_eq!(
        screen.active_tab_ids.get(&1),
        Some(&1),
        "host client is on the second tab"
    );

    screen.add_client(2, true).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&2),
        Some(&0),
        "web client is on the first tab"
    );
}

#[test]
fn attaching_web_client_hides_the_floating_surface_of_the_first_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
        .unwrap();
    assert!(screen.tabs.get(&0).unwrap().are_floating_panes_visible());

    screen.add_client(2, true).expect("TEST");

    assert!(
        !screen.tabs.get(&0).unwrap().are_floating_panes_visible(),
        "the floating surface is hidden"
    );
    assert_eq!(
        screen.get_active_pane_id(&2),
        Some(PaneId::Terminal(1)),
        "web client is focused on a tiled pane"
    );
}

#[test]
fn attaching_web_client_does_not_touch_the_floating_surface_of_other_tabs() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&1)
        .unwrap()
        .new_floating_pane(PaneId::Terminal(3), None, None, false, true, None, None)
        .unwrap();

    screen.add_client(2, true).expect("TEST");

    assert!(
        screen.tabs.get(&1).unwrap().are_floating_panes_visible(),
        "the floating surface of the host tab is untouched"
    );
    assert_eq!(
        screen.get_active_pane_id(&1),
        Some(PaneId::Terminal(3)),
        "host client keeps its floating focus"
    );
}

#[test]
fn attaching_terminal_client_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&1)
        .unwrap()
        .new_floating_pane(PaneId::Terminal(3), None, None, false, true, None, None)
        .unwrap();

    screen.add_client(2, false).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&2),
        Some(&1),
        "terminal client is on the host tab"
    );
    assert!(
        screen.tabs.get(&1).unwrap().are_floating_panes_visible(),
        "the floating surface remains visible"
    );
}

#[test]
fn attaching_web_watcher_follows_the_host_tab() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    screen.add_watcher_client(2).expect("TEST");
    screen.add_client(2, true).expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&2),
        Some(&1),
        "watcher client mirrors the host tab"
    );
}

// =====================================================================================
// GP validation-phase regression tests (see global-pane-review-checklist.md).
// Retained from Phase 1 validation: the failing tests below demonstrate confirmed defects
// and must pass in their respective fix stages.
// - gp01_* (stage 2A): FAIL - resize below a fixed pane / percentage rounding to zero aborts
// - gp02_* (stage 2B): FAIL - moved floating panes drift by the global-pane origin on reflow
// - gp04_* (stage 3B): PASS - protects the strip-change frame reapplication
// - gp05_* (stage 4A): FAIL - borderless left/top panes' content is overwritten by separators
// - gp06_* (stage 4B): FAIL - same-edge global panes have no internal separator
// =====================================================================================

fn gp_insert_global_pane(
    screen: &mut Screen,
    id: u32,
    edge: zellij_utils::data::GlobalPaneEdge,
    size: PercentOrFixed,
    borderless: bool,
) {
    gp_try_insert_global_pane(screen, id, edge, size, borderless, false).unwrap()
}

fn gp_try_insert_global_pane(
    screen: &mut Screen,
    id: u32,
    edge: zellij_utils::data::GlobalPaneEdge,
    size: PercentOrFixed,
    borderless: bool,
    hidden: bool,
) -> Result<(), String> {
    use zellij_utils::data::GlobalPanePlacement;

    let mut global_pane = PaneFactory::terminal(
        id,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    if borderless {
        global_pane.set_borderless(true);
    }
    screen
        .global_panes
        .insert(global_pane, GlobalPanePlacement { edge, size }, hidden)
}

fn gp_drain_client_1_snapshot(
    server_receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    grid: &mut Grid,
) -> String {
    let mut snapshot = String::new();
    while let Ok((instruction, _)) = server_receiver.try_recv() {
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            if let Some(output) = rendered_clients.remove(&1) {
                snapshot = take_snapshot_and_cursor_coordinates(&output, grid).1;
            }
        }
    }
    snapshot
}

fn gp_line_content(snapshot_line: &str) -> &str {
    // grid debug lines are formatted as `00 (C): <content>`
    match snapshot_line.find("): ") {
        Some(index) => &snapshot_line[index + 3..],
        None => snapshot_line,
    }
}

#[test]
fn gp01_fixed_size_global_pane_keeps_rendering_when_terminal_shrinks() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 12);
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (12, 0)
    );

    let resize_result = screen.resize_to_screen(Size { cols: 12, rows: 10 });
    assert!(
        resize_result.is_ok(),
        "shrinking the terminal to the global pane width failed: {:?}",
        resize_result.err().map(|e| format!("{:#}", e))
    );
    let render_result = screen.render_to_clients();
    assert!(
        render_result.is_ok(),
        "rendering after the shrink failed: {:?}",
        render_result.err().map(|e| format!("{:#}", e))
    );

    screen
        .resize_to_screen(Size { cols: 40, rows: 10 })
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (12, 0)
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area(),
        Size { cols: 28, rows: 10 }
    );
}

#[test]
fn gp01_percent_global_pane_keeps_rendering_when_thickness_rounds_to_zero() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Percent(20),
        false,
    );
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (8, 0)
    );

    // 20% of 4 columns rounds down to zero, which the allocator rejects
    let resize_result = screen.resize_to_screen(Size { cols: 4, rows: 10 });
    assert!(
        resize_result.is_ok(),
        "shrinking until the percentage rounds to zero failed: {:?}",
        resize_result.err().map(|e| format!("{:#}", e))
    );
    let render_result = screen.render_to_clients();
    assert!(
        render_result.is_ok(),
        "rendering after the shrink failed: {:?}",
        render_result.err().map(|e| format!("{:#}", e))
    );
}

#[test]
fn gp02_moved_floating_pane_keeps_position_across_noop_reflows_beside_left_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::pane_size::Viewport;

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 10);
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (10, 0)
    );

    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(5)),
                ..Default::default()
            },
        )
        .expect("TEST");
    }
    let before = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(2))
        .unwrap()
        .position_and_size();
    assert_eq!(before.x, 20, "moved pane should sit at tab-local x=10");

    let display_area = Viewport {
        x: 10,
        y: 0,
        cols: 111,
        rows: 20,
    };
    for iteration in 0..3 {
        screen
            .tabs
            .get_mut(&0)
            .unwrap()
            .resize_whole_tab_to(display_area)
            .expect("TEST");
        let after = screen
            .tabs
            .get(&0)
            .unwrap()
            .get_pane_with_id(PaneId::Terminal(2))
            .unwrap()
            .position_and_size();
        assert_eq!(
            after.x,
            before.x,
            "floating pane x drifted on no-op reflow {}",
            iteration + 1
        );
        assert_eq!(
            after.y,
            before.y,
            "floating pane y drifted on no-op reflow {}",
            iteration + 1
        );
    }
}

#[test]
fn gp02_moved_floating_pane_keeps_position_across_noop_reflows_beneath_top_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::pane_size::Viewport;

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(4),
        false,
    );
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (0, 4)
    );

    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(2)),
                ..Default::default()
            },
        )
        .expect("TEST");
    }
    let before = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(2))
        .unwrap()
        .position_and_size();
    assert_eq!(before.y, 6, "moved pane should sit at tab-local y=2");

    let display_area = Viewport {
        x: 0,
        y: 4,
        cols: 121,
        rows: 16,
    };
    for iteration in 0..3 {
        screen
            .tabs
            .get_mut(&0)
            .unwrap()
            .resize_whole_tab_to(display_area)
            .expect("TEST");
        let after = screen
            .tabs
            .get(&0)
            .unwrap()
            .get_pane_with_id(PaneId::Terminal(2))
            .unwrap()
            .position_and_size();
        assert_eq!(
            after.x,
            before.x,
            "floating pane x drifted on no-op reflow {}",
            iteration + 1
        );
        assert_eq!(
            after.y,
            before.y,
            "floating pane y drifted on no-op reflow {}",
            iteration + 1
        );
    }
}

#[test]
fn gp02_moved_floating_pane_keeps_position_across_noop_reflows_with_left_and_top_global_panes() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::pane_size::Viewport;

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(10),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(4),
        false,
    );
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (10, 4)
    );

    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(2)),
                ..Default::default()
            },
        )
        .expect("TEST");
    }
    let before = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(2))
        .unwrap()
        .position_and_size();
    assert_eq!(
        (before.x, before.y),
        (20, 6),
        "moved pane should sit at tab-local (10, 2)"
    );

    let display_area = Viewport {
        x: 10,
        y: 4,
        cols: 111,
        rows: 16,
    };
    for iteration in 0..3 {
        screen
            .tabs
            .get_mut(&0)
            .unwrap()
            .resize_whole_tab_to(display_area)
            .expect("TEST");
        let after = screen
            .tabs
            .get(&0)
            .unwrap()
            .get_pane_with_id(PaneId::Terminal(2))
            .unwrap()
            .position_and_size();
        assert_eq!(
            after.x,
            before.x,
            "floating pane x drifted on no-op reflow {}",
            iteration + 1
        );
        assert_eq!(
            after.y,
            before.y,
            "floating pane y drifted on no-op reflow {}",
            iteration + 1
        );
    }
}

#[test]
fn gp04_placement_change_to_opposite_edge_updates_boundary_reservation_on_next_render() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 8);
    screen.render_to_clients().expect("TEST");

    screen
        .set_global_pane_placement(
            PaneId::Terminal(99),
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Right,
                size: PercentOrFixed::Fixed(8),
            },
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");

    let pane = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(1))
        .unwrap();
    let offset = pane.get_content_offset();
    assert_eq!(
        offset.right, 1,
        "tab pane now abuts the right global pane and must reserve its last column"
    );
    assert_eq!(
        offset.left, 0,
        "tab pane no longer abuts a left global pane"
    );
}

fn gp_fill_global_rows_left(screen: &mut Screen, rows: usize, cols: usize) {
    let mut content = Vec::new();
    for row in 0..rows {
        content.extend(std::iter::repeat(b'G').take(cols));
        if row + 1 < rows {
            content.extend_from_slice(b"\r\n");
        }
    }
    screen
        .global_panes
        .pane_mut(PaneId::Terminal(99))
        .unwrap()
        .handle_pty_bytes(content);
}

// `borderless` on a global pane means "no outer frame", not "no tab/global separator": the tab
// grows its boundary viewport over every visible strip and draws the separator one cell inside a
// left/top global pane, so a borderless pane reserves that cell exactly like a bordered one does.
// Neither layer can then overwrite the other, whichever renders last.
#[test]
fn gp05_borderless_left_global_pane_reserves_the_tab_separator_column() {
    use zellij_utils::data::{GlobalPaneEdge, ResizeStrategy};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size); // PaneFrameStyle::None
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        true,
    );
    // 8 columns wide, of which the last is reserved for the tab separator
    gp_fill_global_rows_left(&mut screen, 10, 7);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .get_pane_with_id_mut(PaneId::Terminal(1))
        .unwrap()
        .handle_pty_bytes(b"TTTTTTTTTTTTTTTTTTTT".to_vec());

    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let first_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // split the tab so the boundary layout changes (this also clears the host display, which
    // force-renders the global panes back on top)
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Down),
                borderless: None,
            },
            Some(1),
            None,
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let second_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // resize the top tab pane without clearing the host display - boundaries are re-emitted
    // while the borderless global pane has no new output, so they must not land inside its
    // content area
    screen
        .focus_pane_with_id(PaneId::Terminal(1), false, false, 1)
        .expect("TEST");
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .resize(
            1,
            ResizeStrategy {
                resize: Resize::Increase,
                direction: Some(Direction::Down),
                invert_on_boundaries: false,
            },
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let third_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // stable across all three renders: the global pane never writes column 7, the tab always draws
    // its separator there
    for (render, snapshot) in [
        ("first", &first_snapshot),
        ("second", &second_snapshot),
        ("third", &third_snapshot),
    ] {
        for row in 0..10 {
            let content = gp_line_content(snapshot.lines().nth(row).unwrap_or(""));
            assert_eq!(
                content.chars().take(7).collect::<String>(),
                "GGGGGGG",
                "{render} render, row {row}: the borderless left global pane must keep its 7 content \
                 columns, got: {content:?}"
            );
            // a separator or one of its junctions, never pane content
            assert!(
                matches!(content.chars().nth(7), Some('\u{2502}') | Some('\u{251c}')),
                "{render} render, row {row}: column 7 is reserved for the tab separator, \
                 got: {content:?}"
            );
        }
    }
}

#[test]
fn gp05_borderless_top_global_pane_reserves_the_tab_separator_row() {
    use zellij_utils::data::{GlobalPaneEdge, ResizeStrategy};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size); // PaneFrameStyle::None
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(3),
        true,
    );
    // 3 rows tall, of which the last is reserved for the tab separator
    let mut global_content = Vec::new();
    for row in 0..2 {
        global_content.extend(std::iter::repeat(b'G').take(40));
        if row + 1 < 2 {
            global_content.extend_from_slice(b"\r\n");
        }
    }
    screen
        .global_panes
        .pane_mut(PaneId::Terminal(99))
        .unwrap()
        .handle_pty_bytes(global_content);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .get_pane_with_id_mut(PaneId::Terminal(1))
        .unwrap()
        .handle_pty_bytes(b"TTTTTTTTTTTTTTTTTTTT".to_vec());

    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let first_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // split the tab so the boundary layout changes (this also clears the host display, which
    // force-renders the global panes back on top)
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Right),
                borderless: None,
            },
            Some(1),
            None,
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let second_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // resize the tab pane without clearing the host display - boundaries are re-emitted while the
    // borderless global pane has no new output, so they must not land inside its content row
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .resize(
            1,
            ResizeStrategy {
                resize: Resize::Increase,
                direction: Some(Direction::Left),
                invert_on_boundaries: false,
            },
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let third_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // stable across all three renders: the global pane never writes row 2, the tab always draws its
    // separator there
    for (render, snapshot) in [
        ("first", &first_snapshot),
        ("second", &second_snapshot),
        ("third", &third_snapshot),
    ] {
        for row in 0..2 {
            let content = gp_line_content(snapshot.lines().nth(row).unwrap_or(""));
            assert!(
                content.starts_with(&"G".repeat(40)),
                "{render} render: the borderless top global pane's content row {row} must not be \
                 overwritten by the tab separator, got: {content:?}"
            );
        }
        let separator_row = gp_line_content(snapshot.lines().nth(2).unwrap_or(""));
        assert!(
            !separator_row.contains('G') && separator_row.contains('\u{2500}'),
            "{render} render: row 2 is reserved for the tab separator, got: {separator_row:?}"
        );
    }
}

// Same-edge global panes have no separator geometry between them (only strips are grown over, they
// never contribute rects of their own), so they are rejected rather than half-supported.
#[test]
fn gp06_same_edge_global_panes_are_rejected() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size); // PaneFrameStyle::None
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen.render_to_clients().expect("TEST");
    let geom_before = screen
        .global_panes
        .pane(PaneId::Terminal(98))
        .unwrap()
        .position_and_size();

    // inserting a second visible pane on the occupied edge
    assert_eq!(
        gp_try_insert_global_pane(
            &mut screen,
            99,
            GlobalPaneEdge::Left,
            PercentOrFixed::Fixed(8),
            false,
            false,
        ),
        Err("a global pane is already docked to the left edge".to_owned())
    );
    assert!(screen.global_panes.pane(PaneId::Terminal(99)).is_none());

    // a free edge is still fine
    gp_insert_global_pane(
        &mut screen,
        97,
        GlobalPaneEdge::Right,
        PercentOrFixed::Fixed(8),
        false,
    );

    // a hidden pane may be parked on an occupied edge, but showing it is rejected
    gp_try_insert_global_pane(
        &mut screen,
        96,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
        true,
    )
    .expect("a hidden pane on an occupied edge is legal");
    let area = zellij_utils::pane_size::Viewport::from(size);
    assert_eq!(
        screen.global_panes.show(PaneId::Terminal(96), area),
        Err("a global pane is already docked to the left edge".to_owned())
    );

    // as is moving a visible pane onto an occupied edge
    assert_eq!(
        screen.global_panes.set_placement(
            PaneId::Terminal(97),
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(8),
            },
            area,
        ),
        Err("a global pane is already docked to the left edge".to_owned())
    );

    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen
            .global_panes
            .pane(PaneId::Terminal(98))
            .unwrap()
            .position_and_size(),
        geom_before,
        "the rejected placements left the docked pane untouched"
    );
}

/// `zellij action new-tab` against a session every client has detached from: nobody contributes a
/// size, so the tab has to fall back to the same last-known surface `surface_size` does. A 0x0 tab
/// has no room for a pane and the next render tick closes it.
#[test]
fn a_tab_created_with_no_clients_attached_gets_the_last_known_session_size() {
    let size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen.render_to_clients().expect("TEST");
    screen.remove_client(1).expect("TEST");

    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.tabs.get(&1).map(|tab| tab.size),
        Some(size),
        "A tab created while nothing is attached takes the size the panes were last laid out over"
    );
    screen.render_to_clients().expect("TEST");
    assert!(
        screen.tabs.contains_key(&1),
        "and it survives the render tick rather than being closed for having no selectable panes"
    );
}

/// A terminal that reports its text area in pixels but not its cell size needs a client size to
/// divide it by. The reply comes off the route thread while the size comes off the
/// server-instruction thread, so it can land first - and it is re-sent only on a font size change,
/// so the size arriving has to be what derives the cell size.
#[test]
fn a_pixel_dimensions_reply_arriving_before_the_client_size_still_derives_the_cell_size() {
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_non_mirrored_screen(size);
    // the helper seeds a size for client 1; the race is about it not being there yet
    screen.client_sizes.clear();

    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 480,
            }),
        },
    );
    assert_eq!(
        *screen.character_cell_size.borrow(),
        None,
        "there is no client size to divide the text area by yet"
    );

    screen.set_client_size(1, size);

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "the size arriving derives what the earlier reply could not"
    );

    screen.set_client_size(
        1,
        Size {
            cols: 100,
            rows: 30,
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "a later resize must not divide the stale text area by the new grid"
    );
}

#[test]
fn recorder_pane_metadata() {
    use zellij_utils::data::{PaneInfo, PaneManifest, TabInfo};
    // Create one tab with one terminal pane and one plugin pane
    let mut pane_manifest = PaneManifest::default();
    let terminal_pane = PaneInfo {
        id: 5,
        is_plugin: false,
        title: "my terminal".to_string(),
        pane_content_rows: 24,
        pane_content_columns: 80,
        ..Default::default()
    };
    let plugin_pane = PaneInfo {
        id: 6,
        is_plugin: true,
        title: "plugin".to_string(),
        pane_content_rows: 24,
        pane_content_columns: 80,
        ..Default::default()
    };
    pane_manifest
        .panes
        .insert(0, vec![terminal_pane.clone(), plugin_pane]);
    let tab_info = TabInfo {
        position: 0,
        name: "my tab".to_string(),
        ..Default::default()
    };
    // Try to call conversion helper - will fail until implemented
    let metadata =
        crate::screen::Screen::convert_pane_metadata(&pane_manifest, &[tab_info.clone()]);
    assert_eq!(metadata.len(), 1, "should filter plugin panes");
    assert_eq!(metadata[0].terminal_id, 5);
    assert_eq!(metadata[0].tab_id, 0);
    assert_eq!(metadata[0].tab_position, 0);
    assert_eq!(metadata[0].tab_name, "my tab");
    assert_eq!(metadata[0].title, "my terminal");
    assert_eq!(metadata[0].cols, 80);
    assert_eq!(metadata[0].rows, 24);
}

// Regression tests for: leaving Scroll mode used to unconditionally snap the active pane's
// scroll position back to the bottom (via an unconditional `clear_active_terminal_scroll` call
// in `Screen::change_mode`), which meant switching focus away from a scrolled pane and back
// (MoveFocus always leaves Scroll mode first) lost the scroll position. The fix drops that
// unconditional reset and instead keeps the client's mode in sync with whether its focused pane
// is scrolled: focusing a scrolled pane enters Scroll mode, focusing an unscrolled one returns to
// the session's actual default mode (Normal, or Locked under unlock-first).

fn fill_pane_zero(mock_screen: &MockScreen) {
    let mut pane_contents = String::new();
    for i in 0..20 {
        pane_contents.push_str(&format!("fill pane up with something {}\n\r", i));
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::PtyBytes(
        0,
        pane_contents.as_bytes().to_vec(),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
}

fn last_change_mode_for_client(
    received_server_instructions: &Arc<Mutex<Vec<ServerInstruction>>>,
    client_id: ClientId,
) -> Option<InputMode> {
    received_server_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            ServerInstruction::ChangeMode(cid, mode, _) if *cid == client_id => Some(*mode),
            _ => None,
        })
}

#[test]
pub fn scroll_position_is_preserved_across_move_focus() {
    let size = Size { cols: 80, rows: 10 };
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let client_id = mock_screen.main_client_id;
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // Fill and scroll up the left pane (terminal 0), then leave Scroll mode: with the fix, the
    // scroll position must survive the mode change (this used to be unconditionally cleared).
    fill_pane_zero(&mock_screen);
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Scroll,
        None,
        client_id,
        None,
    ));
    let scroll_up = CliAction::ScrollUp { pane_id: None };
    for _ in 0..4 {
        send_cli_action_to_server(&session_metadata, scroll_up.clone(), client_id);
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Normal,
        None,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Move focus away to the right pane and back to the left (scrolled) pane. A MoveFocus always
    // leaves Scroll mode first, which is exactly the path that used to reset the scroll position.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Left,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().unwrap().iter(),
        size,
    );
    let (_cursor_position, last_snapshot) = snapshots.last().unwrap();
    assert!(
        last_snapshot.contains("SCROLL:"),
        "the left pane should still show a scroll indicator after moving focus away and back, got:\n{}",
        last_snapshot
    );
    assert!(
        !last_snapshot.contains("something 19"),
        "the left pane should still be scrolled up (the last line should not be visible) after moving focus away and back, got:\n{}",
        last_snapshot
    );
}

fn assert_focus_change_syncs_scroll_mode(default_mode: InputMode) {
    let size = Size { cols: 80, rows: 10 };
    let mut initial_layout = TiledPaneLayout::default();
    initial_layout.children_split_direction = SplitDirection::Vertical;
    initial_layout.children = vec![TiledPaneLayout::default(), TiledPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.default_mode = Some(default_mode);
    let client_id = mock_screen.main_client_id;
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    // Fill and scroll up the left pane, then return to the default mode; its scroll position
    // survives (see scroll_position_is_preserved_across_move_focus above), so the left pane is
    // left scrolled while the client is back in its default mode.
    fill_pane_zero(&mock_screen);
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Scroll,
        None,
        client_id,
        None,
    ));
    let scroll_up = CliAction::ScrollUp { pane_id: None };
    for _ in 0..4 {
        send_cli_action_to_server(&session_metadata, scroll_up.clone(), client_id);
    }
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        default_mode,
        None,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // From the default mode, focus the unscrolled (right) pane: nothing to sync.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Focus back to the scrolled (left) pane: the client should be switched to Scroll.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Left,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        last_change_mode_for_client(&received_server_instructions, client_id),
        Some(InputMode::Scroll),
        "focusing the scrolled pane should switch the client to Scroll mode",
    );

    // The mock has no server loop to round-trip ServerInstruction::ChangeMode back into the
    // screen, so apply the mode the server would have set to advance mode_info to Scroll.
    let _ = mock_screen.to_screen.send(ScreenInstruction::ChangeMode(
        InputMode::Scroll,
        None,
        client_id,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Focus the unscrolled (right) pane: the client should return to its default mode.
    send_cli_action_to_server(
        &session_metadata,
        CliAction::MoveFocus {
            direction: Direction::Right,
        },
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(
        last_change_mode_for_client(&received_server_instructions, client_id),
        Some(default_mode),
        "focusing the unscrolled pane should return the client to its default mode",
    );

    mock_screen.teardown(vec![server_thread, screen_thread]);
}

#[test]
pub fn focusing_a_scrolled_pane_enters_scroll_mode() {
    assert_focus_change_syncs_scroll_mode(InputMode::Normal);
}

#[test]
pub fn focusing_a_scrolled_pane_enters_scroll_mode_with_locked_default() {
    // Unlock-first users have Locked as their default mode; leaving a scrolled pane must return
    // them to Locked (not Normal), and Scroll is entered from Locked.
    assert_focus_change_syncs_scroll_mode(InputMode::Locked);
}

// ---------------------------------------------------------------------------
// Warp eligibility
// ---------------------------------------------------------------------------
//
// `check_pane_can_be_warped` is the only place a warp can be refused cheaply.
// Everything it does not catch is caught after the supervisor has been released,
// when the pane belongs to nobody and the refusal costs a recovery step, so each
// of these is worth pinning down.

#[test]
fn a_plugin_pane_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Plugin(1)), 1),
        Err("this is a plugin pane; it has no process, so there is nothing to move".to_owned())
    );
}

#[test]
fn a_pane_from_another_session_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(99)), 1),
        Err("there is no pane 99 in this session".to_owned())
    );
}

/// A tab where terminal 1 has been replaced by a scrollback editor (terminal 9)
/// and is now suppressed behind it, plus a second tab so the session is never
/// down to its last pane.
fn screen_with_a_scrollback_editor_open() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&0)
        .expect("TEST")
        .suppress_pane_and_replace_with_pid(
            PaneId::Terminal(1),
            PaneId::Terminal(9),
            false,
            None,
            None,
            None,
        )
        .expect("TEST");
    screen
}

#[test]
fn the_visible_half_of_a_scrollback_editor_pair_cannot_be_warped() {
    // `Tab::close_pane` would swap the hidden pane back in rather than remove
    // this one, so the far session would end up holding the wrong pty.
    let mut screen = screen_with_a_scrollback_editor_open();

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(9)), 1),
        Err("this pane has a scrollback editor open; close it before moving the pane".to_owned())
    );
}

#[test]
fn the_hidden_half_of_a_scrollback_editor_pair_cannot_be_warped() {
    // The dangerous one. `suppressed_panes` is keyed by the visible pane, so
    // nothing downstream can find pane 1 to serialize — and `take_released_pane_for_warp`
    // only discovers that after the supervisor has been released, which would
    // leave a running pane with no session to go back to.
    let mut screen = screen_with_a_scrollback_editor_open();

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Err("this pane has a scrollback editor open; close it before moving the pane".to_owned())
    );
}

#[test]
fn the_sessions_only_pane_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Err("this is the session's only pane; moving it would close the session".to_owned())
    );
}

#[test]
fn a_terminal_pane_with_somewhere_to_leave_behind_can_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(1)), 1),
        Ok(1)
    );
}

#[test]
fn a_tabs_last_pane_can_still_be_warped() {
    // Deliberately allowed: the render pass sweeps a tab with nothing selectable
    // left in it, so emptying a tab is a tab closing, not a session ending.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.tabs.get(&1).expect("TEST").get_all_pane_ids().len(),
        1
    );
    assert_eq!(
        screen.check_pane_can_be_warped(Some(PaneId::Terminal(2)), 1),
        Ok(2)
    );
}

// ---------------------------------------------------------------------------
// Tab warp eligibility
// ---------------------------------------------------------------------------
//
// The same argument as above, with more at stake: what a tab warp releases
// before it can discover a problem is not one supervisor but every supervisor in
// the tab.

#[test]
fn a_tab_that_does_not_exist_cannot_be_warped() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);

    assert_eq!(
        screen.check_tab_can_be_warped(Some(99), 1).err(),
        Some("there is no tab 99 in this session".to_owned())
    );
}

#[test]
fn a_tab_with_a_scrollback_editor_open_cannot_be_warped() {
    // Refused for the same reason a single pane with one open is: the
    // suppressed-pane bookkeeping is keyed by the visible pane, so closing the
    // pair hands over the wrong pty.
    let mut screen = screen_with_a_scrollback_editor_open();

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some(
            "this tab has a pane hidden behind another one; close it before moving the tab"
                .to_owned()
        )
    );
}

#[test]
fn the_sessions_only_tab_can_be_warped() {
    // The pane version refuses the session's last pane, because moving it would
    // end the session by accident. Asking for the last *tab* is asking for
    // everything on purpose, so it is allowed and the session then ends.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);

    assert_eq!(screen.tabs.len(), 1);
    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(0), 1)
        .expect("the only tab should still be movable");
    assert_eq!((tab_index, terminal_ids), (0, vec![1]));
}

#[test]
fn warping_a_tab_takes_every_pane_that_has_a_process() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(Some(1), 1)
        .expect("the tab should be movable");
    assert_eq!(tab_index, 1);
    assert_eq!(terminal_ids, vec![2]);
}

#[test]
fn a_tab_warp_defaults_to_the_focused_tab() {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.go_to_tab(1, 1).expect("TEST");

    let (tab_index, terminal_ids, _) = screen
        .check_tab_can_be_warped(None, 1)
        .expect("the focused tab should be movable");
    assert_eq!((tab_index, terminal_ids), (0, vec![1]));
}

#[test]
fn a_tab_that_can_be_warped_has_already_had_its_arrangement_worked_out() {
    // The ordering that matters most here. Rebuilding a tab's layout from its
    // geometry can fail, and it happens during this check — while the tab is
    // still a tab — rather than after its supervisors have been let go, when
    // failing would leave a tab on screen with nothing attached to it.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);

    let (_, terminal_ids, prepared) = screen
        .check_tab_can_be_warped(Some(1), 1)
        .expect("the tab should be movable");
    assert_eq!(
        prepared.tiled_panes.len(),
        terminal_ids.len(),
        "every terminal that would be released should already have a slot",
    );
}

#[test]
fn a_tab_of_nothing_but_plugin_panes_has_nothing_to_warp() {
    // Not a refusal on principle — a plugin pane rides along in the layout and is
    // restarted at the far end. But a tab made of nothing else has no process to
    // move, so moving it would be an elaborate way of opening a new tab.
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.close_pane(PaneId::Terminal(1), false, None);

    assert_eq!(
        screen.check_tab_can_be_warped(Some(0), 1).err(),
        Some("this tab has no panes with a process to move".to_owned())
    );
}

// ---------------------------------------------------------------------------
// Warp scrollback budget
// ---------------------------------------------------------------------------

#[test]
fn a_tabs_panes_share_one_frames_worth_of_scrollback() {
    // A whole tab travels in a single IPC frame. If every pane spent the
    // single-pane budget, a tab of sixteen would sit exactly on
    // MAX_IPC_FRAME_LEN and a tab of seventeen would be unsendable — which would
    // lose the tab at the very last step, after every supervisor had already
    // been released.
    for panes in [1, 2, 8, 16, 64, 256] {
        let total = Screen::warp_budget_per_pane(panes) * panes;
        assert!(
            total < zellij_utils::ipc::MAX_IPC_FRAME_LEN,
            "a tab of {panes} panes would not fit in a frame",
        );
    }
}

#[test]
fn a_small_tab_is_not_charged_for_panes_it_does_not_have() {
    // Sharing the allowance out must not make a one-pane tab carry *more*
    // scrollback than a single warped pane would, or the two paths would
    // truncate differently for no reason the user could see.
    assert_eq!(
        Screen::warp_budget_per_pane(1),
        Screen::WARP_CONTENTS_BUDGET
    );
    assert!(Screen::warp_budget_per_pane(64) < Screen::WARP_CONTENTS_BUDGET);
}

#[test]
fn a_scrollback_over_budget_keeps_its_most_recent_end() {
    // Truncating from the front is what makes this safe: the receiving side
    // feeds the result to a VTE parser, which picks up mid-stream.
    let contents = "abcdefghij".repeat(100);
    let fitted = Screen::fit_to_budget(contents.clone(), 100);
    assert_eq!(fitted.len(), 100);
    assert!(contents.ends_with(&fitted));
}

#[test]
fn a_scrollback_within_budget_is_left_alone() {
    let contents = "hello".to_owned();
    assert_eq!(Screen::fit_to_budget(contents.clone(), 100), contents);
}

// ---------------------------------------------------------------------------
// Moving panes between tabs when tab ids and tab positions have diverged
//
// A tab has a stable `id` (the `Screen::tabs` key) and a display `position` that
// recompacts every time a tab closes. In a session that has never closed a tab the two
// are equal, which is why a function that confuses them can look correct for a long
// time. These tests build the state a real session reaches the moment any tab closes.
// ---------------------------------------------------------------------------

/// Four tabs, the first two closed: ids {2, 3} at positions {0, 1}.
///
/// Each surviving tab holds one pane, with a pid that identifies it. Panes 1 and 2 died
/// with tabs 0 and 1.
fn screen_with_diverged_tab_ids() -> Screen {
    let mut screen = create_new_screen(
        Size {
            cols: 121,
            rows: 20,
        },
        true,
        true,
    );
    for tab_id in 0..4 {
        new_tab(&mut screen, tab_id as u32 + 1, tab_id);
    }
    screen.close_tab_by_id(0).expect("TEST");
    screen.close_tab_by_id(1).expect("TEST");

    assert_eq!(
        screen.tabs.get(&2).map(|t| t.position),
        Some(0),
        "Pre-condition: tab id 2 sits at position 0"
    );
    assert_eq!(
        screen.tabs.get(&3).map(|t| t.position),
        Some(1),
        "Pre-condition: tab id 3 sits at position 1"
    );
    screen
}

/// Adds a pane to a tab by id, so a test does not depend on which tab is focused.
fn add_pane_to_tab(screen: &mut Screen, tab_id: usize, pane_id: PaneId, floating: bool) {
    let placement = if floating {
        NewPanePlacement::Floating(None)
    } else {
        NewPanePlacement::default()
    };
    screen
        .tabs
        .get_mut(&tab_id)
        .expect("TEST")
        .new_pane(pane_id, None, None, false, false, placement, Some(1), None)
        .expect("TEST");
}

#[test]
fn break_panes_to_tab_with_id_moves_a_pane_when_id_and_position_diverge() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 2, pane_to_move, false);

    // tab id 3 is at position 1, so a function that read this argument as a position
    // would find no tab at all and quietly do nothing
    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], 3, false, 1)
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.moved, vec![pane_to_move]);
    assert_eq!(outcome.destination_tab_id, Some(3));
    assert!(
        screen
            .tabs
            .get(&3)
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "The pane is in the tab it was addressed to"
    );
    assert!(
        !screen
            .tabs
            .get(&2)
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "and no longer in the tab it came from"
    );
}

#[test]
fn break_panes_to_tab_with_id_is_a_no_op_for_a_nonexistent_tab_id() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 2, pane_to_move, false);

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], 99, false, 1)
        .expect("TEST");

    assert!(!outcome.succeeded(), "{outcome:?}");
    assert_eq!(
        outcome.destination_tab_id, None,
        "No destination means nothing was even extracted"
    );
    assert_eq!(outcome.affected_tab_id(), None, "so the plugin is told no");
    assert!(
        screen
            .tabs
            .get(&2)
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "The pane is untouched"
    );
    assert_eq!(screen.tabs.len(), 2, "and so are the tabs");
}

#[test]
fn break_panes_to_tab_never_destroys_a_pane_it_could_not_move() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 2, pane_to_move, false);

    // every way of naming a tab that does not exist, including the ids of the two tabs
    // that were closed and the positions the surviving tabs no longer have
    for bad_tab_id in [0, 1, 4, 99] {
        let outcome = screen
            .break_multiple_panes_to_tab_with_id(vec![pane_to_move], bad_tab_id, false, 1)
            .expect("TEST");
        assert!(!outcome.succeeded(), "tab id {bad_tab_id}: {outcome:?}");
        assert!(
            screen
                .tabs
                .get(&2)
                .unwrap()
                .has_pane_with_pid(&pane_to_move),
            "tab id {bad_tab_id}: the pane is still in the tab it came from"
        );
        assert!(
            outcome.closed.is_empty(),
            "tab id {bad_tab_id}: and nothing was torn down"
        );
    }
}

#[test]
fn break_panes_to_tab_leaves_a_pane_that_is_already_in_the_destination() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 3, pane, false);

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane], 3, false, 1)
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert_eq!(outcome.already_in_destination, vec![pane]);
    assert!(outcome.moved.is_empty(), "nothing needed moving");
    assert!(screen.tabs.get(&3).unwrap().has_pane_with_pid(&pane));
}

#[test]
fn break_panes_to_tab_with_id_keeps_a_floating_pane_floating() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 2, pane_to_move, true);
    assert!(
        screen
            .tabs
            .get(&2)
            .unwrap()
            .pane_id_is_floating(&pane_to_move),
        "Pre-condition: the pane starts floating"
    );

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], 3, false, 1)
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert!(
        screen
            .tabs
            .get(&3)
            .unwrap()
            .pane_id_is_floating(&pane_to_move),
        "and arrives floating rather than tiled"
    );
}

#[test]
fn break_panes_to_tab_with_id_focuses_the_destination_tab() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 2, pane_to_move, false);
    screen.switch_active_tab(0, None, true, 1).expect("TEST");

    screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], 3, true, 1)
        .expect("TEST");

    assert_eq!(
        screen.active_tab_ids.get(&1),
        Some(&3),
        "Focus follows the panes, and lands on the tab id rather than the position"
    );
}

#[test]
fn break_panes_to_tab_reports_failure_when_only_some_panes_moved() {
    let mut screen = screen_with_diverged_tab_ids();
    let real_pane = PaneId::Terminal(42);
    let stale_pane = PaneId::Terminal(43);
    add_pane_to_tab(&mut screen, 2, real_pane, false);

    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![real_pane, stale_pane], 3, false, 1)
        .expect("TEST");

    assert_eq!(outcome.moved, vec![real_pane], "the real pane did move");
    assert_eq!(outcome.not_found, vec![stale_pane]);
    assert!(
        !outcome.succeeded(),
        "but a partial move is reported as a failure, so the caller re-reads the session"
    );
    assert_eq!(outcome.affected_tab_id(), None);
}

/// The `BreakPanesToTabWithIndex` handler takes a 0-based display position, which it
/// must convert to a stable id before moving anything. This is the exact call that
/// destroyed a pane in the field: position 1 addressed a tab whose id was 3, and the
/// mover then looked up tab *id* 1, found nothing, and dropped the pane it had already
/// pulled out of its tab.
#[test]
fn break_panes_to_tab_with_index_resolves_a_position_and_not_an_id() {
    let mut screen = screen_with_diverged_tab_ids();
    let pane_to_move = PaneId::Terminal(42);
    add_pane_to_tab(&mut screen, 2, pane_to_move, false);

    let tab_id = screen
        .get_tab_id_at_position(1)
        .expect("TEST: position 1 exists");
    assert_eq!(tab_id, 3, "position 1 is tab id 3, not tab id 1");
    let outcome = screen
        .break_multiple_panes_to_tab_with_id(vec![pane_to_move], tab_id, false, 1)
        .expect("TEST");

    assert!(outcome.succeeded(), "{outcome:?}");
    assert!(
        screen
            .tabs
            .get(&3)
            .unwrap()
            .has_pane_with_pid(&pane_to_move),
        "The pane is in the tab the user pointed at"
    );
    assert_eq!(screen.tabs.len(), 2, "and no tab was swept away");
}

#[test]
fn break_panes_to_tab_with_index_has_no_tab_to_resolve_when_the_position_is_out_of_range() {
    let screen = screen_with_diverged_tab_ids();
    assert_eq!(
        screen.get_tab_id_at_position(7),
        None,
        "so the handler refuses before extracting anything"
    );
}
