//! Per-client idle state shared with the input thread; terminal writes stay in the client loop.
use crate::os_input_output::ClientOsApi;
use crate::splash::{
    self,
    renderer::{Direction, Renderer, Stage},
};
use crate::stdin_ansi_parser::SyncOutput;
use crate::terminal_escapes::write_synchronised;
use crate::InputInstruction;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_config::config::Config;
use yinz_key_codec::vendored::termwiz::input::InputEvent;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::options::Options;
use yinz_types::pane_size::Size;

const LOOP_PAUSE: Duration = Duration::from_secs(3);
const ORIGINAL_LOOPS: u32 = 3;
const BOUNCE_DURATION: Duration = Duration::from_secs(60);

pub(crate) struct Activity {
    last_input: Instant,
    sleeping: bool,
}
pub(crate) type SharedActivity = Arc<Mutex<Activity>>;

/// Returns true only for the event that wakes the client. Protocol replies never count as input.
pub(crate) fn record_input(
    activity: &SharedActivity,
    instruction: &InputInstruction,
    now: Instant,
) -> Result<bool> {
    if !matches!(
        instruction,
        InputInstruction::KeyWithModifierEvent(..)
            | InputInstruction::KeyEvent(
                InputEvent::Key(_) | InputEvent::Mouse(_) | InputEvent::Paste(_),
                _
            )
    ) {
        return Ok(false);
    }
    let mut state = activity
        .lock()
        .to_anyhow()
        .context("record client activity")?;
    state.last_input = now;
    Ok(std::mem::take(&mut state.sleeping))
}

pub(crate) struct Screensaver {
    pub activity: SharedActivity,
    timeout: Duration,
    config: Config,
    options: Options,
    renderer: Option<Renderer>,
    started: Instant,
    next_frame: Instant,
    restoring: bool,
    last_size: Option<Size>,
    cycle: Option<u128>,
    direction: Option<Direction>,
    client_identity: String,
    server_identity: String,
}

impl Screensaver {
    pub fn new(config: &Config, options: &Options) -> Option<Self> {
        if !options.screensaver.unwrap_or(true) {
            return None;
        }
        let now = Instant::now();
        Some(Self {
            activity: Arc::new(Mutex::new(Activity {
                last_input: now,
                sleeping: false,
            })),
            timeout: Duration::from_secs(u64::from(
                options.screensaver_idle_timeout.unwrap_or(60).max(1),
            )),
            config: config.clone(),
            options: options.clone(),
            renderer: None,
            started: now,
            next_frame: now,
            restoring: false,
            last_size: None,
            cycle: None,
            direction: None,
            client_identity: identity_line(
                "Client",
                yinz_runtime::consts::VERSION,
                &yinz_runtime::host_identity::host_display_name(),
            ),
            server_identity: identity_line("Server", "", ""),
        })
    }

    pub fn set_server_identity(&mut self, version: &str, host: &str) {
        self.server_identity = identity_line("Server", version, host);
        self.next_frame = Instant::now();
    }

    pub fn active(&self) -> bool {
        self.renderer.is_some()
    }

    pub fn repaint_complete(&mut self) {
        self.restoring = false;
    }

    /// Returns true when the session must be repainted after dismissal.
    pub fn tick(
        &mut self,
        os: &dyn ClientOsApi,
        sync: Option<SyncOutput>,
        now: Instant,
        eligible: bool,
    ) -> Result<bool> {
        let mut state = self
            .activity
            .lock()
            .to_anyhow()
            .context("check client idle state")?;
        if !eligible {
            state.last_input = now;
            state.sleeping = false;
            self.renderer = None;
            self.cycle = None;
            self.restoring = false;
            return Ok(false);
        }
        let size = os.get_terminal_size();
        let stage = Stage::screensaver(size);
        if self.active() && (!state.sleeping || stage.is_none()) {
            state.sleeping = false;
            state.last_input = now;
            self.renderer = None;
            self.cycle = None;
            self.restoring = true;
            return Ok(true);
        }
        if self.restoring || stage.is_none() {
            state.last_input = now;
            return Ok(false);
        }
        let mut activated = false;
        if self.renderer.is_none()
            && now.saturating_duration_since(state.last_input) >= self.timeout
        {
            self.renderer = stage
                .map(|(stage, _)| splash::renderer_for_stage(stage, &self.config, &self.options));
            state.sleeping = self.renderer.is_some();
            self.started = now;
            self.next_frame = now;
            activated = state.sleeping;
        }
        drop(state);
        if activated {
            // Delete placements without deleting image data; wake requests a full graphics replay.
            write_synchronised(
                &mut *os.get_stdout_writer(),
                sync,
                b"\x1b_Ga=d,d=a,q=2\x1b\\",
            )
            .context("hide graphics for screensaver")?;
        }
        if let (Some(renderer), Some((stage, footer_rows))) = (&mut self.renderer, stage) {
            let resized = self.last_size != Some(size);
            if resized || now >= self.next_frame {
                let elapsed = now.saturating_duration_since(self.started);
                let (cycle, phase) = animation_phase(elapsed);
                let direction = match (self.cycle, self.direction) {
                    (Some(active), Some(direction)) if active == cycle => direction,
                    _ => {
                        // Select only for the observed cycle, including after missed cycles.
                        let direction = Direction::random_after(self.direction);
                        self.direction = Some(direction);
                        self.cycle = Some(cycle);
                        direction
                    },
                };
                let mut output = match phase {
                    Phase::Original(time) => {
                        if renderer.stage != stage {
                            renderer.resize(stage);
                        }
                        renderer.frame(time.min(splash::DURATION))
                    },
                    Phase::Bouncing(time) => {
                        renderer.bouncing_frame(size, time, direction, footer_rows)
                    },
                };
                renderer.append_screensaver_footer(
                    &mut output,
                    size,
                    footer_rows,
                    &self.client_identity,
                    &self.server_identity,
                );
                write_synchronised(
                    &mut *os.get_stdout_writer(),
                    sync,
                    format!("\x1b[?25l{output}").as_bytes(),
                )
                .context("draw idle screensaver")?;
                self.last_size = Some(size);
                let delay = match phase {
                    Phase::Original(time) if time < splash::DURATION => {
                        splash::FRAME_INTERVAL.min(splash::DURATION - time)
                    },
                    Phase::Original(time) => splash::DURATION + LOOP_PAUSE - time,
                    Phase::Bouncing(time) => splash::FRAME_INTERVAL.min(BOUNCE_DURATION - time),
                };
                self.next_frame = now + delay;
            }
        }
        Ok(false)
    }

    pub fn wake_at(&self, now: Instant) -> Result<Option<Instant>> {
        if self.active() {
            return Ok(Some(self.next_frame));
        }
        if self.restoring {
            return Ok(None);
        }
        let state = self
            .activity
            .lock()
            .to_anyhow()
            .context("schedule idle screensaver")?;
        Ok(Some(
            state
                .last_input
                .checked_add(self.timeout)
                .unwrap_or(now + Duration::from_secs(60)),
        ))
    }
}

fn identity_line(role: &str, version: &str, host: &str) -> String {
    let component = |value: &str| {
        let value: String = value.chars().filter(|c| !c.is_control()).collect();
        if value.trim().is_empty() {
            "unknown".to_owned()
        } else {
            value
        }
    };
    format!("{role}: {} on {}", component(version), component(host))
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Original(Duration),
    Bouncing(Duration),
}

fn animation_phase(elapsed: Duration) -> (u128, Phase) {
    let original_loop = splash::DURATION + LOOP_PAUSE;
    let original_duration = original_loop * ORIGINAL_LOOPS;
    let cycle = original_duration + BOUNCE_DURATION;
    let time = Duration::from_nanos((elapsed.as_nanos() % cycle.as_nanos()) as u64);
    let phase = if time < original_duration {
        Phase::Original(Duration::from_nanos(
            (time.as_nanos() % original_loop.as_nanos()) as u64,
        ))
    } else {
        Phase::Bouncing(time - original_duration)
    };
    (elapsed.as_nanos() / cycle.as_nanos(), phase)
}

#[cfg(test)]
#[path = "unit/screensaver_tests.rs"]
mod tests;
