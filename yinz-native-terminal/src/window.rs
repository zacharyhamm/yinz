use crate::emulation::color::Paints;
use crate::emulation::selection::Selection;
use crate::emulation::terminal::TerminalState;
use crate::input::clipboard::{Clipboard, ClipboardHandle};
use crate::input::composition::{Composition, Reaction};
use crate::input::{keyboard, links};
use crate::platform::Platform;
use crate::render::atlas::GlyphCache;
use crate::render::pacing::{Decision, Pacer};
use crate::render::renderer::{Frame, Renderer};
use crate::render::retained::RetainedScene;
use crate::render::scene::{BlinkPhase, CursorOptions, SceneInputs};
use crate::session::{self, Recorder, Sender};
use crate::text::font::{points_to_pixels, FontOptions};
use glutin::config::ConfigTemplateBuilder;
use glutin::context::PossiblyCurrentContext;
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};
use yinz_cli::args::{CliArgs, TerminalArgs};
use yinz_ipc::ipc::{ClientToServerMsg, PixelDimensions, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_types::input::actions::Action;
use yinz_types::input::native_terminal::{StartupMode, TerminalConfig};
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::pane_size::{Size, SizeInPixels};
use yinz_types::position::Position;

#[derive(Default)]
struct ScrollAccumulator {
    x: f64,
    y: f64,
}

impl ScrollAccumulator {
    fn steps(&mut self, delta: MouseScrollDelta, width: f64, height: f64) -> (i32, i32) {
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x as f64, y as f64),
            MouseScrollDelta::PixelDelta(position) => (position.x / width, position.y / height),
        };
        fn accumulate(remainder: &mut f64, delta: f64) -> i32 {
            if !delta.is_finite() {
                return 0;
            }
            let total = *remainder + delta;
            *remainder = total.fract();
            // Discard excess whole steps so a large event cannot create a deferred tail.
            total.trunc().clamp(-20.0, 20.0) as i32
        }
        let y = accumulate(&mut self.y, y);
        // Keep the per-event budget at 20, with the server's vertical priority.
        let budget = 20 - y.abs();
        let x = accumulate(&mut self.x, x).clamp(-budget, budget);
        (x, y)
    }
}

fn open_link(uri: &str) {
    links::open(uri)
        .map_err(|error| anyhow!("{error}"))
        .context("open native terminal hyperlink")
        .non_fatal();
}

struct Surfaces {
    // Drop GPU resources while the context and window are still alive.
    renderer: Renderer,
    context: PossiblyCurrentContext,
    surface: Surface<WindowSurface>,
    window: Window,
}
struct App {
    surfaces: Option<Surfaces>,
    proxy: winit::event_loop::EventLoopProxy<()>,
    args: TerminalArgs,
    opts: CliArgs,
    settings: TerminalConfig,
    paints: Paints,
    cache: GlyphCache,
    retained: RetainedScene,
    state: TerminalState,
    sender: Option<Sender>,
    messages: Option<Receiver<ServerToClientMsg>>,
    recorder: Recorder,
    modifiers: ModifiersState,
    alt_keys: keyboard::AltKeys,
    composition: Composition,
    clipboard: ClipboardHandle,
    pointer: (f64, f64),
    scroll: ScrollAccumulator,
    buttons: (bool, bool, bool),
    selection: Option<Selection>,
    mouse_mode: bool,
    focused: bool,
    pacer: Pacer,
    blink: BlinkPhase,
    next_blink: Instant,
    scale: f64,
    zoom: f32,
    received_frame: bool,
    started: Instant,
    error: Option<anyhow::Error>,
}

pub fn run(args: TerminalArgs, opts: CliArgs) -> Result<()> {
    let event_loop = EventLoop::<()>::with_user_event()
        .build()
        .context("initialize native event loop")?;
    let (config, options) = yinz_cli::setup::Setup::config_without_layout(&opts)
        .map_err(|error| anyhow!("{error}"))
        .context("load native settings")?;
    let recorder = Recorder::new(args.record.as_deref())?;
    let fonts = font_options(&config.terminal, 1.0)
        .stack(1.0)
        .context("load native terminal fonts")?;
    let now = Instant::now();
    let paints = paints_for(&config, &options);
    let mut app = App {
        surfaces: None,
        proxy: event_loop.create_proxy(),
        state: TerminalState::new(args.rows as usize, args.cols as usize),
        args,
        opts,
        settings: config.terminal,
        paints,
        cache: GlyphCache::new(fonts),
        retained: RetainedScene::new(),
        sender: None,
        messages: None,
        recorder,
        clipboard: std::sync::Arc::new(std::sync::Mutex::new(Clipboard::open())),
        modifiers: ModifiersState::empty(),
        alt_keys: keyboard::AltKeys::default(),
        composition: Composition::new(),
        pointer: (0.0, 0.0),
        scroll: ScrollAccumulator::default(),
        buttons: (false, false, false),
        selection: None,
        mouse_mode: true,
        focused: true,
        pacer: Pacer::new(None),
        blink: BlinkPhase::On,
        next_blink: now + Duration::from_millis(500),
        scale: 1.0,
        zoom: 1.0,
        received_frame: false,
        started: now,
        error: None,
    };
    // A bounded queue holds at most two messages between IPC and the window thread.
    // The event proxy only wakes the loop; frames remain in this bounded queue.
    let proxy = event_loop.create_proxy();
    app.open_connection(proxy)?;
    event_loop
        .run_app(&mut app)
        .context("run native event loop")?;
    app.messages.take();
    if let Some(sender) = &app.sender {
        session::send(sender, ClientToServerMsg::ClientExited).non_fatal();
        if let Ok(sender) = sender.lock() {
            sender
                .shutdown()
                .context("close native session connection")
                .non_fatal();
        }
    }
    match app.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn font_options(settings: &TerminalConfig, zoom: f32) -> FontOptions {
    FontOptions {
        family: settings.font_family.clone(),
        size: points_to_pixels(settings.font_size.unwrap_or(12.0)) * zoom,
        weight: settings.font_weight.unwrap_or(400),
        ligatures: settings.font_ligatures.unwrap_or(true),
        system_fonts: settings.system_fonts.unwrap_or(true),
        cell: crate::text::font::CellAdjust {
            line_height: settings.line_height.unwrap_or(1.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn native_option_as_alt(
    setting: yinz_types::input::native_terminal::OptionAsAlt,
) -> winit::platform::macos::OptionAsAlt {
    use winit::platform::macos::OptionAsAlt as Native;
    use yinz_types::input::native_terminal::OptionAsAlt;
    match setting {
        OptionAsAlt::None => Native::None,
        OptionAsAlt::Left => Native::OnlyLeft,
        OptionAsAlt::Right => Native::OnlyRight,
        OptionAsAlt::Both => Native::Both,
    }
}

// Consume one message per wake, keeping recoverable diagnostics out of the
// window's fatal-error path while preserving connection and explicit exit errors.
fn receive_message(
    messages: Option<&Receiver<ServerToClientMsg>>,
) -> Result<Option<ServerToClientMsg>> {
    match messages.map(|messages| messages.try_recv()) {
        Some(Ok(ServerToClientMsg::LogError { lines })) => {
            for line in lines {
                log::error!("{line}");
            }
            Ok(None)
        },
        Some(Ok(ServerToClientMsg::Exit {
            exit_reason: yinz_ipc::ipc::ExitReason::Error(message),
        })) => anyhow::bail!("{message}"),
        Some(Ok(message)) => Ok(Some(message)),
        Some(Err(mpsc::TryRecvError::Disconnected)) => {
            anyhow::bail!("the native session connection closed unexpectedly")
        },
        _ => Ok(None),
    }
}

impl App {
    fn open_connection(&mut self, proxy: winit::event_loop::EventLoopProxy<()>) -> Result<()> {
        let connection = session::connect(
            &self.args,
            &self.opts,
            Size {
                rows: self.args.rows as usize,
                cols: self.args.cols as usize,
            },
        )?;
        let (tx, rx) = mpsc::sync_channel(2);
        let mut reader = connection.receiver;
        std::thread::Builder::new()
            .name("native-session-reader".into())
            .spawn(move || {
                while let Some((message, _)) = reader.recv_server_msg().msg() {
                    if tx.send(message).is_err() || proxy.send_event(()).is_err() {
                        return;
                    }
                }
                drop(tx);
                let _ = proxy.send_event(());
            })
            .context("spawn native session reader")?;
        self.sender = Some(connection.sender);
        self.args.session_name = Some(connection.name);
        self.messages = Some(rx);
        Ok(())
    }
    fn send(&self, message: ClientToServerMsg) -> Result<()> {
        if let Some(sender) = &self.sender {
            session::send(sender, message)?;
        }
        Ok(())
    }
    fn action(&self, action: Action) -> Result<()> {
        if let Some(sender) = &self.sender {
            session::action(sender, action)?;
        }
        Ok(())
    }
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let metrics = self.cache.metrics();
        let mut attrs = WindowAttributes::default()
            .with_title("Yinz")
            .with_transparent(true)
            .with_blur(self.settings.blur.unwrap_or(false))
            .with_inner_size(winit::dpi::LogicalSize::new(
                metrics.width * self.args.cols as u32,
                metrics.height * self.args.rows as u32,
            ));
        let startup = self
            .args
            .startup_mode
            .or(self.settings.startup_mode)
            .unwrap_or_default();
        if startup == StartupMode::Maximized {
            attrs = attrs.with_maximized(true);
        }
        if startup == StartupMode::Fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        if startup == StartupMode::Remember
            && let Ok(bytes) = std::fs::read(window_state_path())
            && let Ok((width, height)) = serde_json::from_slice::<(u32, u32)>(&bytes)
            && (100..=16384).contains(&width)
            && (100..=16384).contains(&height)
        {
            attrs = attrs.with_inner_size(winit::dpi::PhysicalSize::new(width, height));
        }
        #[cfg(target_os = "linux")]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attrs = attrs.with_name("yinz", "Yinz");
        }
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_option_as_alt(native_option_as_alt(
                self.settings.macos_option_as_alt.unwrap_or_default(),
            ));
        }
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs))
            .build(
                event_loop,
                ConfigTemplateBuilder::new().with_alpha_size(8),
                |configs| {
                    configs
                        .max_by_key(|config| {
                            (
                                config.supports_transparency().unwrap_or(false),
                                config.num_samples(),
                            )
                        })
                        .expect("display returned no OpenGL configurations")
                },
            )
            .map_err(|error| anyhow!("create native terminal window: {error}"))?;
        let window = window.context("windowing system did not create a native window")?;
        window.set_ime_allowed(true);
        let raw_handle = window
            .window_handle()
            .context("get native window handle")?
            .as_raw();
        let display = config.display();
        let context = crate::render::graphics::create_context(
            &display,
            &config,
            raw_handle,
            Platform::current(),
        )
        .context("create GPU context")?;
        let attrs = window
            .build_surface_attributes(SurfaceAttributesBuilder::new())
            .context("describe GPU surface")?;
        let surface = unsafe { display.create_window_surface(&config, &attrs) }
            .context("create GPU surface")?;
        let context = context
            .make_current(&surface)
            .context("activate GPU context")?;
        surface
            .set_swap_interval(&context, SwapInterval::DontWait)
            .context("set GPU swap interval")
            .non_fatal();
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|symbol| display.get_proc_address(symbol))
        };
        let renderer = Renderer::new(gl).context("initialize GPU renderer")?;
        self.scale = window.scale_factor();
        self.pacer.follow_refresh(
            window
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz()),
        );
        self.surfaces = Some(Surfaces {
            renderer,
            context,
            surface,
            window,
        });
        self.rebuild_fonts()?;
        for message in crate::emulation::palette::seed_messages(&self.paints) {
            self.send(message)?;
        }
        Ok(())
    }
    fn rebuild_fonts(&mut self) -> Result<()> {
        self.cache = GlyphCache::new(
            font_options(&self.settings, self.zoom)
                .stack(self.scale)
                .context("load scaled native fonts")?,
        );
        self.retained = RetainedScene::new();
        self.resize()
    }
    fn resize(&mut self) -> Result<()> {
        let Some(surfaces) = &self.surfaces else {
            return Ok(());
        };
        let size = surfaces.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        surfaces
            .window
            .resize_surface(&surfaces.surface, &surfaces.context);
        let metrics = self.cache.metrics();
        let padding = self.padding();
        let grid = Size {
            cols: (size.width.saturating_sub(2 * padding) / metrics.width.max(1)).clamp(1, 4096)
                as usize,
            rows: (size.height.saturating_sub(2 * padding) / metrics.height.max(1)).clamp(1, 4096)
                as usize,
        };
        self.state.set_cell_size(metrics.width, metrics.height);
        self.send(ClientToServerMsg::TerminalPixelDimensions {
            pixel_dimensions: PixelDimensions {
                character_cell_size: Some(SizeInPixels {
                    width: metrics.width as usize,
                    height: metrics.height as usize,
                }),
                text_area_size: Some(SizeInPixels {
                    width: grid.cols * metrics.width as usize,
                    height: grid.rows * metrics.height as usize,
                }),
                text_area_grid_size: Some(grid),
            },
        })?;
        self.send(ClientToServerMsg::TerminalResize { new_size: grid })?;
        self.pacer.schedule();
        Ok(())
    }
    fn padding(&self) -> u32 {
        (self.settings.padding.unwrap_or(0.0) as f64 * self.scale).round() as u32
    }
    fn cell(&self) -> (usize, usize) {
        let metrics = self.cache.metrics();
        let row = ((self.pointer.1 - self.padding() as f64).max(0.0) / metrics.height.max(1) as f64)
            as usize;
        let col = ((self.pointer.0 - self.padding() as f64).max(0.0) / metrics.width.max(1) as f64)
            as usize;
        (
            row.min(self.state.size().rows.saturating_sub(1)),
            col.min(self.state.size().cols.saturating_sub(1)),
        )
    }
    fn mouse(&self, kind: MouseEventType, wheel: (bool, bool, bool, bool)) -> Result<()> {
        let (row, col) = self.cell();
        self.action(Action::MouseEvent {
            event: MouseEvent {
                event_type: kind,
                left: self.buttons.0,
                right: self.buttons.1,
                middle: self.buttons.2,
                wheel_up: wheel.0,
                wheel_down: wheel.1,
                wheel_left: wheel.2,
                wheel_right: wheel.3,
                shift: self.modifiers.shift_key(),
                alt: self.modifiers.alt_key(),
                ctrl: self.modifiers.control_key(),
                position: Position::new(row as i32, col as u16),
            },
        })
    }
    fn reload(&mut self) -> Result<()> {
        let (config, options) = yinz_cli::setup::Setup::config_without_layout(&self.opts)
            .map_err(|error| anyhow!("{error}"))
            .context("reload native terminal configuration")?;
        self.paints = paints_for(&config, &options);
        self.settings = config.terminal;
        if let Some(surfaces) = &self.surfaces {
            surfaces
                .window
                .set_blur(self.settings.blur.unwrap_or(false));
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::WindowExtMacOS;
                surfaces.window.set_option_as_alt(native_option_as_alt(
                    self.settings.macos_option_as_alt.unwrap_or_default(),
                ));
            }
        }
        self.rebuild_fonts()
    }
    fn handle_message(
        &mut self,
        message: ServerToClientMsg,
        event_loop: &ActiveEventLoop,
    ) -> Result<()> {
        match message {
            ServerToClientMsg::RenderFrame { frame } => {
                let applied = self
                    .state
                    .apply_frame(&frame)
                    .context("apply native render frame")?;
                self.recorder.frame(&frame)?;
                self.received_frame = true;
                self.retained.mark(&applied.damage);
                self.send(ClientToServerMsg::RenderFrameAck { seq: applied.seq })?;
                if let Some(title) = self.state.take_title()
                    && let Some(surfaces) = &self.surfaces
                {
                    surfaces
                        .window
                        .set_title(title.as_deref().unwrap_or("Yinz"));
                }
                if self.state.take_bells() > 0
                    && let Some(surfaces) = &self.surfaces
                {
                    surfaces.window.request_user_attention(Some(
                        winit::window::UserAttentionType::Informational,
                    ));
                }
                if let Some(text) = self.state.take_copied_text() {
                    self.set_clipboard(text);
                }
                self.pacer.schedule();
            },
            ServerToClientMsg::Exit { .. } => event_loop.exit(),
            ServerToClientMsg::SwitchSession { connect_to_session } => {
                // Reuse this window and its GPU resources for the new session.
                if let Some(sender) = &self.sender {
                    session::send(sender, ClientToServerMsg::ClientExited).non_fatal();
                    if let Ok(sender) = sender.lock() {
                        sender
                            .shutdown()
                            .context("close previous native connection")
                            .non_fatal();
                    }
                }
                self.messages.take();
                self.args.create = true;
                let connection = session::connect_to(
                    &self.args,
                    &self.opts,
                    Size {
                        rows: self.state.size().rows,
                        cols: self.state.size().cols,
                    },
                    Some(&connect_to_session),
                )?;
                self.sender = Some(connection.sender);
                let (tx, rx) = mpsc::sync_channel(2);
                let proxy = self.proxy.clone();
                let mut reader = connection.receiver;
                std::thread::Builder::new()
                    .name("native-session-reader".into())
                    .spawn(move || {
                        while let Some((message, _)) = reader.recv_server_msg().msg() {
                            if tx.send(message).is_err() || proxy.send_event(()).is_err() {
                                break;
                            }
                        }
                        drop(tx);
                        let _ = proxy.send_event(());
                    })
                    .context("spawn switched native reader")?;
                self.args.session_name = Some(connection.name);
                self.messages = Some(rx);
                self.state = TerminalState::new(self.state.size().rows, self.state.size().cols);
                self.retained = RetainedScene::new();
                if let Some(surfaces) = &mut self.surfaces {
                    surfaces.renderer.clear_images();
                }
                self.resize()?;
            },
            ServerToClientMsg::ClipboardUpdate { content } => {
                if let Some(text) = crate::emulation::kitty::clipboard_update(&content) {
                    self.set_clipboard(text);
                }
            },
            ServerToClientMsg::ConfigFileUpdated => self.reload()?,
            ServerToClientMsg::SetMouseMode { enabled } => self.mouse_mode = enabled,
            ServerToClientMsg::ForwardQueryToHost {
                token, query_bytes, ..
            } => {
                let metrics = self.cache.metrics();
                let size = self.state.size();
                let reply = crate::host_reply::answer(
                    &query_bytes,
                    session::Geometry {
                        rows: size.rows,
                        cols: size.cols,
                        cell_width: metrics.width as usize,
                        cell_height: metrics.height as usize,
                    },
                    &self.paints,
                    None,
                );
                // Clipboard reads remain opt-in; do not pass the clipboard to host queries.
                self.send(ClientToServerMsg::ForwardedReplyFromHost {
                    token,
                    reply_bytes: reply,
                })?;
            },
            ServerToClientMsg::QueryTerminalSize => self.resize()?,
            // An initial ANSI repaint may already be queued before capability negotiation.
            ServerToClientMsg::Render { .. } => {},
            _ => {},
        }
        Ok(())
    }
    fn draw(&mut self) -> Result<()> {
        let preedit = self.composition.shown();
        self.retained.set_selection(self.selection);
        self.retained
            .set_transparency(crate::render::scene::Transparency {
                opacity: self.settings.opacity.unwrap_or(1.0),
                mode: yinz_types::input::native_terminal::OpacityMode::Background,
            });
        self.retained.refresh(
            SceneInputs {
                state: &self.state,
                phase: self.blink,
                paints: &self.paints,
                cursor: CursorOptions {
                    shape: self.settings.cursor_style.map(|style| match style {
                        yinz_types::input::native_terminal::CursorStyle::Block => {
                            crate::emulation::screen_buffer::CursorShape::Block
                        },
                        yinz_types::input::native_terminal::CursorStyle::Bar => {
                            crate::emulation::screen_buffer::CursorShape::Beam
                        },
                        yinz_types::input::native_terminal::CursorStyle::Underline => {
                            crate::emulation::screen_buffer::CursorShape::Underline
                        },
                    }),
                    blink: self.settings.cursor_blink,
                    hollow: !self.focused,
                },
                hovered_link: None,
                preedit: preedit.as_ref(),
            },
            &mut self.cache,
        );
        let padding = self.padding() as i32;
        if let Some(surfaces) = &mut self.surfaces {
            let size = surfaces.window.inner_size();
            if size.width == 0 || size.height == 0 {
                return Ok(());
            }
            surfaces.renderer.draw_retained(
                &self.retained,
                self.cache.atlases(),
                (size.width, size.height),
                Frame {
                    origin: (padding, padding),
                    ..Default::default()
                },
            );
            let (row, col) = self.state.cursor_position();
            let metrics = self.cache.metrics();
            surfaces.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(
                    padding + (col as u32 * metrics.width) as i32,
                    padding + (row as u32 * metrics.height) as i32,
                ),
                winit::dpi::PhysicalSize::new(metrics.width, metrics.height),
            );
            surfaces.window.pre_present_notify();
            surfaces
                .surface
                .swap_buffers(&surfaces.context)
                .context("present native terminal frame")?;
        }
        self.pacer.drawn(Instant::now());
        Ok(())
    }
    fn event(&mut self, event: WindowEvent, event_loop: &ActiveEventLoop) -> Result<()> {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(surfaces) = &self.surfaces {
                    let size = surfaces.window.inner_size();
                    if let Some(parent) = window_state_path().parent() {
                        std::fs::create_dir_all(parent)
                            .context("create native window state directory")
                            .non_fatal();
                    }
                    let bytes = serde_json::to_vec(&(size.width, size.height))
                        .context("encode native window size")?;
                    std::fs::write(window_state_path(), bytes)
                        .context("remember native window size")
                        .non_fatal();
                }
                event_loop.exit();
            },
            WindowEvent::Resized(_) => self.resize()?,
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale = scale_factor;
                self.rebuild_fonts()?;
            },
            WindowEvent::RedrawRequested => self.draw()?,
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.send(ClientToServerMsg::HostTerminalFocusChanged { focused })?;
                self.retained.mark_everything();
                self.pacer.schedule();
            },
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                self.alt_keys = keyboard::AltKeys::of(&modifiers);
                use yinz_types::keys::KeyModifier;
                let modifiers = [
                    (self.modifiers.shift_key(), KeyModifier::Shift),
                    (self.modifiers.control_key(), KeyModifier::Ctrl),
                    (self.modifiers.alt_key(), KeyModifier::Alt),
                    (self.modifiers.super_key(), KeyModifier::Super),
                ]
                .into_iter()
                .filter_map(|(held, modifier)| held.then_some(modifier))
                .collect();
                self.send(ClientToServerMsg::ModifiersHeld { modifiers })?;
            },
            WindowEvent::Ime(event) => match self.composition.ime(event) {
                Reaction::Commit(chars) => {
                    self.type_text(chars)?;
                    self.retained.mark_everything();
                    self.pacer.schedule();
                },
                Reaction::Redraw => {
                    self.retained.mark_everything();
                    self.pacer.schedule();
                },
                Reaction::Nothing => {},
            },
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } if event.state == ElementState::Pressed => {
                let press = keyboard::Press::of(
                    &event,
                    self.modifiers,
                    keyboard::option_acts_as_alt(
                        Platform::current(),
                        self.settings.macos_option_as_alt.unwrap_or_default(),
                        self.alt_keys,
                    ),
                );
                if event.logical_key == Key::Named(NamedKey::Insert) && self.modifiers.shift_key() {
                    if let Some(chars) = self.get_clipboard() {
                        self.action(Action::Paste {
                            chars,
                            pane_id: None,
                        })?;
                    }
                    return Ok(());
                }
                if self.compose_key(&press)? {
                    return Ok(());
                }
                let shortcut = (self.modifiers.control_key() && self.modifiers.shift_key())
                    || self.modifiers.super_key();
                if shortcut && let Key::Character(key) = &event.logical_key {
                    match key.to_lowercase().as_str() {
                        "c" => {
                            if let Some(selection) = self.selection {
                                self.set_clipboard(selection.text(self.state.screen()));
                                return Ok(());
                            }
                        },
                        "v" => {
                            if let Some(chars) = self.get_clipboard() {
                                self.action(Action::Paste {
                                    chars,
                                    pane_id: None,
                                })?;
                            }
                            return Ok(());
                        },
                        "+" | "=" => {
                            self.zoom = (self.zoom * 1.1).min(4.0);
                            self.rebuild_fonts()?;
                            return Ok(());
                        },
                        "-" | "_" => {
                            self.zoom = (self.zoom / 1.1).max(0.25);
                            self.rebuild_fonts()?;
                            return Ok(());
                        },
                        "0" | ")" => {
                            self.zoom = 1.0;
                            self.rebuild_fonts()?;
                            return Ok(());
                        },
                        _ => {},
                    }
                }
                if event.logical_key == Key::Named(NamedKey::Escape) {
                    self.selection = None;
                }
                if let Some(message) = keyboard::key_message(&press) {
                    self.send(message)?;
                }
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x, position.y);
                if self.buttons.0 && (self.modifiers.shift_key() || !self.mouse_mode) {
                    let (row, col) = self.cell();
                    self.selection = self
                        .selection
                        .map(|selection| selection.extended_to(row, col));
                    self.pacer.schedule();
                } else if self.mouse_mode {
                    self.mouse(MouseEventType::Motion, (false, false, false, false))?;
                }
            },
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => self.buttons.0 = pressed,
                    MouseButton::Right => self.buttons.1 = pressed,
                    MouseButton::Middle => self.buttons.2 = pressed,
                    _ => {},
                }
                if button == MouseButton::Left
                    && self.modifiers.control_key()
                    && pressed
                    && self.settings.open_links.unwrap_or(true)
                {
                    let (row, col) = self.cell();
                    if let Some(link) = links::run_at(self.state.screen(), row, col) {
                        open_link(&link.uri);
                    }
                } else if button == MouseButton::Left
                    && (self.modifiers.shift_key() || !self.mouse_mode)
                {
                    if pressed {
                        let (row, col) = self.cell();
                        self.selection = Some(Selection::at(row, col));
                    }
                    self.pacer.schedule();
                } else if button == MouseButton::Middle
                    && pressed
                    && self.settings.middle_click_paste.unwrap_or(true)
                {
                    if let Some(chars) = self.get_clipboard() {
                        self.action(Action::Paste {
                            chars,
                            pane_id: None,
                        })?;
                    }
                } else if self.mouse_mode {
                    // A release names the button being released, even though it is no
                    // longer in the set of held buttons sent by subsequent motion events.
                    let held = self.buttons;
                    if !pressed {
                        self.buttons = (
                            button == MouseButton::Left,
                            button == MouseButton::Right,
                            button == MouseButton::Middle,
                        );
                    }
                    let result = self.mouse(
                        if pressed {
                            MouseEventType::Press
                        } else {
                            MouseEventType::Release
                        },
                        (false, false, false, false),
                    );
                    self.buttons = held;
                    result?;
                }
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let metrics = self.cache.metrics();
                let (x, y) = self
                    .scroll
                    .steps(delta, metrics.width as f64, metrics.height as f64);
                // Wheel directions are separate events: the server handles only one
                // direction per event, so combining axes would lose horizontal steps.
                for _ in 0..y.abs() {
                    self.mouse(MouseEventType::Press, (y > 0, y < 0, false, false))?;
                }
                for _ in 0..x.abs() {
                    self.mouse(MouseEventType::Press, (false, false, x < 0, x > 0))?;
                }
            },
            _ => {},
        }
        Ok(())
    }
    fn type_text(&mut self, text: String) -> Result<()> {
        if let Some(message) = keyboard::typed_text_message(text) {
            self.selection = None;
            self.send(message)?;
        }
        Ok(())
    }
    fn compose_key(&mut self, press: &keyboard::Press) -> Result<bool> {
        use crate::input::composition::Gate;
        let gate = self.composition.gate();
        if gate == Gate::Composing {
            return Ok(true);
        }
        if let Key::Dead(accent) = press.logical {
            match keyboard::dead_key_press(press) {
                keyboard::DeadKeyOutcome::Wait => {
                    self.composition.dead_key(accent);
                },
                keyboard::DeadKeyOutcome::Type(text) => {
                    self.composition.resolve();
                    self.type_text(text)?;
                },
            }
            self.retained.mark_everything();
            self.pacer.schedule();
            return Ok(true);
        }
        if gate == Gate::Resolving {
            if keyboard::leaves_the_accent_pending(press) {
                return Ok(true);
            }
            self.composition.resolve();
            self.retained.mark_everything();
            self.pacer.schedule();
            match keyboard::resolve_pending_accent(press) {
                keyboard::Resolution::Key => {
                    if let Some(message) = keyboard::key_message(press) {
                        self.send(message)?;
                    }
                },
                keyboard::Resolution::Type(text) => self.type_text(text)?,
                keyboard::Resolution::TypeThenKey(text) => {
                    self.type_text(text)?;
                    if let Some(message) = keyboard::key_message(press) {
                        self.send(message)?;
                    }
                },
                keyboard::Resolution::Nothing => {},
            }
            return Ok(true);
        }
        if let Some(text) =
            keyboard::unidentified_text(press).or_else(|| keyboard::leftover_accent(press))
        {
            self.type_text(text)?;
            return Ok(true);
        }
        Ok(false)
    }
    fn get_clipboard(&self) -> Option<String> {
        self.clipboard
            .lock()
            .ok()
            .and_then(|mut clipboard| clipboard.get())
    }
    fn set_clipboard(&self, text: String) {
        if let Ok(mut clipboard) = self.clipboard.lock() {
            clipboard.set(&text);
        }
    }
    fn fail(&mut self, error: anyhow::Error, event_loop: &ActiveEventLoop) {
        self.error = Some(error);
        event_loop.exit();
    }
}
impl ApplicationHandler<()> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surfaces.is_none()
            && let Err(error) = self.open(event_loop)
        {
            self.fail(error, event_loop);
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Err(error) = self.event(event, event_loop) {
            self.fail(error, event_loop);
        }
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        if event_loop.exiting() {
            return;
        }
        // The reader posts one wake for each bounded-queue message. Handle one
        // per event so continuous output cannot grow a backlog of stale wakes
        // while an unbounded drain loop keeps the window from drawing.
        match receive_message(self.messages.as_ref()) {
            Ok(Some(message)) => {
                if let Err(error) = self.handle_message(message, event_loop) {
                    self.fail(error, event_loop);
                }
            },
            Err(error) => self.fail(error, event_loop),
            Ok(None) => {},
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if !self.received_frame && now.duration_since(self.started) >= Duration::from_secs(10) {
            self.fail(anyhow!("server did not send structured rendering; restart the session with this Yinz build"),event_loop);
            return;
        }
        if self
            .args
            .duration_secs
            .is_some_and(|seconds| now.duration_since(self.started) >= Duration::from_secs(seconds))
        {
            event_loop.exit();
            return;
        }
        if now >= self.next_blink {
            self.blink = if self.blink == BlinkPhase::On {
                BlinkPhase::Off
            } else {
                BlinkPhase::On
            };
            self.next_blink = now + Duration::from_millis(500);
            self.retained.mark_everything();
            self.pacer.schedule();
        }
        let mut wake = self.next_blink;
        match self.pacer.decide(now) {
            Decision::Now => {
                if let Some(surfaces) = &self.surfaces {
                    surfaces.window.request_redraw();
                }
            },
            Decision::At(deadline) => wake = wake.min(deadline),
            Decision::Idle => {},
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}
fn window_state_path() -> std::path::PathBuf {
    yinz_runtime::consts::YINZ_CACHE_DIR.join("native-terminal-window.json")
}
fn paints_for(
    config: &yinz_config::config::Config,
    options: &yinz_types::input::options::Options,
) -> Paints {
    options
        .theme
        .as_deref()
        .and_then(|name| config.themes.get_theme(name))
        .and_then(|theme| theme.terminal.as_ref())
        .map_or_else(Paints::default, |theme| Paints {
            foreground: theme.foreground.into(),
            background: theme.background.into(),
            ansi: theme.ansi.map(Into::into),
            cursor: None,
        })
}

#[cfg(test)]
#[path = "unit/window_tests.rs"]
mod tests;
