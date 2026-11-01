//! Per-client idle state shared with the input thread; terminal writes stay in the client loop.
use crate::os_input_output::ClientOsApi;
use crate::splash::{
    self,
    renderer::{Renderer, Stage},
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

const FRAME_INTERVAL: Duration = Duration::from_millis(200);

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
        })
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
            self.restoring = false;
            return Ok(false);
        }
        let stage = Stage::new(os.get_terminal_size());
        if self.active() && (!state.sleeping || stage.is_none()) {
            state.sleeping = false;
            state.last_input = now;
            self.renderer = None;
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
            self.renderer = splash::renderer(os.get_terminal_size(), &self.config, &self.options);
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
        if let (Some(renderer), Some(stage)) = (&mut self.renderer, stage) {
            let resized = stage != renderer.stage;
            if resized {
                renderer.resize(stage);
            }
            if resized || now >= self.next_frame {
                let output =
                    renderer.frame(animation_time(now.saturating_duration_since(self.started)));
                write_synchronised(
                    &mut *os.get_stdout_writer(),
                    sync,
                    format!("\x1b[?25l{output}").as_bytes(),
                )
                .context("draw idle screensaver")?;
                self.next_frame = now + FRAME_INTERVAL;
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

fn animation_time(elapsed: Duration) -> Duration {
    Duration::from_secs_f64((elapsed.as_secs_f64() / 4.0) % 1.0)
}

#[cfg(test)]
#[path = "unit/screensaver_tests.rs"]
mod tests;
