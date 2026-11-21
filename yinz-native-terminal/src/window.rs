use crate::emulation::color::Paints;
use crate::emulation::screen_buffer::CursorShape;
use crate::emulation::selection::LocalSelection;
use crate::emulation::terminal::{FrameApplied, TerminalState};
use crate::input::clipboard::{Clipboard, ClipboardHandle};
use crate::input::composition::{Composition, Preedit, Reaction};
use crate::input::{keyboard, links};
use crate::platform::Platform;
use crate::render::atlas::GlyphCache;
use crate::render::cursor_trail::{CursorTrail, TrailMesh};
use crate::render::pacing::{Decision, Pacer};
use crate::render::renderer::{Frame, Renderer};
use crate::render::retained::RetainedScene;
use crate::render::scene::{
    notice_scene, BlinkPhase, CursorOptions, Margins, Notice, RowContext, RowScene, SceneInputs,
};
use crate::session::{self, Recorder, Sender};
use crate::text::font::{points_to_pixels, CellChange, FontOptions};
use glutin::config::ConfigTemplateBuilder;
use glutin::context::PossiblyCurrentContext;
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::DisplayBuilder;
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize, Size as WindowSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};
use yinz_cli::args::{CliArgs, TerminalArgs};
use yinz_config::config::Config;
use yinz_ipc::ipc::{ClientToServerMsg, ExitReason, PixelDimensions, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::SessionConnection;
use yinz_types::input::actions::Action;
use yinz_types::input::native_terminal::{StartupMode, TerminalConfig};
use yinz_types::input::options::Options;
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

impl Surfaces {
    fn size(&self) -> (u32, u32) {
        let size = self.window.inner_size();
        (size.width, size.height)
    }
}

struct App {
    endpoint: session::Endpoint,
    handshake: session::Handshake,
    surfaces: Option<Surfaces>,
    wake_reader: ReaderWake,
    args: TerminalArgs,
    opts: CliArgs,
    settings: TerminalConfig,
    paints: Paints,
    cache: GlyphCache,
    active_fonts: FontOptions,
    connection_geometry: ConnectionGeometry,
    last_measured_geometry: Option<Geometry>,
    retained: RetainedScene,
    state: TerminalState,
    sender: Option<AttachedSender>,
    messages: Option<Receiver<ServerToClientMsg>>,
    connection_id: u64,
    next_connection_id: u64,
    pending_switch: Option<PendingSwitch>,
    switch_notice: Option<String>,
    canceled_switch_escape: bool,
    recorder: Recorder,
    modifiers: ModifiersState,
    alt_keys: keyboard::AltKeys,
    composition: Composition,
    clipboard: ClipboardHandle,
    pointer: (f64, f64),
    scroll: ScrollAccumulator,
    buttons: (bool, bool, bool),
    buttons_pressed_before_repaint: HashSet<MouseButton>,
    selection: LocalSelection,
    mouse_mode: bool,
    focused: bool,
    pacer: Pacer,
    cursor_trail: CursorTrail,
    trail_mesh: TrailMesh,
    blink: BlinkPhase,
    next_blink: Instant,
    scale: f64,
    zoom: f32,
    startup_mode: StartupMode,
    opening: Option<OpeningGeometry>,
    recorded_window: Option<serde_json::Value>,
    started: Instant,
    error: Option<anyhow::Error>,
}

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
type ReaderWake = Arc<dyn Fn(u64) -> bool + Send + Sync>;

struct AttachedSender(Sender);

impl Drop for AttachedSender {
    fn drop(&mut self) {
        if let Ok(mut sender) = self.0.lock() {
            sender
                .send_client_msg_until(
                    ClientToServerMsg::ClientExited,
                    Instant::now() + Duration::from_millis(100),
                )
                .context("detach native client")
                .non_fatal();
            sender
                .shutdown()
                .context("close native connection")
                .non_fatal();
        }
    }
}

fn read_session(
    mut reader: yinz_ipc::ipc::IpcReceiverWithContext<ServerToClientMsg>,
    tx: mpsc::SyncSender<ServerToClientMsg>,
    wake: impl Fn() -> bool,
) {
    use yinz_ipc::ipc::{ExitReason, IpcRecv};
    loop {
        let message = match reader.recv_native_server_msg(None) {
            IpcRecv::Msg(message, _) => message,
            IpcRecv::Idle => continue,
            IpcRecv::Eof => break,
            IpcRecv::Malformed => {
                let _ = tx.send(ServerToClientMsg::Exit {
                    exit_reason: ExitReason::Error("Malformed native attachment message".into()),
                });
                break;
            },
        };
        if tx.send(message).is_err() || !wake() {
            return;
        }
    }
    drop(tx);
    wake();
}

struct PendingSwitch {
    handshake: session::Handshake,
    id: u64,
    deadline: Instant,
    destination: String,
    opening: Option<Receiver<Result<(String, AttachedSender)>>>,
    sender: Option<AttachedSender>,
    messages: Receiver<ServerToClientMsg>,
    mouse_mode: bool,
    reload: bool,
}

struct SwitchFrame {
    state: TerminalState,
    frame: Vec<u8>,
    applied: FrameApplied,
}

impl PendingSwitch {
    fn stage(app: &App, id: u64, target: SessionConnection, destination: String) -> Result<Self> {
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        let (opened_tx, opened_rx) = mpsc::sync_channel(1);
        let (tx, messages) = mpsc::sync_channel(2);
        let args = app.args.clone();
        let opts = app.opts.clone();
        let size = app.switch_grid(app.live_geometry()?);
        let wake_reader = app.wake_reader.clone();
        std::thread::Builder::new()
            .name("native-switch-reader".into())
            .spawn(move || {
                let wake = || wake_reader(id);
                match session::connect_to(&args, &opts, size, Some(&target), Some(deadline)) {
                    Ok(connection) => {
                        let opened = Ok((connection.name, AttachedSender(connection.sender)));
                        if opened_tx.send(opened).is_ok() && wake() {
                            read_session(connection.receiver, tx, wake);
                        }
                    },
                    Err(error) => {
                        if opened_tx.send(Err(error)).is_ok() {
                            wake();
                        }
                    },
                }
            })
            .context("start native session switch")?;
        Ok(Self {
            handshake: Default::default(),
            id,
            deadline,
            destination,
            opening: Some(opened_rx),
            sender: None,
            messages,
            mouse_mode: true,
            reload: false,
        })
    }

    fn check_deadline(&self, now: Instant) -> Result<()> {
        anyhow::ensure!(
            now < self.deadline,
            "timed out waiting for destination: {}",
            self.handshake
                .timeout(&session::Endpoint::Local, &self.destination)
        );
        Ok(())
    }

    fn poll(
        &mut self,
        now: Instant,
        size: Size,
        cell: crate::text::font::CellMetrics,
        paints: &Paints,
    ) -> Result<Option<SwitchFrame>> {
        self.check_deadline(now)?;
        if let Some(opening) = &self.opening {
            match opening.try_recv() {
                Ok(opened) => {
                    let (name, sender) = opened?;
                    self.destination = name;
                    self.sender = Some(sender);
                    self.opening = None;
                },
                Err(mpsc::TryRecvError::Empty) => {},
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("the destination stopped opening its connection")
                },
            }
            return Ok(None);
        }
        let Some(message) = receive_message(Some(&self.messages))? else {
            return Ok(None);
        };
        let sender = &self
            .sender
            .as_ref()
            .context("missing destination sender")?
            .0;
        if self.handshake.control(
            &message,
            &session::Endpoint::Local,
            &self.destination,
            sender,
            self.deadline,
        )? {
            return Ok(None);
        }
        match message {
            ServerToClientMsg::RenderFrame { frame } => {
                let decoded = yinz_types::structured_render::decode(&frame)
                    .map_err(|error| anyhow!("{error:?}"))
                    .context("decode the destination's first frame")?;
                anyhow::ensure!(
                    decoded.header().full_repaint(),
                    "the destination did not send a full first frame"
                );
                let mut state = TerminalState::new(size.rows, size.cols);
                state.set_cell_size(cell.width, cell.height);
                self.handshake.begin_frames(&mut state)?;
                let applied = state
                    .apply_frame(&frame)
                    .context("apply the destination's first frame")?;
                Ok(Some(SwitchFrame {
                    state,
                    frame,
                    applied,
                }))
            },
            ServerToClientMsg::Exit { exit_reason } => {
                anyhow::bail!("the destination exited before rendering: {exit_reason}")
            },
            ServerToClientMsg::SetMouseMode { enabled } => {
                self.mouse_mode = enabled;
                Ok(None)
            },
            ServerToClientMsg::ConfigFileUpdated => {
                self.reload = true;
                Ok(None)
            },
            ServerToClientMsg::ForwardQueryToHost {
                token, query_bytes, ..
            } => {
                let reply = window_host_reply(
                    token,
                    &query_bytes,
                    host_query_geometry(None, size, cell),
                    paints,
                );
                sender
                    .lock()
                    .map_err(|_| anyhow!("destination IPC sender lock poisoned"))?
                    .send_client_msg_until(reply, self.deadline)
                    .context("answer destination host query")?;
                Ok(None)
            },
            _ => Ok(None),
        }
    }

    fn report_ready(&self, geometry: Option<Geometry>, focused: bool, seq: u64) -> Result<()> {
        let sender = self
            .sender
            .as_ref()
            .context("destination connection is missing")?;
        let mut sender = sender
            .0
            .lock()
            .map_err(|_| anyhow!("destination IPC sender lock poisoned"))?;
        for message in [
            ClientToServerMsg::HostTerminalFocusChanged { focused },
            ClientToServerMsg::RenderFrameAck { seq },
        ]
        .into_iter()
        .chain(
            geometry
                .into_iter()
                .flat_map(|geometry| geometry.messages(None)),
        ) {
            sender
                .send_client_msg_until(message, self.deadline)
                .context("confirm destination connection")?;
        }
        sender
            .set_send_timeout(None)
            .context("restore native sender timeout")
    }
}

fn switch_notice(destination: &str, error: &anyhow::Error) -> String {
    format!("Could not switch to {destination}. Esc dismisses.\n{error:#}")
}

fn load_native_config(opts: &CliArgs) -> Result<(Config, Options)> {
    yinz_cli::setup::Setup::native_window_config(opts).map_err(|error| {
        let detail = error.diagnostic_message();
        if matches!(error, yinz_config::config::ConfigError::IoPath(..)) {
            anyhow!("{detail}. Restore a readable regular file at the reported path.")
        } else {
            anyhow!("{detail}")
        }
    })
}

pub fn run(
    args: TerminalArgs,
    opts: CliArgs,
    connection: Option<session::Connection>,
) -> Result<()> {
    let config = load_native_config(&opts).context("load native settings")?;
    if connection.is_none()
        && let Some(path) = &args.record
    {
        Recorder::check_destination(path)
            .with_context(|| format!("cannot record native frames to {}", path.display()))?;
    }
    let event_loop = EventLoop::<u64>::with_user_event()
        .build()
        .context("initialize native event loop")?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(
        args,
        opts,
        config,
        Arc::new(move |id| proxy.send_event(id).is_ok()),
        Clipboard::open,
    )?;
    // A bounded queue holds at most two messages between IPC and the window thread.
    // The event proxy only wakes the loop; frames remain in this bounded queue.
    app.open_connection(connection)?;
    event_loop
        .run_app(&mut app)
        .context("run native event loop")?;
    match app.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

pub(crate) fn font_options(settings: &TerminalConfig, zoom: f32) -> FontOptions {
    fn named(family: &Option<String>) -> Option<String> {
        family.clone().filter(|value| !value.trim().is_empty())
    }
    FontOptions {
        family: named(&settings.font_family),
        bold_family: named(&settings.font_bold),
        italic_family: named(&settings.font_italic),
        bold_italic_family: named(&settings.font_bold_italic),
        size: points_to_pixels(settings.font_size.unwrap_or(12.0)) * zoom,
        weight: settings.font_weight.unwrap_or(400),
        ligatures: settings.font_ligatures.unwrap_or(true),
        system_fonts: settings.system_fonts.unwrap_or(true),
        cell: crate::text::font::CellAdjust {
            line_height: settings.line_height.unwrap_or(1.0),
            cell_width: settings.cell_width.unwrap_or(1.0),
            baseline_offset: settings.baseline_offset.unwrap_or(0) as f32 * zoom,
            underline_offset: settings.underline_offset.unwrap_or(0) as f32 * zoom,
            underline_thickness: settings.underline_thickness.unwrap_or(0) as f32 * zoom,
        },
        features: font_features(settings),
    }
}

/// The configured OpenType features in list order, without deduplication, as
/// upstream passes them to the shaper. KDL validates every entry; an invalid one
/// from another constructor is reported, never silently dropped.
fn font_features(settings: &TerminalConfig) -> Vec<crate::text::font::FontFeature> {
    settings
        .font_features
        .iter()
        .flatten()
        .filter_map(|text| {
            let feature = crate::text::font::FontFeature::parse(text);
            if feature.is_none() {
                log::warn!(
                    "native terminal font_features: ignoring {text:?}, which is not a font feature"
                );
            }
            feature
        })
        .collect()
}

// Store the physical options actually used by the cache, after all scaling.
pub(crate) fn effective_font_options(
    settings: &TerminalConfig,
    zoom: f32,
    scale: f64,
) -> FontOptions {
    let mut options = font_options(settings, zoom).scaled(scale);
    let bounded = options.size.clamp(
        crate::text::font::MIN_FONT_SIZE,
        crate::text::font::MAX_FONT_SIZE,
    );
    if bounded != options.size {
        log::warn!(
            "native font raster size {} adjusted to {bounded} pixels (zoom/monitor scale bounds)",
            options.size
        );
        options.size = bounded;
    }
    options
}

pub(crate) enum FontUpdate {
    Unchanged,
    /// Only the ligature setting or feature list changed: re-derive the shaper's
    /// features and drop shaped runs, keeping faces, rasters and atlases.
    ShapingOnly,
    CellOnly,
    Replacement(Box<GlyphCache>),
}

pub(crate) fn prepare_fonts(active: &FontOptions, requested: &FontOptions) -> Result<FontUpdate> {
    if active == requested {
        return Ok(FontUpdate::Unchanged);
    }
    let mut comparison = active.clone();
    comparison.ligatures = requested.ligatures;
    comparison.features.clone_from(&requested.features);
    if comparison == *requested {
        return Ok(FontUpdate::ShapingOnly);
    }
    comparison.cell = requested.cell;
    if comparison == *requested {
        return Ok(FontUpdate::CellOnly);
    }
    Ok(FontUpdate::Replacement(Box::new(GlyphCache::new(
        requested
            .stack(1.0)
            .context("prepare replacement native fonts")?,
    ))))
}

pub(crate) fn apply_fonts(
    active: &mut FontOptions,
    cache: &mut GlyphCache,
    retained: &mut RetainedScene,
    requested: FontOptions,
    update: FontUpdate,
) {
    match update {
        FontUpdate::Unchanged => return,
        FontUpdate::ShapingOnly => cache.update_shaping(&requested),
        FontUpdate::CellOnly => {
            let changed = cache.update_cell(&requested);
            // A metric change alone keeps runs when the width is unchanged, so a
            // combined shaping change must still drop runs shaped with old features.
            let shaping_changed =
                active.ligatures != requested.ligatures || active.features != requested.features;
            if shaping_changed {
                cache.update_shaping(&requested);
            }
            if changed != CellChange::GlyphPlacement && !shaping_changed {
                *active = requested;
                return;
            }
        },
        FontUpdate::Replacement(replacement) => *cache = *replacement,
    }
    *active = requested;
    // The atlas identity stays valid for a shaping change, but retained row
    // vertices still contain the previous run's glyphs.
    retained.mark(&crate::emulation::damage::Damage::Everything);
}

pub(crate) fn apply_settings(
    settings: &mut TerminalConfig,
    paints: &mut Paints,
    retained: &mut RetainedScene,
    next_settings: TerminalConfig,
    next_paints: Paints,
) {
    // Font metrics determine baseline and underline damage, including clamping.
    // Keep other settings and queued font/terminal damage in the comparison.
    let mut comparison = settings.clone();
    comparison.baseline_offset = next_settings.baseline_offset;
    comparison.underline_offset = next_settings.underline_offset;
    comparison.underline_thickness = next_settings.underline_thickness;
    comparison.initial_columns = next_settings.initial_columns;
    comparison.initial_rows = next_settings.initial_rows;
    comparison.cursor_trail = next_settings.cursor_trail;
    if comparison != next_settings || *paints != next_paints {
        retained.mark_everything();
    }
    *settings = next_settings;
    *paints = next_paints;
}

pub(crate) struct PreparedReload {
    settings: TerminalConfig,
    paints: Paints,
    requested: FontOptions,
    update: FontUpdate,
}

impl PreparedReload {
    pub(crate) fn load(
        opts: &CliArgs,
        active: &FontOptions,
        zoom: f32,
        scale: f64,
    ) -> Result<Self> {
        let (config, options) =
            load_native_config(opts).context("reload native terminal configuration")?;
        let requested = effective_font_options(&config.terminal, zoom, scale);
        let update = prepare_fonts(active, &requested)?;
        Ok(Self {
            paints: paints_for(&config, &options),
            settings: config.terminal,
            requested,
            update,
        })
    }

    pub(crate) fn apply(
        self,
        active: &mut FontOptions,
        cache: &mut GlyphCache,
        retained: &mut RetainedScene,
        settings: &mut TerminalConfig,
        paints: &mut Paints,
    ) {
        apply_fonts(active, cache, retained, self.requested, self.update);
        apply_settings(settings, paints, retained, self.settings, self.paints);
    }
}

fn drawable_size(size: Option<(u32, u32)>) -> Option<(u32, u32)> {
    size.filter(|&(width, height)| width > 0 && height > 0)
}

pub(crate) fn refresh_drawable(
    size: Option<(u32, u32)>,
    retained: &mut RetainedScene,
    inputs: SceneInputs<'_>,
    cache: &mut GlyphCache,
) -> Option<(u32, u32)> {
    let size = drawable_size(size)?;
    retained.refresh(inputs, cache);
    Some(size)
}

fn redraw_decision(pacer: &Pacer, size: Option<(u32, u32)>, now: Instant) -> Decision {
    if drawable_size(size).is_some() {
        pacer.decide(now)
    } else {
        Decision::Idle
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    cell: SizeInPixels,
    grid: Size,
}

const STARTUP_CELL_LIMIT: usize = 1_048_576;
const STARTUP_PIXEL_LIMIT: usize = 33_177_600;
const STARTUP_AXIS_LIMIT: u32 = 16_384;

fn graphical_opening_size(args: &TerminalArgs) -> Size {
    fit_size(args.opening_size(), STARTUP_CELL_LIMIT)
}

fn fit_size(size: Size, limit: usize) -> Size {
    if size.rows as u64 * size.cols as u64 <= limit as u64 {
        return size;
    }
    let cols =
        ((limit as u64 * size.cols as u64 / size.rows as u64).isqrt() as usize).clamp(1, size.cols);
    Size {
        cols,
        rows: (limit / cols).clamp(1, size.rows),
    }
}

#[derive(Clone, Copy)]
enum StartupAxis {
    Cells(u16),
    Pixels(u32),
}

impl StartupAxis {
    fn resolve(
        cli: Option<u16>,
        remembered: Option<u32>,
        configured: Option<u16>,
        default: u16,
    ) -> Self {
        cli.map(Self::Cells)
            .or_else(|| remembered.map(Self::Pixels))
            .unwrap_or_else(|| Self::Cells(configured.unwrap_or(default)))
    }

    /// `padding` is the axis sum: left + right for width, top + bottom for height.
    fn extent(self, cell: u32, padding: u32) -> u32 {
        match self {
            Self::Cells(cells) => u32::from(cells)
                .saturating_mul(cell)
                .saturating_add(padding),
            Self::Pixels(pixels) => pixels,
        }
    }
}

struct StartupSize {
    width: StartupAxis,
    height: StartupAxis,
}

impl StartupSize {
    fn resolve(
        args: &TerminalArgs,
        settings: &TerminalConfig,
        remembered: Option<(u32, u32)>,
    ) -> Self {
        Self {
            width: StartupAxis::resolve(
                args.cols,
                remembered.map(|size| size.0),
                settings.initial_columns,
                120,
            ),
            height: StartupAxis::resolve(
                args.rows,
                remembered.map(|size| size.1),
                settings.initial_rows,
                40,
            ),
        }
    }

    fn has_pixel_axis(&self) -> bool {
        matches!(self.width, StartupAxis::Pixels(_))
            || matches!(self.height, StartupAxis::Pixels(_))
    }

    fn extent(
        &self,
        metrics: crate::text::font::CellMetrics,
        padding: PaddingLayout,
        ceiling: PhysicalSize<u32>,
    ) -> PhysicalSize<u32> {
        let width = self
            .width
            .extent(metrics.width, padding.horizontal())
            .clamp(1, ceiling.width);
        let height = self
            .height
            .extent(metrics.height, padding.vertical())
            .clamp(1, ceiling.height);
        let pixels = fit_size(
            Size {
                cols: width as usize,
                rows: height as usize,
            },
            STARTUP_PIXEL_LIMIT,
        );
        let width = pixels.cols as u32;
        let height = pixels.rows as u32;
        let Size { cols, rows } = Geometry::grid_size(width, height, padding, metrics);
        let fitted = fit_size(Size { cols, rows }, STARTUP_CELL_LIMIT);
        PhysicalSize::new(
            if fitted.cols == cols {
                width
            } else {
                width.min(fitted.cols as u32 * metrics.width + padding.horizontal())
            },
            if fitted.rows == rows {
                height
            } else {
                height.min(fitted.rows as u32 * metrics.height + padding.vertical())
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowBackend {
    Wayland,
    X11,
    Mac,
}

struct StartupCorrection {
    target: StartupSize,
    hint: WindowSize,
    wayland_ceiling: Option<LogicalSize<u32>>,
}

impl StartupCorrection {
    fn observe_wayland_grant(&mut self, granted: PhysicalSize<u32>, scale: f64) {
        if granted.width == 0 || granted.height == 0 {
            return;
        }
        let logical = granted.to_logical::<u32>(scale);
        self.wayland_ceiling = Some(self.wayland_ceiling.map_or(logical, |previous| {
            LogicalSize::new(
                previous.width.min(logical.width),
                previous.height.min(logical.height),
            )
        }));
    }

    fn request(
        &self,
        granted: PhysicalSize<u32>,
        monitor: PhysicalSize<u32>,
        scale: f64,
        padding: PaddingLayout,
        metrics: crate::text::font::CellMetrics,
    ) -> Option<(WindowSize, PhysicalSize<u32>)> {
        let bounds = if let Some(ceiling) = self.wayland_ceiling {
            let ceiling = ceiling.to_physical::<u32>(scale);
            PhysicalSize::new(
                monitor
                    .width
                    .clamp(1, STARTUP_AXIS_LIMIT)
                    .min(ceiling.width),
                monitor
                    .height
                    .clamp(1, STARTUP_AXIS_LIMIT)
                    .min(ceiling.height),
            )
        } else {
            correction_bounds(granted, self.hint.to_physical(scale), monitor)
        };
        if bounds.width == 0 || bounds.height == 0 {
            return None;
        }
        let measured = self.target.extent(metrics, padding, bounds);
        let (request, expected) = if self.wayland_ceiling.is_some() {
            let logical =
                wayland_correction(&self.target, measured, bounds, padding, metrics, scale)?;
            (
                WindowSize::Logical(logical.cast()),
                logical.to_physical(scale),
            )
        } else {
            (WindowSize::Physical(measured), measured)
        };
        (expected != granted).then_some((request, expected))
    }
}

const OPENING_QUIET: Duration = Duration::from_millis(120);
const OPENING_PLACEMENT_LIMIT: Duration = Duration::from_millis(400);
const OPENING_GRANT_LIMIT: Duration = Duration::from_millis(250);

struct OpeningGeometry {
    backend: WindowBackend,
    phase: OpeningPhase,
}

enum OpeningPhase {
    Placement {
        correction: StartupCorrection,
        quiet_until: Instant,
        deadline: Instant,
    },
    Grant {
        expected: PhysicalSize<u32>,
        deadline: Instant,
    },
}

impl OpeningGeometry {
    fn new(backend: WindowBackend, correction: StartupCorrection, now: Instant) -> Self {
        Self {
            backend,
            phase: OpeningPhase::Placement {
                correction,
                quiet_until: now + OPENING_QUIET,
                deadline: now + OPENING_PLACEMENT_LIMIT,
            },
        }
    }

    fn placement_signal(&mut self, now: Instant) {
        if self.backend == WindowBackend::X11
            && let OpeningPhase::Placement {
                quiet_until,
                deadline,
                ..
            } = &mut self.phase
        {
            *quiet_until = (now + OPENING_QUIET).min(*deadline);
        }
    }

    fn deadline(&self) -> Instant {
        match self.phase {
            OpeningPhase::Placement { deadline, .. } | OpeningPhase::Grant { deadline, .. } => {
                deadline
            },
        }
    }

    fn wake_at(&self) -> Instant {
        match self.phase {
            OpeningPhase::Placement {
                quiet_until,
                deadline,
                ..
            } if self.backend == WindowBackend::X11 => quiet_until.min(deadline),
            OpeningPhase::Placement { deadline, .. } | OpeningPhase::Grant { deadline, .. } => {
                deadline
            },
        }
    }

    fn placement_ready(&self, now: Instant, output_ready: bool) -> bool {
        matches!(self.phase, OpeningPhase::Placement { .. })
            && (now >= self.wake_at()
                || self.backend == WindowBackend::Mac
                || (self.backend == WindowBackend::Wayland && output_ready))
    }

    fn validate(&self, extent: PhysicalSize<u32>, geometry: Geometry) -> Result<()> {
        check_startup_extent(extent, geometry.grid)
    }

    fn grant_settled(&self, now: Instant, actual: PhysicalSize<u32>) -> bool {
        matches!(self.phase, OpeningPhase::Grant { expected, deadline } if actual == expected || now >= deadline)
    }
}

struct ConnectionGeometry {
    seed: Size,
    first_frame_deadline: Instant,
    received_frame: bool,
    reported: Option<Geometry>,
}

impl ConnectionGeometry {
    fn new(seed: Size, now: Instant) -> Self {
        Self {
            seed,
            first_frame_deadline: now + FIRST_FRAME_TIMEOUT,
            received_frame: false,
            reported: None,
        }
    }

    fn switch_grid(&self, measured: Option<Geometry>) -> Size {
        measured
            .or(self.reported)
            .map_or(self.seed, |geometry| geometry.grid)
    }

    fn messages(&self, geometry: Geometry) -> Vec<ClientToServerMsg> {
        if self.received_frame {
            geometry.messages(self.reported)
        } else {
            vec![]
        }
    }
}

fn startup_mode(args: &TerminalArgs, settings: &TerminalConfig) -> StartupMode {
    args.startup_mode
        .or(settings.startup_mode)
        .unwrap_or_default()
}

fn remembered_size(mode: StartupMode, path: &std::path::Path) -> Option<(u32, u32)> {
    if mode != StartupMode::Remember {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let size: (u32, u32) = serde_json::from_slice(&bytes).ok()?;
    let valid = 100..=STARTUP_AXIS_LIMIT;
    (valid.contains(&size.0) && valid.contains(&size.1)).then_some(size)
}

/// Physical padding for one scale: each side resolved by
/// [`TerminalConfig::padding_sides`] and rounded on its own, plus balancing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PaddingLayout {
    margins: Margins,
    balance: bool,
}

impl PaddingLayout {
    fn horizontal(self) -> u32 {
        self.margins.left.saturating_add(self.margins.right)
    }

    fn vertical(self) -> u32 {
        self.margins.top.saturating_add(self.margins.bottom)
    }

    /// Top-left pixel of the grid inside a `width`×`height` window. Balancing
    /// adds half of the leftover room on each axis; integer division leaves
    /// an odd pixel on the right or bottom.
    fn origin(
        self,
        width: u32,
        height: u32,
        grid: Size,
        metrics: crate::text::font::CellMetrics,
    ) -> (i32, i32) {
        let (mut x, mut y) = (self.margins.left, self.margins.top);
        if self.balance {
            let room_x = width.saturating_sub(self.horizontal());
            let room_y = height.saturating_sub(self.vertical());
            x += room_x.saturating_sub((grid.cols as u32).saturating_mul(metrics.width.max(1))) / 2;
            y +=
                room_y.saturating_sub((grid.rows as u32).saturating_mul(metrics.height.max(1))) / 2;
        }
        (x as i32, y as i32)
    }
}

fn padding_layout(settings: &TerminalConfig, scale: f64) -> PaddingLayout {
    let sides = settings.padding_sides();
    let physical = |logical: f32| (f64::from(logical) * scale).round().max(0.0) as u32;
    PaddingLayout {
        margins: Margins {
            left: physical(sides.left),
            top: physical(sides.top),
            right: physical(sides.right),
            bottom: physical(sides.bottom),
        },
        balance: settings.padding_balance_enabled(),
    }
}

fn correction_bounds(
    granted: PhysicalSize<u32>,
    expected: PhysicalSize<u32>,
    monitor: PhysicalSize<u32>,
) -> PhysicalSize<u32> {
    let ceiling = |granted: u32, hint: u32, monitor: u32| {
        let limit = monitor.clamp(1, STARTUP_AXIS_LIMIT);
        if granted > 0 && granted.saturating_add(1) < hint {
            limit.min(granted)
        } else {
            limit
        }
    };
    PhysicalSize::new(
        ceiling(granted.width, expected.width, monitor.width),
        ceiling(granted.height, expected.height, monitor.height),
    )
}

// Winit uses whole logical pixels on Wayland. Prefer a cell-preserving
// representation for cell axes and the nearest physical match for saved axes.
fn wayland_correction(
    target: &StartupSize,
    size: PhysicalSize<u32>,
    bounds: PhysicalSize<u32>,
    padding: PaddingLayout,
    metrics: crate::text::font::CellMetrics,
    scale: f64,
) -> Option<LogicalSize<u32>> {
    let physical = |logical: u32| {
        LogicalSize::new(logical, logical)
            .to_physical::<u32>(scale)
            .width
    };
    let at_least = |pixels: u32| {
        let logical = ((f64::from(pixels) - 0.5) / scale).ceil().max(1.0) as u32;
        if physical(logical) < pixels {
            logical + 1
        } else {
            logical
        }
    };
    let at_most = |pixels: u32| {
        let next = at_least(pixels + 1);
        next - 1
    };
    let options = |axis, pixels, bound, cell: u32, cells: usize, padding: u32| {
        let lower = at_most(pixels);
        let preferred = match axis {
            StartupAxis::Cells(_) => at_least((cells as u32 * cell + padding).min(pixels)),
            StartupAxis::Pixels(_) => {
                let upper = at_least(pixels);
                if lower > 0 && pixels - physical(lower) <= physical(upper) - pixels {
                    lower
                } else {
                    upper
                }
            },
        };
        [preferred.min(at_most(bound)), lower]
    };
    let grid = Geometry::grid_size(size.width, size.height, padding, metrics);
    let widths = options(
        target.width,
        size.width,
        bounds.width,
        metrics.width,
        grid.cols,
        padding.horizontal(),
    );
    let heights = options(
        target.height,
        size.height,
        bounds.height,
        metrics.height,
        grid.rows,
        padding.vertical(),
    );
    widths
        .into_iter()
        .flat_map(|width| heights.map(|height| LogicalSize::new(width, height)))
        .find(|logical| {
            let rounded: PhysicalSize<u32> = logical.to_physical(scale);
            rounded.width <= bounds.width
                && rounded.height <= bounds.height
                && Geometry::calculate(rounded.width, rounded.height, padding, metrics)
                    .is_some_and(|geometry| check_startup_extent(rounded, geometry.grid).is_ok())
        })
}

fn client_sized(mode: StartupMode) -> bool {
    matches!(mode, StartupMode::Windowed | StartupMode::Remember)
}

fn check_startup_extent(size: PhysicalSize<u32>, grid: Size) -> Result<()> {
    if size.width > STARTUP_AXIS_LIMIT
        || size.height > STARTUP_AXIS_LIMIT
        || u64::from(size.width) * u64::from(size.height) > STARTUP_PIXEL_LIMIT as u64
        || grid.rows * grid.cols > STARTUP_CELL_LIMIT
    {
        return Err(anyhow!(
            "native startup window is {}×{} pixels ({}×{} cells), exceeding the supported startup extent",
            size.width, size.height, grid.cols, grid.rows
        ));
    }
    Ok(())
}

impl Geometry {
    fn calculate(
        width: u32,
        height: u32,
        padding: PaddingLayout,
        metrics: crate::text::font::CellMetrics,
    ) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            cell: SizeInPixels {
                width: metrics.width as usize,
                height: metrics.height as usize,
            },
            grid: Self::grid_size(width, height, padding, metrics),
        })
    }

    fn grid_size(
        width: u32,
        height: u32,
        padding: PaddingLayout,
        metrics: crate::text::font::CellMetrics,
    ) -> Size {
        Size {
            cols: (width.saturating_sub(padding.horizontal()) / metrics.width.max(1)).clamp(1, 4096)
                as usize,
            rows: (height.saturating_sub(padding.vertical()) / metrics.height.max(1)).clamp(1, 4096)
                as usize,
        }
    }

    fn messages(self, previous: Option<Self>) -> Vec<ClientToServerMsg> {
        let mut messages = Vec::new();
        if previous != Some(self) {
            messages.push(ClientToServerMsg::TerminalPixelDimensions {
                pixel_dimensions: PixelDimensions {
                    character_cell_size: Some(self.cell),
                    text_area_size: Some(SizeInPixels {
                        width: self.grid.cols * self.cell.width,
                        height: self.grid.rows * self.cell.height,
                    }),
                    text_area_grid_size: Some(self.grid),
                },
            });
        }
        if previous.is_none_or(|old| old.grid != self.grid) {
            messages.push(ClientToServerMsg::TerminalResize {
                new_size: self.grid,
            });
        }
        messages
    }
}

/// Maps a pointer position to a grid cell. `origin` is the grid's top-left
/// pixel ([`App::content_origin`]); positions in the padding clamp to the
/// nearest edge cell.
fn pointer_cell(
    pointer: (f64, f64),
    origin: (i32, i32),
    metrics: crate::text::font::CellMetrics,
    size: crate::emulation::screen_buffer::TermSize,
) -> (usize, usize) {
    let row = ((pointer.1 - f64::from(origin.1)).max(0.0) / metrics.height.max(1) as f64) as usize;
    let col = ((pointer.0 - f64::from(origin.0)).max(0.0) / metrics.width.max(1) as f64) as usize;
    (
        row.min(size.rows.saturating_sub(1)),
        col.min(size.cols.saturating_sub(1)),
    )
}

fn host_query_geometry(
    reported: Option<Geometry>,
    fallback: Size,
    metrics: crate::text::font::CellMetrics,
) -> session::Geometry {
    // A render frame can still carry the old grid while the server processes a
    // font/zoom/scale resize. Host replies must use the geometry just reported.
    let grid = reported.map_or(fallback, |geometry| geometry.grid);
    session::Geometry {
        rows: grid.rows,
        cols: grid.cols,
        cell_width: metrics.width as usize,
        cell_height: metrics.height as usize,
    }
}

fn window_host_reply(
    token: u32,
    query: &[u8],
    geometry: session::Geometry,
    paints: &Paints,
) -> ClientToServerMsg {
    // Host queries use the window's metrics and colors without reading its clipboard.
    ClientToServerMsg::ForwardedReplyFromHost {
        token,
        reply_bytes: crate::host_reply::answer(query, geometry, paints, None),
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
            exit_reason: ExitReason::Error(message),
        })) => Err(session::exit_error(message)),
        Some(Ok(message)) => Ok(Some(message)),
        Some(Err(mpsc::TryRecvError::Disconnected)) => {
            anyhow::bail!("the native session connection closed unexpectedly")
        },
        _ => Ok(None),
    }
}

impl App {
    fn new(
        args: TerminalArgs,
        opts: CliArgs,
        (config, options): (Config, Options),
        wake_reader: ReaderWake,
        clipboard: impl FnOnce() -> Clipboard,
    ) -> Result<Self> {
        let recorder = Recorder::new(args.record.as_deref());
        let active_fonts = effective_font_options(&config.terminal, 1.0, 1.0);
        let fonts = active_fonts
            .stack(1.0)
            .context("load native terminal fonts")?;
        let now = Instant::now();
        let paints = paints_for(&config, &options);
        let opening_size = graphical_opening_size(&args);
        let startup_mode = startup_mode(&args, &config.terminal);
        Ok(Self {
            surfaces: None,
            wake_reader,
            state: TerminalState::new(opening_size.rows, opening_size.cols),
            args,
            opts,
            settings: config.terminal,
            paints,
            cache: GlyphCache::new(fonts),
            active_fonts,
            handshake: Default::default(),
            endpoint: Default::default(),
            connection_geometry: ConnectionGeometry::new(opening_size, now),
            last_measured_geometry: None,
            retained: RetainedScene::new(),
            sender: None,
            messages: None,
            connection_id: 0,
            next_connection_id: 1,
            pending_switch: None,
            switch_notice: None,
            canceled_switch_escape: false,
            recorder,
            clipboard: Arc::new(Mutex::new(clipboard())),
            modifiers: ModifiersState::empty(),
            alt_keys: keyboard::AltKeys::default(),
            composition: Composition::new(),
            pointer: (0.0, 0.0),
            scroll: ScrollAccumulator::default(),
            buttons: (false, false, false),
            buttons_pressed_before_repaint: HashSet::new(),
            selection: LocalSelection::default(),
            mouse_mode: true,
            focused: true,
            pacer: Pacer::new(None),
            cursor_trail: CursorTrail::default(),
            trail_mesh: TrailMesh::default(),
            blink: BlinkPhase::On,
            next_blink: now + Duration::from_millis(500),
            scale: 1.0,
            zoom: 1.0,
            startup_mode,
            opening: None,
            recorded_window: None,
            started: now,
            error: None,
        })
    }

    fn open_connection(&mut self, connection: Option<session::Connection>) -> Result<()> {
        let connection = match connection {
            Some(connection) => connection,
            None => session::connect(&self.args, &self.opts, self.connection_geometry.seed)?,
        };
        self.endpoint = connection.endpoint;
        let (tx, rx) = mpsc::sync_channel(2);
        let sender = AttachedSender(connection.sender);
        let wake_reader = self.wake_reader.clone();
        let id = self.connection_id;
        std::thread::Builder::new()
            .name("native-session-reader".into())
            .spawn(move || {
                read_session(connection.receiver, tx, || wake_reader(id));
            })
            .context("spawn native session reader")?;
        self.sender = Some(sender);
        self.args.session_name = Some(connection.name);
        self.messages = Some(rx);
        Ok(())
    }
    fn send(&self, message: ClientToServerMsg) -> Result<()> {
        if let Some(sender) = &self.sender {
            session::send(&sender.0, message)?;
        }
        Ok(())
    }
    fn action(&self, action: Action) -> Result<()> {
        if let Some(sender) = &self.sender {
            session::action(&sender.0, action)?;
        }
        Ok(())
    }
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let metrics = self.cache.metrics();
        let startup = self.startup_mode;
        let remembered = remembered_size(startup, &window_state_path());
        let target = StartupSize::resolve(&self.args, &self.settings, remembered);
        #[cfg(target_os = "linux")]
        let backend = if winit::platform::x11::ActiveEventLoopExtX11::is_x11(event_loop) {
            WindowBackend::X11
        } else {
            WindowBackend::Wayland
        };
        #[cfg(not(target_os = "linux"))]
        let backend = WindowBackend::Mac;
        let physical_hint = backend == WindowBackend::X11 || target.has_pixel_axis();
        let hint_limit = if physical_hint {
            STARTUP_AXIS_LIMIT
        } else {
            4096
        };
        let hint = target.extent(
            metrics,
            self.padding_layout(),
            PhysicalSize::new(hint_limit, hint_limit),
        );
        let mut attrs = WindowAttributes::default()
            .with_title("Yinz")
            .with_transparent(true)
            .with_blur(self.settings.blur.unwrap_or(false));
        let hint = if physical_hint {
            WindowSize::Physical(hint)
        } else {
            WindowSize::Logical(LogicalSize::new(
                f64::from(hint.width),
                f64::from(hint.height),
            ))
        };
        attrs = attrs.with_inner_size(hint);
        if startup == StartupMode::Maximized {
            attrs = attrs.with_maximized(true);
        }
        if startup == StartupMode::Fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
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
        self.update_fonts(self.zoom, window.scale_factor());
        let extent = window.inner_size();
        if client_sized(startup) {
            let mut correction = StartupCorrection {
                target,
                hint,
                wayland_ceiling: None,
            };
            if backend == WindowBackend::Wayland {
                correction.observe_wayland_grant(extent, window.scale_factor());
            }
            self.opening = Some(OpeningGeometry::new(backend, correction, Instant::now()));
            self.last_measured_geometry = self.measure_geometry(extent)?;
        }
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
        let width =
            NonZeroU32::new(extent.width).context("zero native window width at creation")?;
        let height =
            NonZeroU32::new(extent.height).context("zero native window height at creation")?;
        let attrs =
            SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_handle, width, height);
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
        self.refresh_geometry()?;
        for message in crate::emulation::palette::seed_messages(&self.paints) {
            self.send(message)?;
        }
        Ok(())
    }
    fn change_scale(&mut self, zoom: f32, scale: f64) -> Result<()> {
        self.update_fonts(zoom, scale);
        self.refresh_geometry()
    }
    fn update_fonts(&mut self, zoom: f32, scale: f64) {
        let requested = effective_font_options(&self.settings, zoom, scale);
        let replacement = match prepare_fonts(&self.active_fonts, &requested) {
            Ok(replacement) => replacement,
            Err(error) => {
                log::error!("retaining working native fonts: {error:#}");
                return;
            },
        };
        let previous_metrics = self.cache.metrics();
        apply_fonts(
            &mut self.active_fonts,
            &mut self.cache,
            &mut self.retained,
            requested,
            replacement,
        );
        if self.cache.metrics() != previous_metrics {
            self.cursor_trail.reset();
            if self.connection_geometry.received_frame {
                self.observe_cursor_trail(Instant::now(), self.composition.shown().as_ref());
            }
        }
        self.zoom = zoom;
        self.scale = scale;
    }
    fn settle_opening(&mut self, now: Instant) -> Result<()> {
        let (Some(opening), Some(surfaces)) = (&self.opening, &self.surfaces) else {
            return Ok(());
        };
        let window = &surfaces.window;
        let size = window.inner_size();
        let scale = window.scale_factor();
        let monitor = window.current_monitor();
        if size.width == 0 || size.height == 0 {
            // A hidden/minimized window can lack a usable new extent. The last
            // validated sample is sufficient for the bounded opening fallback.
            if now >= opening.deadline() && self.last_measured_geometry.is_some() {
                self.opening = None;
            }
            return Ok(());
        }
        // Read the actual window after the event batch, including X11's monitor
        // update and any backend-generated resize. Event payloads can be stale.
        let scale_changed = self.scale != scale;
        if scale_changed {
            self.update_fonts(self.zoom, scale);
        }
        let padding = self.padding_layout();
        let metrics = self.cache.metrics();
        self.measure_geometry(size)?;
        let Some(opening) = &mut self.opening else {
            return Ok(());
        };
        let Some(surfaces) = &self.surfaces else {
            return Ok(());
        };
        let window = &surfaces.window;
        let placement_ready = opening.placement_ready(now, monitor.is_some());
        if let OpeningPhase::Placement { correction, .. } = &mut opening.phase {
            if opening.backend == WindowBackend::Wayland {
                correction.observe_wayland_grant(size, scale);
            }
            if !placement_ready {
                return Ok(());
            }
            let monitor = monitor
                .map(|monitor| monitor.size())
                .unwrap_or(PhysicalSize::new(STARTUP_AXIS_LIMIT, STARTUP_AXIS_LIMIT));
            if let Some((request, expected)) =
                correction.request(size, monitor, scale, padding, metrics)
            {
                opening.phase = OpeningPhase::Grant {
                    expected,
                    deadline: now + OPENING_GRANT_LIMIT,
                };
                let _ = window.request_inner_size(request);
                // Some backends grant synchronously. Use the observed extent
                // even when they do not send a separate resize callback.
                let granted = window.inner_size();
                // Even a synchronous size can be followed by another backend
                // configure. Keep the guard through the next event-loop batch.
                return self.resize_to(granted);
            }
        } else if !opening.grant_settled(now, size) {
            return Ok(());
        }
        self.resize_to(size)?;
        self.opening = None;
        Ok(())
    }

    fn record_window_observation(&mut self) {
        if self.args.record.is_none()
            || self.opening.is_some()
            || !self.connection_geometry.received_frame
        {
            return;
        }
        let Some(surfaces) = &self.surfaces else {
            return;
        };
        let window = &surfaces.window;
        let extent = window.inner_size();
        let metrics = self.cache.metrics();
        let padding = self.padding_layout();
        let Some(geometry) = Geometry::calculate(extent.width, extent.height, padding, metrics)
        else {
            return;
        };
        let origin = self.content_origin((extent.width, extent.height));
        let Ok(handle) = window.window_handle() else {
            return;
        };
        use winit::raw_window_handle::RawWindowHandle;
        let (backend, x11_id) = match handle.as_raw() {
            RawWindowHandle::Wayland(_) => ("wayland", None),
            RawWindowHandle::Xlib(handle) => ("x11", Some(format!("{:#x}", handle.window))),
            RawWindowHandle::Xcb(handle) => ("x11", Some(format!("{:#x}", handle.window.get()))),
            RawWindowHandle::AppKit(_) => ("macos", None),
            _ => return,
        };
        let observation = serde_json::json!({
            "backend": backend,
            "x11_id": x11_id,
            "output": window.current_monitor().is_some(),
            "scale": window.scale_factor(),
            "width": extent.width,
            "height": extent.height,
            "cell_width": metrics.width,
            "cell_height": metrics.height,
            "padding_left": padding.margins.left,
            "padding_top": padding.margins.top,
            "padding_right": padding.margins.right,
            "padding_bottom": padding.margins.bottom,
            "padding_balance": padding.balance,
            "origin_x": origin.0,
            "origin_y": origin.1,
            "cols": geometry.grid.cols,
            "rows": geometry.grid.rows,
            // Winit's Wayland getters read the compositor's last configure.
            // The fixture queries the applied EWMH properties separately on X11.
            "maximized": backend == "wayland" && window.is_maximized(),
            "fullscreen": backend == "wayland" && window.fullscreen().is_some(),
        });
        if self.recorded_window.as_ref() != Some(&observation) {
            log::info!("native window observation {observation}");
            self.recorded_window = Some(observation);
        }
    }

    fn resize(&mut self) -> Result<()> {
        if let Some(surfaces) = &self.surfaces {
            self.resize_to(surfaces.window.inner_size())?;
        }
        Ok(())
    }

    fn resize_to(&mut self, size: PhysicalSize<u32>) -> Result<()> {
        if let (Some(surfaces), Some(width), Some(height)) = (
            &self.surfaces,
            NonZeroU32::new(size.width),
            NonZeroU32::new(size.height),
        ) && let Some(geometry) = self.measure_geometry(size)?
        {
            surfaces.surface.resize(&surfaces.context, width, height);
            self.publish_geometry(geometry)?;
        }
        Ok(())
    }

    fn measure_geometry(&self, size: PhysicalSize<u32>) -> Result<Option<Geometry>> {
        let geometry = Geometry::calculate(
            size.width,
            size.height,
            self.padding_layout(),
            self.cache.metrics(),
        );
        if let (Some(opening), Some(geometry)) = (&self.opening, geometry) {
            opening.validate(size, geometry)?;
        }
        Ok(geometry)
    }

    fn refresh_geometry(&mut self) -> Result<()> {
        let Some(geometry) = self.live_geometry()? else {
            return Ok(());
        };
        self.publish_geometry(geometry)
    }

    fn publish_geometry(&mut self, geometry: Geometry) -> Result<()> {
        self.last_measured_geometry = Some(geometry);
        self.state
            .set_cell_size(geometry.cell.width as u32, geometry.cell.height as u32);
        for message in self.connection_geometry.messages(geometry) {
            if let ClientToServerMsg::TerminalResize { new_size } = &message {
                self.selection
                    .resizing(&self.state, new_size.rows, new_size.cols);
            }
            self.send(message)?;
        }
        if self.connection_geometry.received_frame {
            self.connection_geometry.reported = Some(geometry);
        }
        self.pacer.schedule();
        Ok(())
    }
    fn live_geometry(&self) -> Result<Option<Geometry>> {
        match &self.surfaces {
            Some(surfaces) => self.measure_geometry(surfaces.window.inner_size()),
            None => Ok(None),
        }
    }
    fn padding_layout(&self) -> PaddingLayout {
        padding_layout(&self.settings, self.scale)
    }
    /// The grid's top-left pixel in a `window`-sized drawable. Drawing, the
    /// IME area, the window observation and pointer conversion all use this,
    /// so they cannot disagree. It derives the grid the same way
    /// `measure_geometry` does, so it follows reload, scale and resize on the
    /// next call.
    fn content_origin(&self, window: (u32, u32)) -> (i32, i32) {
        let layout = self.padding_layout();
        let metrics = self.cache.metrics();
        match Geometry::calculate(window.0, window.1, layout, metrics) {
            Some(geometry) => layout.origin(window.0, window.1, geometry.grid, metrics),
            None => (layout.margins.left as i32, layout.margins.top as i32),
        }
    }
    fn cell_at(&self, window: (u32, u32)) -> (usize, usize) {
        pointer_cell(
            self.pointer,
            self.content_origin(window),
            self.cache.metrics(),
            self.state.size(),
        )
    }
    fn cell(&self) -> (usize, usize) {
        // Without a window there are no pointer events to convert.
        let window = self.surfaces.as_ref().map_or((0, 0), Surfaces::size);
        self.cell_at(window)
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
        let reload =
            match PreparedReload::load(&self.opts, &self.active_fonts, self.zoom, self.scale) {
                Ok(reload) => reload,
                Err(error) => {
                    log::error!("retaining working native configuration: {error:#}");
                    return Ok(());
                },
            };
        let previous_metrics = self.cache.metrics();
        let had_trail = self.cursor_trail.is_active();
        reload.apply(
            &mut self.active_fonts,
            &mut self.cache,
            &mut self.retained,
            &mut self.settings,
            &mut self.paints,
        );
        if self.cache.metrics() != previous_metrics || !self.cursor_trail_enabled() {
            self.cursor_trail.reset();
            if had_trail {
                self.pacer.schedule();
            }
        }
        if self.connection_geometry.received_frame && self.cursor_trail_enabled() {
            self.observe_cursor_trail(Instant::now(), self.composition.shown().as_ref());
        }
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
        self.refresh_geometry()
    }
    fn handle_message(&mut self, message: ServerToClientMsg, exit: impl FnOnce()) -> Result<()> {
        if self.handshake.control(
            &message,
            &self.endpoint,
            self.args.session_name.as_deref().unwrap_or("session"),
            &self.sender.as_ref().context("missing active sender")?.0,
            self.connection_geometry.first_frame_deadline,
        )? {
            return Ok(());
        }
        match message {
            ServerToClientMsg::RenderFrame { frame } => {
                if !self.connection_geometry.received_frame {
                    self.handshake.begin_frames(&mut self.state)?;
                }
                let applied = self
                    .selection
                    .apply_frame(&mut self.state, &frame)
                    .context("apply native render frame")?;
                self.recorder.frame(&frame)?;
                self.send(ClientToServerMsg::RenderFrameAck { seq: applied.seq })?;
                let first = !self.connection_geometry.received_frame;
                self.connection_geometry.received_frame = true;
                if first {
                    self.cursor_trail.reset();
                    self.refresh_geometry()?;
                }
                self.present_frame(applied);
            },
            ServerToClientMsg::Exit { exit_reason } => {
                let name = self.args.session_name.as_deref().unwrap_or("session");
                self.recorder
                    .finish(&self.endpoint.attachment_end(name, &exit_reason))?;
                exit();
            },
            ServerToClientMsg::SwitchSession { connect_to_session } => {
                self.begin_switch(connect_to_session, PendingSwitch::stage);
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
                let reply = window_host_reply(
                    token,
                    &query_bytes,
                    host_query_geometry(
                        self.connection_geometry.reported,
                        Size {
                            rows: self.state.size().rows,
                            cols: self.state.size().cols,
                        },
                        metrics,
                    ),
                    &self.paints,
                );
                self.send(reply)?;
            },
            ServerToClientMsg::QueryTerminalSize => {
                self.connection_geometry.reported = None;
                self.refresh_geometry()?;
            },
            // An initial ANSI repaint may already be queued before capability negotiation.
            ServerToClientMsg::Render { .. } => {},
            _ => {},
        }
        Ok(())
    }

    fn cursor_trail_enabled(&self) -> bool {
        self.settings.cursor_trail.unwrap_or(true)
    }

    fn configured_cursor_shape(&self) -> Option<CursorShape> {
        self.settings.cursor_style.map(|style| match style {
            yinz_types::input::native_terminal::CursorStyle::Block => CursorShape::Block,
            yinz_types::input::native_terminal::CursorStyle::Bar => CursorShape::Beam,
            yinz_types::input::native_terminal::CursorStyle::Underline => CursorShape::Underline,
        })
    }

    fn cursor_options(&self) -> CursorOptions {
        CursorOptions {
            shape: self.configured_cursor_shape(),
            blink: self.settings.cursor_blink,
            hollow: !self.focused,
        }
    }

    fn observe_cursor_trail(&mut self, now: Instant, preedit: Option<&Preedit>) -> bool {
        if !self.cursor_trail_enabled() {
            self.cursor_trail.reset();
            return false;
        }
        let selection = self.selection.range();
        let context = RowContext::new(
            SceneInputs {
                state: &self.state,
                phase: self.blink,
                paints: &self.paints,
                cursor: self.cursor_options(),
                hovered_link: None,
                preedit,
            },
            &self.cache,
        )
        .with_contrast(self.retained.contrast())
        .with_selection(selection.as_ref());
        let (snapshot, draw_cursor) = context.cursor_trail_state();
        self.cursor_trail
            .observe(snapshot, context.size, context.metrics, true, now);
        draw_cursor
    }

    fn sample_cursor_trail(&mut self, now: Instant, preedit: Option<&Preedit>) -> bool {
        if self.connection_geometry.received_frame && self.observe_cursor_trail(now, preedit) {
            self.cursor_trail.sample(now, &mut self.trail_mesh)
        } else {
            self.trail_mesh.clear();
            false
        }
    }

    fn present_frame(&mut self, applied: FrameApplied) {
        self.retained.mark(&applied.damage);
        if applied.cleared {
            self.cursor_trail.reset();
        }
        self.observe_cursor_trail(Instant::now(), self.composition.shown().as_ref());
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
            surfaces
                .window
                .request_user_attention(Some(winit::window::UserAttentionType::Informational));
        }
        if let Some(text) = self.state.take_copied_text() {
            self.set_clipboard(text);
        }
        self.pacer.schedule();
    }

    fn begin_switch(
        &mut self,
        target: SessionConnection,
        stage: impl FnOnce(&Self, u64, SessionConnection, String) -> Result<PendingSwitch>,
    ) {
        if self.pending_switch.is_some() {
            return;
        }
        let id = self.next_connection_id;
        self.next_connection_id += 1;
        let destination = target
            .name
            .as_ref()
            .map_or_else(|| "a new session".to_owned(), |name| format!("{name}"));
        if matches!(self.endpoint, session::Endpoint::Forwarded(_)) {
            let error = match &target.name {
                Some(name) => anyhow!(
                    "Open a fresh forward to the destination on the same remote host, or use {}.",
                    self.endpoint.ansi_attach(name.as_str())
                ),
                None => anyhow!(
                    "Create a new session on the same remote host and open a fresh forward to it."
                ),
            };
            self.show_switch_notice(&destination, &error);
            return;
        }
        match stage(self, id, target, destination.clone()) {
            Ok(pending) => {
                self.switch_notice = None;
                self.pending_switch = Some(pending);
                self.pacer.schedule();
            },
            Err(error) => self.show_switch_notice(&destination, &error),
        }
    }

    fn switch_grid(&self, measured: Option<Geometry>) -> Size {
        self.connection_geometry
            .switch_grid(measured.or(self.last_measured_geometry))
    }

    fn show_switch_notice(&mut self, destination: &str, error: &anyhow::Error) {
        log::error!("could not switch native window to {destination}: {error:#}");
        self.switch_notice = Some(switch_notice(destination, error));
        self.pacer.schedule();
    }

    fn dismiss_switch_notice(&mut self) -> bool {
        let had_notice = self.switch_notice.take().is_some();
        if had_notice {
            self.pacer.schedule();
        }
        had_notice
    }

    fn switch_failed(&mut self, error: &anyhow::Error) {
        if let Some(pending) = self.pending_switch.take() {
            self.show_switch_notice(&pending.destination, error);
        }
    }

    fn receive_switch(&mut self, id: u64, now: Instant) -> Result<()> {
        if !self
            .pending_switch
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            return Ok(());
        }
        let geometry = self.live_geometry()?;
        let size = self.switch_grid(geometry);
        let cell = self.cache.metrics();
        let pending = self
            .pending_switch
            .as_mut()
            .context("poll pending native switch")?;
        let was_opening = pending.opening.is_some();
        let first = match pending.poll(now, size, cell, &self.paints) {
            Ok(Some(first)) => first,
            Ok(None) => {
                if was_opening && pending.opening.is_none() {
                    self.pacer.schedule();
                }
                return Ok(());
            },
            Err(error) => {
                self.switch_failed(&error);
                return Ok(());
            },
        };
        let pending = self
            .pending_switch
            .take()
            .context("finish pending native switch")?;
        if let Err(error) = pending.report_ready(geometry, self.focused, first.applied.seq) {
            self.show_switch_notice(&pending.destination, &error);
            return Ok(());
        }
        self.recorder.frame(&first.frame)?;
        self.connection_id = pending.id;
        self.messages = Some(pending.messages);
        self.sender = pending.sender;
        self.handshake = pending.handshake;
        self.endpoint = session::Endpoint::Local;
        self.args.session_name = Some(pending.destination);
        self.selection.clear();
        self.composition = Composition::new();
        self.mouse_mode = pending.mouse_mode;
        self.switch_notice = None;
        self.connection_geometry = ConnectionGeometry::new(size, now);
        self.connection_geometry.reported = geometry;
        self.connection_geometry.received_frame = true;
        if let Some(geometry) = geometry {
            self.last_measured_geometry = Some(geometry);
        }
        self.state = first.state;
        self.retained = RetainedScene::new();
        self.cursor_trail.reset();
        if let Some(surfaces) = &mut self.surfaces {
            surfaces.renderer.clear_images();
            surfaces.window.set_title("Yinz");
        }
        self.present_frame(first.applied);
        if pending.reload {
            self.reload()?;
        }
        Ok(())
    }

    fn switch_overlay(&mut self, size: (u32, u32), origin: (i32, i32)) -> Option<RowScene> {
        let notice = if let Some(text) = self.switch_notice.as_deref() {
            Notice::Text(text)
        } else if let Some(pending) = &self.pending_switch {
            Notice::Pending {
                destination: &pending.destination,
                input_source: self.connection_geometry.received_frame.then_some(
                    self.args
                        .session_name
                        .as_deref()
                        .unwrap_or("current session"),
                ),
            }
        } else if !self.connection_geometry.received_frame {
            Notice::Text(
                "Waiting for the first repaint. Input will be available when the session appears.",
            )
        } else {
            return None;
        };
        let margins = self.padding_layout().margins;
        Some(notice_scene(notice, &mut self.cache, size, origin, margins))
    }

    fn draw(&mut self) -> Result<()> {
        let extent = self
            .surfaces
            .as_ref()
            .map(|surfaces| surfaces.window.inner_size());
        if let Some(size) = extent {
            self.measure_geometry(size)?;
        }
        let preedit = self.composition.shown();
        self.retained.set_selection(self.selection.range());
        self.retained
            .set_transparency(crate::render::scene::Transparency {
                opacity: self.settings.opacity.unwrap_or(1.0),
                mode: yinz_types::input::native_terminal::OpacityMode::Background,
            });
        let cursor = self.cursor_options();
        let size = self.surfaces.as_ref().map(Surfaces::size);
        let Some(size) = refresh_drawable(
            size,
            &mut self.retained,
            SceneInputs {
                state: &self.state,
                phase: self.blink,
                paints: &self.paints,
                cursor,
                hovered_link: None,
                preedit: preedit.as_ref(),
            },
            &mut self.cache,
        ) else {
            return Ok(());
        };
        let origin = self.content_origin(size);
        let notice = self.switch_overlay(size, origin);
        let trail_active = self.sample_cursor_trail(Instant::now(), preedit.as_ref());
        if let Some(surfaces) = &mut self.surfaces {
            surfaces.renderer.draw_retained(
                &self.retained,
                self.cache.atlases(),
                size,
                Frame {
                    origin,
                    trail: trail_active.then_some(&self.trail_mesh),
                    notice: notice.as_ref(),
                    ..Default::default()
                },
            );
            let (row, col) = self.state.cursor_position();
            let metrics = self.cache.metrics();
            surfaces.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(
                    origin.0 + (col as u32 * metrics.width) as i32,
                    origin.1 + (row as u32 * metrics.height) as i32,
                ),
                winit::dpi::PhysicalSize::new(metrics.width, metrics.height),
            );
            surfaces.window.pre_present_notify();
            surfaces
                .surface
                .swap_buffers(&surfaces.context)
                .context("present native terminal frame")?;
            self.pacer.drawn(Instant::now());
            if trail_active {
                self.pacer.schedule();
            }
        }
        Ok(())
    }
    fn event(&mut self, event: WindowEvent, exit: impl FnOnce()) -> Result<()> {
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
                exit();
            },
            WindowEvent::Resized(_) => {
                if let Some(opening) = &mut self.opening {
                    opening.placement_signal(Instant::now());
                    if opening.backend == WindowBackend::Wayland
                        && let OpeningPhase::Placement { correction, .. } = &mut opening.phase
                        && let Some(surfaces) = &self.surfaces
                    {
                        correction.observe_wayland_grant(
                            surfaces.window.inner_size(),
                            surfaces.window.scale_factor(),
                        );
                    }
                }
                self.resize()?;
            },
            WindowEvent::Moved(_) => {
                if let Some(opening) = &mut self.opening {
                    opening.placement_signal(Instant::now());
                }
            },
            WindowEvent::ScaleFactorChanged {
                scale_factor,
                mut inner_size_writer,
            } => {
                if let Some(opening) = &mut self.opening {
                    opening.placement_signal(Instant::now());
                    if opening.backend == WindowBackend::X11
                        && let Some(surfaces) = &self.surfaces
                    {
                        let size = surfaces.window.inner_size();
                        if size.width > 0 && size.height > 0 {
                            inner_size_writer
                                .request_inner_size(size)
                                .context("keep the opening X11 extent through monitor placement")?;
                        }
                    }
                    self.update_fonts(self.zoom, scale_factor);
                } else {
                    self.change_scale(self.zoom, scale_factor)?;
                }
            },
            WindowEvent::RedrawRequested => self.draw()?,
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if self.connection_geometry.received_frame {
                    self.observe_cursor_trail(Instant::now(), self.composition.shown().as_ref());
                }
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
            WindowEvent::Ime(event) if self.connection_geometry.received_frame => {
                match self.composition.ime(event) {
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
                }
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
                self.key_pressed(&press, event.repeat)?;
            },
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x, position.y);
                if !self.connection_geometry.received_frame {
                    return Ok(());
                }
                if self.buttons.0 && (self.modifiers.shift_key() || !self.mouse_mode) {
                    let (row, col) = self.cell();
                    self.selection.extend(row, col);
                    self.pacer.schedule();
                } else if self.mouse_mode {
                    self.mouse(MouseEventType::Motion, (false, false, false, false))?;
                }
            },
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if !self.connection_geometry.received_frame && pressed {
                    self.buttons_pressed_before_repaint.insert(button);
                    return Ok(());
                }
                let pressed_before_repaint = self.buttons_pressed_before_repaint.remove(&button);
                if !self.connection_geometry.received_frame || (!pressed && pressed_before_repaint)
                {
                    return Ok(());
                }
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
                    && let Some(link) = {
                        let (row, col) = self.cell();
                        links::run_at(self.state.screen(), row, col)
                    }
                {
                    open_link(&link.uri);
                } else if button == MouseButton::Left
                    && (self.modifiers.shift_key() || !self.mouse_mode)
                {
                    if pressed {
                        let (row, col) = self.cell();
                        self.selection.start(row, col);
                    }
                    self.pacer.schedule();
                } else if button == MouseButton::Middle
                    && pressed
                    && self.settings.middle_click_paste.unwrap_or(true)
                {
                    self.paste()?;
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
            WindowEvent::MouseWheel { delta, .. } if self.connection_geometry.received_frame => {
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
    fn key_pressed(&mut self, press: &keyboard::Press, repeat: bool) -> Result<()> {
        let escape = press.logical == Key::Named(NamedKey::Escape);
        if escape {
            // A retry can begin while the key that canceled the last switch is still held.
            if repeat && self.canceled_switch_escape {
                return Ok(());
            }
            if !repeat {
                self.canceled_switch_escape = false;
                if press.modifiers.is_empty() && self.pending_switch.take().is_some() {
                    self.canceled_switch_escape = true;
                    self.pacer.schedule();
                    return Ok(());
                }
            }
            if self.dismiss_switch_notice() {
                return Ok(());
            }
        }
        let pane_ready = self.connection_geometry.received_frame;
        if press.logical == Key::Named(NamedKey::Insert) && press.modifiers.shift_key() {
            return self.paste();
        }
        if pane_ready && self.compose_key(press)? {
            return Ok(());
        }
        let shortcut = (press.modifiers.control_key() && press.modifiers.shift_key())
            || press.modifiers.super_key();
        if shortcut && let Key::Character(key) = &press.logical {
            match key.to_lowercase().as_str() {
                "c" => {
                    if let Some(text) = self.selection.text(self.state.screen()) {
                        self.set_clipboard(text);
                        return Ok(());
                    }
                },
                "v" => return self.paste(),
                "+" | "=" => {
                    self.change_scale((self.zoom * 1.1).min(4.0), self.scale)?;
                    return Ok(());
                },
                "-" | "_" => {
                    self.change_scale((self.zoom / 1.1).max(0.25), self.scale)?;
                    return Ok(());
                },
                "0" | ")" => {
                    self.change_scale(1.0, self.scale)?;
                    return Ok(());
                },
                _ => {},
            }
        }
        if !pane_ready {
            return Ok(());
        }
        if escape {
            self.selection.clear();
        }
        if let Some(message) = keyboard::key_message(press) {
            self.selection.clear();
            self.pacer.schedule();
            self.send(message)?;
        }
        Ok(())
    }
    fn type_text(&mut self, text: String) -> Result<()> {
        if let Some(message) = keyboard::typed_text_message(text) {
            self.selection.clear();
            self.pacer.schedule();
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
                        self.selection.clear();
                        self.send(message)?;
                    }
                },
                keyboard::Resolution::Type(text) => self.type_text(text)?,
                keyboard::Resolution::TypeThenKey(text) => {
                    self.type_text(text)?;
                    if let Some(message) = keyboard::key_message(press) {
                        self.selection.clear();
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
    fn paste(&mut self) -> Result<()> {
        if self.connection_geometry.received_frame
            && let Some(chars) = self.get_clipboard()
        {
            self.selection.clear();
            self.pacer.schedule();
            self.send(ClientToServerMsg::Paste(chars))?;
        }
        Ok(())
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

    fn duration_elapsed(&self, now: Instant) -> Result<bool> {
        let duration_elapsed = self.args.duration_secs.is_some_and(|seconds| {
            now.duration_since(self.started) >= Duration::from_secs(seconds)
        });
        if duration_elapsed {
            self.recorder.finish("Capture duration elapsed")?;
        }
        Ok(duration_elapsed)
    }
}
impl ApplicationHandler<u64> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surfaces.is_none()
            && let Err(error) = self.open(event_loop)
        {
            self.fail(error, event_loop);
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Err(error) = self.event(event, || event_loop.exit()) {
            self.fail(error, event_loop);
        }
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, id: u64) {
        if event_loop.exiting() {
            return;
        }
        if id != self.connection_id {
            if let Err(error) = self.receive_switch(id, Instant::now()) {
                self.fail(error, event_loop);
            }
            return;
        }
        // The reader posts one wake for each bounded-queue message. Handle one
        // per event so continuous output cannot grow a backlog of stale wakes
        // while an unbounded drain loop keeps the window from drawing.
        match receive_message(self.messages.as_ref()) {
            Ok(Some(message)) => {
                if let Err(error) = self.handle_message(message, || event_loop.exit()) {
                    self.fail(error, event_loop);
                }
            },
            Err(error) => self.fail(error, event_loop),
            Ok(None) => {},
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            return;
        }
        let now = Instant::now();
        if let Err(error) = self.settle_opening(now) {
            self.fail(error, event_loop);
            return;
        }
        self.record_window_observation();
        if !self.connection_geometry.received_frame
            && now >= self.connection_geometry.first_frame_deadline
        {
            self.fail(
                self.handshake.timeout(
                    &self.endpoint,
                    self.args.session_name.as_deref().unwrap_or("session"),
                ),
                event_loop,
            );
            return;
        }
        if let Some(pending) = &self.pending_switch
            && let Err(error) = pending.check_deadline(now)
        {
            self.switch_failed(&error);
        }
        match self.duration_elapsed(now) {
            Ok(true) => {
                event_loop.exit();
                return;
            },
            Err(error) => {
                self.fail(error, event_loop);
                return;
            },
            Ok(false) => {},
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
        if let Some(opening) = &self.opening {
            let deadline = opening.wake_at();
            if deadline > now {
                wake = wake.min(deadline);
            }
        }
        if !self.connection_geometry.received_frame {
            wake = wake.min(self.connection_geometry.first_frame_deadline);
        }
        if let Some(pending) = &self.pending_switch {
            wake = wake.min(pending.deadline);
        }
        match redraw_decision(&self.pacer, self.surfaces.as_ref().map(Surfaces::size), now) {
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

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.pending_switch.take();
        self.messages.take();
        self.sender.take();
    }
}
fn window_state_path() -> std::path::PathBuf {
    yinz_runtime::consts::YINZ_CACHE_DIR.join("native-terminal-window.json")
}
fn paints_for(config: &Config, options: &Options) -> Paints {
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

#[cfg(feature = "attachment-test-support")]
#[path = "unit/live_window.rs"]
pub(crate) mod live_window;
