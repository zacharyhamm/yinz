use super::{screen_thread_main, CopyOptions, Screen, ScreenInstruction};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::{PaneFactory, PaneId};
use crate::pty_host::client::{ReleasedPane, ReleasedPtyHost, ReleasedRemotePane};
use crate::{
    channels::SenderWithContext, os_input_output::ServerOsApi, route::route_action,
    thread_bus::Bus, ClientId, ServerInstruction, SessionMetaData, ThreadSenders,
};
use zellij_test_support::Watched;

use insta::assert_snapshot;
use std::path::PathBuf;
use zellij_utils::cli::CliAction;
use zellij_utils::data::WarpedPane;
use zellij_utils::data::{Event, EventType, Mouse, OverlayPlacement, Resize, Style, Styling};
use zellij_utils::errors::{prelude::*, ErrorContext};
use zellij_utils::input::actions::Action;
use zellij_utils::input::command::{RunCommand, TerminalAction};
use zellij_utils::input::config::Config;
use zellij_utils::input::context_menu::ContextMenu;
use zellij_utils::input::keybinds::Keybinds;
use zellij_utils::input::layout::{
    FloatingPaneLayout, Layout, PercentOrFixed, PluginAlias, PluginUserConfiguration, Run,
    RunPlugin, RunPluginLocation, RunPluginOrAlias, SplitDirection, TiledPaneLayout,
};

use zellij_utils::input::mouse::MouseEvent;
use zellij_utils::input::options::{
    HostNotificationProtocol, NestedSessionHandling, NotificationPosition, Options, PaneFrameStyle,
    DEFAULT_NOTIFICATION_TIMEOUT_SECS, DEFAULT_WORD_SEPARATORS,
};

use crate::background_jobs::BackgroundJob;
use crate::os_input_output::{AsyncReader, NullAsyncReader};
use crate::pty_writer::PtyWriteInstruction;
use crate::{
    plugins::{PluginInstruction, PluginRenderAsset},
    pty::{ClientTabIndexOrPaneId, PtyInstruction},
};
use std::collections::{BTreeMap, HashSet};
use std::env::set_var;
use std::sync::{Arc, Mutex};
use zellij_utils::ipc::IpcReceiverWithContext;
use zellij_utils::pane_size::{Size, SizeInPixels};
use zellij_utils::position::Position;

use interprocess::local_socket::Stream as LocalSocketStream;
use zellij_utils::ipc::PixelDimensions;
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

/// Blocks until the screen has rendered a frame satisfying `is_settled`, and returns it.
///
/// This is the replacement for `sleep(...)` followed by `snapshots.last()`. Renders are emitted
/// asynchronously and coalesce, so *which* frame is last — and how many arrive at all — depends on
/// timing. Asserting on the last one therefore asserts on the scheduler. Waiting for a frame whose
/// **content** shows the state under test is stable regardless of how the renders were batched.
///
/// The grid is cumulative, so every frame is replayed from the beginning each time. That is
/// quadratic in the number of renders and entirely fine at test sizes.
fn wait_for_settled_render(
    received_server_instructions: &Watched<Vec<ServerInstruction>>,
    screen_size: Size,
    what: &str,
    is_settled: impl Fn(&str) -> bool,
) -> String {
    received_server_instructions.wait_until_described(
        what,
        zellij_test_support::default_timeout(),
        |events| {
            take_snapshots_and_cursor_coordinates_from_render_events(events.iter(), screen_size)
                .into_iter()
                .find(|(_, snapshot)| is_settled(snapshot))
                .map(|(_, snapshot)| snapshot)
        },
        |events| {
            let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
                events.iter(),
                screen_size,
            );
            match snapshots.last() {
                Some((_, last)) => format!(
                    "{} instruction(s), {} render(s); last render was:\n{last}",
                    events.len(),
                    snapshots.len()
                ),
                None => format!("{} instruction(s) and no renders at all", events.len()),
            }
        },
    )
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

/// Everything the screen wrote to each terminal's stdin, keyed by terminal id.
type TtyStdinBytes = Arc<Mutex<BTreeMap<u32, Vec<u8>>>>;
type ServerReceiver = Receiver<(ServerInstruction, ErrorContext)>;

#[derive(Clone, Default)]
struct FakeInputOutput {
    fake_filesystem: Arc<Mutex<HashMap<String, String>>>,
    server_to_client_messages: Arc<Mutex<HashMap<ClientId, Vec<ServerToClientMsg>>>>,
    tty_stdin_bytes: TtyStdinBytes,
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
    fn write_to_tty_stdin(&self, id: u32, buf: &[u8]) -> Result<usize> {
        self.tty_stdin_bytes
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
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
    fn send_sigcont(&self, _pid: u32) -> Result<()> {
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
    let (screen, _tty_stdin_bytes, _server_receiver) = create_new_screen_with_capture(
        size,
        advanced_mouse_actions,
        mouse_hover_effects,
        support_kitty_graphics_protocol,
        true,
    );
    screen
}

/// Like `create_new_screen_with_kitty_graphics`, but also hands back what the screen writes to
/// each pane's stdin and what it sends to the server thread.
fn create_new_screen_with_capture(
    size: Size,
    advanced_mouse_actions: bool,
    mouse_hover_effects: bool,
    support_kitty_graphics_protocol: bool,
    session_is_mirrored: bool,
) -> (Screen, TtyStdinBytes, ServerReceiver) {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    let tty_stdin_bytes = fake_os_input.tty_stdin_bytes.clone();
    bus.os_input = Some(Box::new(fake_os_input));
    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::unbounded();
    bus.senders.to_server = Some(SenderWithContext::new(to_server));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let max_panes = None;
    let mode_info = ModeInfo {
        session_name: Some("zellij-test".into()),
        ..Default::default()
    };
    let draw_pane_frames = PaneFrameStyle::None;
    let auto_layout = true;
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
        None, // default_shell_action
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
        advanced_mouse_actions,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        mouse_scroll_resize,
        mouse_hover_effects,
        visual_bell,
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        ContextMenu::default(),
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    (
        seed_first_client_size(screen, size),
        tty_stdin_bytes,
        server_receiver,
    )
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
    pub received_background_jobs: Watched<Vec<BackgroundJob>>,
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
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            pane_layout,
            initial_floating_panes_layout,
            pane_ids,
            floating_pane_ids,
            vec![],
            plugin_ids,
            tab_index,
            true,
            self.main_client_id,
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
                    Box::default(),
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
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            pane_layout,
            initial_floating_panes_layout,
            pane_ids,
            floating_pane_ids,
            vec![],
            plugin_ids,
            tab_index,
            true,
            self.main_client_id,
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
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            tab_layout,
            vec![], // floating_panes_layout
            pane_ids,
            vec![], // floating panes ids
            vec![], // adopted agents
            plugin_ids,
            0,
            true,
            self.main_client_id,
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
    }
    pub fn new_tab_with_plugins(&mut self, plugin_pane_ids: Vec<u32>) {
        // Build a layout where each child is a plugin pane
        let fake_plugin_url = "file:/path/to/fake/plugin";
        let run_plugin = RunPluginOrAlias::from_url(fake_plugin_url, &None, None, None).unwrap();
        let tab_layout = TiledPaneLayout {
            children_split_direction: SplitDirection::Vertical,
            children: plugin_pane_ids
                .iter()
                .map(|_| TiledPaneLayout {
                    run: Some(Run::Plugin(run_plugin.clone())),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
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
            self.main_client_id,
            None,
        ));
        let _ = self.to_screen.send(ScreenInstruction::ApplyLayout(
            tab_layout,
            vec![], // floating_panes_layout
            pane_ids,
            vec![], // floating panes ids
            vec![], // adopted agents
            plugin_ids,
            tab_index,
            true,
            self.main_client_id,
            None,
            None,
        ));
        self.last_opened_tab_index = Some(tab_index);
    }
    /// Shuts the fake session down and re-raises any panic from the threads it joins.
    ///
    /// Discarding the join results - as this used to - means a panicked screen thread shows up as
    /// a test failing on a wrong or missing snapshot, which sends the reader looking at the
    /// assertion instead of at the panic that caused it. Re-raising puts the original panic, with
    /// its own message and location, back in front of them.
    ///
    /// The sends are still allowed to fail: a thread that has already exited has already done what
    /// the Exit message would have asked of it, and its `join` below is the real check.
    pub fn teardown(&mut self, threads: Vec<std::thread::JoinHandle<()>>) {
        let _ = self.to_pty.send(PtyInstruction::Exit);
        let _ = self.to_pty_writer.send(PtyWriteInstruction::Exit);
        let _ = self.to_screen.send(ScreenInstruction::Exit);
        let _ = self.to_server.send(ServerInstruction::KillSession);
        let _ = self.to_plugin.send(PluginInstruction::Exit);
        let _ = self.to_background_jobs.send(BackgroundJob::Exit);
        for thread in threads {
            if let Err(panic) = thread.join() {
                std::panic::resume_unwind(panic);
            }
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
            config_file_path: self.session_metadata.config_file_path.clone(),
            config_dir: self.session_metadata.config_dir.clone(),
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
            config_file_path: None,
            config_dir: None,
            key_passthrough_clients: Default::default(),
        };

        let os_input = FakeInputOutput::default();
        let config_options = Options::default();
        let mut config = Config::default();
        config.options.pane_frame_style = Some(PaneFrameStyle::Full);
        config.options.stacked_pane_list = Some(false);
        let main_client_id = 1;

        let _ = to_screen.send(ScreenInstruction::RecomputeTabSize(main_client_id, size));

        let received_background_jobs = Watched::new(vec![]);
        std::thread::Builder::new()
            .name("background_jobs_thread".to_string())
            .spawn({
                let to_screen = to_screen.clone();
                let received_background_jobs = received_background_jobs.clone();
                move || loop {
                    let (event, _err_ctx) = background_jobs_receiver
                        .recv()
                        .expect("failed to receive event on channel");
                    received_background_jobs.lock().push(event.clone());
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
                            log.lock().push(event.clone());
                            break;
                        },
                        _ => {
                            log.lock().push(event.clone());
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
            client_id,
            None,
        )
        .expect("TEST");
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
    let mode_info = ModeInfo {
        session_name: Some("zellij-test".into()),
        ..Default::default()
    };
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
        None, // default_shell_action
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
        true,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,
        true,
        visual_bell,
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        ContextMenu::default(),
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    (seed_first_client_size(screen, size), messages)
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
            if let ServerInstruction::ForwardQueryToHost(token, bytes, _) = instr {
                out.push((token, bytes));
            }
        }
        out
    }

    fn drain_forward_queries_with_async(&self) -> Vec<(u32, Vec<u8>, bool)> {
        let mut out = Vec::new();
        while let Ok((instr, _ctx)) = self.server_rx.try_recv() {
            if let ServerInstruction::ForwardQueryToHost(token, bytes, resolve_async) = instr {
                out.push((token, bytes, resolve_async));
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
    let mode_info = ModeInfo {
        session_name: Some("zellij-test".into()),
        ..Default::default()
    };
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
        None, // default_shell_action
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
        true,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,
        true,
        visual_bell,
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        ContextMenu::default(),
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

fn clipboard_query() -> HostQuery {
    HostQuery::ClipboardContent {
        selection: 'c',
        terminator: OscTerminator::St,
    }
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
    let mode_info = ModeInfo {
        session_name: Some("zellij-test".into()),
        ..Default::default()
    };
    let copy_options = CopyOptions::default();
    let default_layout = Box::new(Layout::default());
    let default_shell = PathBuf::from("my_default_shell");
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
        None, // default_shell_action
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
        true,
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,
        true,
        true,
        false,
        false,
        false, // context_menu_on_right_click
        ContextMenu::default(),
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

/// A screen whose background-job sends can be read back, for the tests that care about what was
/// asked of the shared animation clock rather than about what was painted.
fn screen_with_background_jobs(size: Size) -> (Screen, Receiver<(BackgroundJob, ErrorContext)>) {
    let mut screen = create_non_mirrored_screen(size);
    let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));
    (screen, background_jobs_receiver)
}

/// Every background job sent so far, draining the channel.
fn drain_background_jobs(receiver: &Receiver<(BackgroundJob, ErrorContext)>) -> Vec<BackgroundJob> {
    receiver.try_iter().map(|(job, _)| job).collect()
}

fn create_non_mirrored_screen(size: Size) -> Screen {
    let mut bus: Bus<ScreenInstruction> = Bus::empty();
    let fake_os_input = FakeInputOutput::default();
    bus.os_input = Some(Box::new(fake_os_input));
    let client_attributes = ClientAttributes {
        size,
        ..Default::default()
    };
    let mode_info = ModeInfo {
        session_name: Some("zellij-test".into()),
        ..Default::default()
    };
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
        Box::default(),
        None, // default_layout_name
        PathBuf::from("my_default_shell"),
        None,  // default_shell_action
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
        true, // advanced_mouse_actions
        true,
        DEFAULT_WORD_SEPARATORS.to_owned(),
        true,  // mouse_scroll_resize
        true,  // mouse_hover_effects
        true,  // visual_bell
        false, // focus_follows_mouse
        false, // mouse_click_through
        false, // context_menu_on_right_click
        ContextMenu::default(),
        NestedSessionHandling::default(),
        NotificationPosition::default(),
        DEFAULT_NOTIFICATION_TIMEOUT_SECS,
    );
    seed_first_client_size(screen, size)
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

/// Open a floating pane in `tab_index` and give it something to say. `overlay` places it
/// against the whole surface, as an overlay plugin places itself.
fn open_floating_pane(
    screen: &mut Screen,
    tab_index: usize,
    pane_id: u32,
    contents: &str,
    overlay: bool,
) {
    let placement = if overlay {
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(2)),
                y: Some(PercentOrFixed::Fixed(2)),
                width: Some(PercentOrFixed::Percent(60)),
                height: Some(PercentOrFixed::Fixed(4)),
                ..Default::default()
            },
        )))
    } else {
        NewPanePlacement::Floating(None)
    };
    let tab = screen.tabs.get_mut(&tab_index).expect("TEST");
    tab.new_pane(
        PaneId::Terminal(pane_id),
        None,
        None,
        false,
        true,
        placement,
        Some(1),
        None,
    )
    .expect("TEST");
    tab.handle_pty_bytes(pane_id, contents.as_bytes().to_vec())
        .expect("TEST");
}

/// Like `render_frame_into`, but also reports where the frame left the terminal's cursor.
///
/// Read off the replayed grid rather than out of the render instructions, because that is the only
/// faithful way to ask: `serialize_chunks` writes an absolute goto per character chunk, so where
/// the cursor ends up is a property of the whole frame and not just of the post-VTE instructions
/// `Tab::render_cursor` adds.
/// Where the active pane thinks its cursor is, in absolute surface coordinates. What the frame
/// then does with it - place it, or leave it hidden because something is drawn over that cell - is
/// what the tests using this are about.
fn pane_cursor(screen: &mut Screen, client_id: ClientId) -> (usize, usize) {
    screen
        .get_active_tab_mut(client_id)
        .expect("TEST")
        .get_active_terminal_cursor_position(client_id)
        .map(|(x, y, _)| (x, y))
        .expect("the active pane has a cursor")
}

/// Give a directly-constructed `Screen` somewhere to render to, and hand back the other end.
fn screen_wired_to_server(
    screen: &mut Screen,
) -> channels::Receiver<(ServerInstruction, ErrorContext)> {
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    server_receiver
}

fn render_frame_and_cursor_into(
    screen: &mut Screen,
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    client_id: ClientId,
    grid: &mut Grid,
) -> (Option<(usize, usize)>, String) {
    let snapshot = render_frame_into(screen, receiver, client_id, grid);
    let cursor = grid
        .cursor_coordinates()
        .and_then(|(x, y, visible)| if visible { Some((x, y)) } else { None });
    (cursor, snapshot)
}

fn render_frame_into(
    screen: &mut Screen,
    receiver: &channels::Receiver<(ServerInstruction, ErrorContext)>,
    client_id: ClientId,
    grid: &mut Grid,
) -> String {
    screen.render_to_clients().expect("TEST");
    let mut snapshot = String::new();
    while let Ok((instruction, _)) = receiver.try_recv() {
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            if let Some(client_output) = rendered_clients.remove(&client_id) {
                snapshot = take_snapshot_and_cursor_coordinates(&client_output, grid).1;
            }
        }
    }
    snapshot
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
    received_server_instructions: &Watched<Vec<ServerInstruction>>,
    client_id: ClientId,
) -> Option<InputMode> {
    received_server_instructions
        .lock()
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            ServerInstruction::ChangeMode(cid, mode, _) if *cid == client_id => Some(*mode),
            _ => None,
        })
}

fn assert_focus_change_syncs_scroll_mode(default_mode: InputMode) {
    let size = Size { cols: 80, rows: 10 };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.default_mode = Some(default_mode);
    let client_id = mock_screen.main_client_id;
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
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

// ---------------------------------------------------------------------------
// Where an adopted pane lands
// ---------------------------------------------------------------------------
//
// A pane arriving from another session is placed by `NewPane`, which reads
// `ClientTabIndexOrPaneId::ClientId` as "wherever the client is".

/// Adopt a pane with the given placement and return the `NewPane` it asks for.
fn adopted_pane_placement_target(
    screen: &mut Screen,
    placement: zellij_utils::data::WarpPlacement,
    client_id: ClientId,
) -> ClientTabIndexOrPaneId {
    use zellij_utils::data::WarpedPane;

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    screen.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
    screen
        .place_adopted_pane(
            50,
            WarpedPane::default(),
            placement,
            client_id,
            RunCommand::default(),
            vec![],
            None,
        )
        .expect("TEST");
    screen_receiver
        .try_iter()
        .find_map(|(instruction, _)| match instruction {
            ScreenInstruction::NewPane(_, _, _, _, _, _, target, _, _) => Some(target),
            _ => None,
        })
        .expect("the adopted pane should have asked for a pane")
}

/// A tab where terminal 1's scrollback is being edited by terminal 9, so 1 is
/// suppressed behind the editor, plus a second tab so the session is never down
/// to its last pane.
fn screen_with_a_scrollback_editor_open() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen
        .tabs
        .get_mut(&0)
        .expect("TEST")
        .replace_pane_with_editor_pane(PaneId::Terminal(9), PaneId::Terminal(1))
        .expect("TEST");
    screen
}

/// The same shape, but the stand-in is an ordinary pane opened over terminal 1
/// rather than an editor of it - a pane with its own process and its own reason
/// to exist.
fn screen_with_a_pane_replaced_in_place() -> Screen {
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

/// Drive the second half of a warp: the supervisor has been released, the pane
/// has to be serialized and taken out of its tab.
fn released_pane_for_warp(screen: &mut Screen, terminal_id: u32) -> Box<WarpedPane> {
    screen
        .take_released_pane_for_warp(
            terminal_id,
            Ok(ReleasedPane::Local(ReleasedPtyHost {
                host_id: format!("host-{terminal_id}"),
                host_pid: 1,
                control_socket: Some(PathBuf::from("/tmp/warp-test.sock")),
            })),
        )
        .expect("the pane should have been taken out of its tab")
}

/// The same, for a pane whose process is on another machine.
///
/// What a remote release hands over is a broker and a token rather than a
/// socket, and `received` is the byte count the replay has to resume from —
/// nonzero here so a path that dropped it could not pass by accident.
fn released_remote_pane_for_warp(screen: &mut Screen, terminal_id: u32) -> Box<WarpedPane> {
    screen
        .take_released_pane_for_warp(
            terminal_id,
            Ok(ReleasedPane::Remote(ReleasedRemotePane {
                host: "build-box".to_owned(),
                pane_token: format!("token-{terminal_id}"),
                received: 4096,
                cmd: RunCommand {
                    command: PathBuf::from("/usr/bin/tail"),
                    args: vec!["-f".to_owned(), "/var/log/build".to_owned()],
                    target: zellij_utils::data::PaneTarget::Ssh {
                        host: "build-box".to_owned(),
                    },
                    ..Default::default()
                },
            })),
        )
        .expect("the pane should have been taken out of its tab")
}

// ---------------------------------------------------------------------------
// Warping a tab that holds a stack list
// ---------------------------------------------------------------------------
//
// A stack list keeps all but its visible member in `suppressed_panes`, the same
// place a scrollback editor parks the pane it is editing. Refusing every tab
// with a hidden terminal in it therefore refused every tab with a stack in it,
// which is most of them once the mode is on. The members are ordinary panes with
// ordinary processes and they already have a place in the layout the tab travels
// as - so what has to be told apart is not "hidden" but *how* it is hidden.

/// A session whose first tab holds a three-member stack list, plus a second tab
/// so the session is never down to its last pane.
///
/// Terminal 1 is the tab's original pane; 2 and 3 are stacked onto it, leaving 3
/// visible and 1 and 2 parked in `suppressed_panes` under their own ids.
fn screen_with_a_stack_list() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    // terminal 9, so that a member left behind in this tab cannot be mistaken
    // for a pane that was always here
    new_tab(&mut screen, 9, 1);
    *screen.stacked_pane_list.borrow_mut() = true;
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    for pane_id in [PaneId::Terminal(2), PaneId::Terminal(3)] {
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            Some(1),
            None,
        )
        .expect("TEST");
    }
    assert!(
        tab.has_stack_lists(),
        "the fixture should have formed a stack list",
    );
    screen
}

/// The same three-member stack list, but as the whole session: nothing else is
/// running, so the members are the only panes there are.
fn screen_with_only_a_stack_list() -> Screen {
    let mut screen = create_new_screen(Size { cols: 80, rows: 24 }, false, false);
    new_tab(&mut screen, 1, 0);
    *screen.stacked_pane_list.borrow_mut() = true;
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    for pane_id in [PaneId::Terminal(2), PaneId::Terminal(3)] {
        tab.new_pane(
            pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            Some(1),
            None,
        )
        .expect("TEST");
    }
    screen
}

/// A stack list whose member 3 has a scrollback editor (terminal 8) open on it,
/// with the list then focused elsewhere - so the editor is the member, and it is
/// hidden, and it is standing in for pane 3 all at once.
fn screen_with_an_editor_on_a_hidden_stack_list_member() -> Screen {
    let mut screen = screen_with_a_stack_list();
    let tab = screen.tabs.get_mut(&0).expect("TEST");
    tab.replace_pane_with_editor_pane(PaneId::Terminal(8), PaneId::Terminal(3))
        .expect("TEST");
    tab.focus_hidden_stack_list_member(PaneId::Terminal(1), 1);
    screen
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
fn notification_history_keeps_dismissed_toasts() {
    let mut mock_screen = MockScreen::new(Size { cols: 80, rows: 24 });
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    mock_screen
        .to_screen
        .send(ScreenInstruction::Notify("first".to_owned(), None, None))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::Notify("second".to_owned(), None, None))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::DismissNotification(0))
        .unwrap();

    let (response_tx, response_rx) = crossbeam::channel::bounded(1);
    mock_screen
        .to_screen
        .send(ScreenInstruction::ListNotifications {
            response_channel: response_tx,
        })
        .unwrap();
    let history = response_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("screen returns notification history");
    assert_eq!(
        history
            .iter()
            .map(|notification| (notification.id, notification.message.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "first"), (1, "second")]
    );
    assert!(history
        .iter()
        .all(|notification| notification.received_at_unix_ms > 0));

    mock_screen.teardown(vec![screen_thread]);
}

mod animation_clock;
mod break_and_move_panes;
mod break_panes_to_tab;
mod capabilities;
mod cli_actions;
mod cli_layouts_and_rename;
mod cli_pane_and_tab_id;
mod clients;
mod command_respawn;
mod context_menu;
mod desktop_notifications;
mod floating_panes;
mod hooks;
mod host_focus;
mod host_query;
mod key_intercepts;
mod mouse;
mod nested_sessions;
mod notification_animation;
mod overlays;
mod pane_state;
mod plugin_broadcasts;
mod quit;
mod reveal_pane;
mod scroll_and_warp;
mod sizing;
mod subscribers;
mod tab_projects;
mod tabs;
mod theme;
