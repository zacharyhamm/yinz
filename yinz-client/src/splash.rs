use crate::os_input_output::ClientOsApi;
use crate::stdin_ansi_parser::SyncOutput;
use crate::terminal_escapes::write_synchronised;
use std::time::{Duration, Instant};
use yinz_config::config::Config;
use yinz_runtime::errors::prelude::*;
use yinz_types::colors::PaletteColor;
use yinz_types::input::options::Options;
use yinz_types::pane_size::Size;
use yinz_types::style::default_palette;

mod geometry;
mod outlines;
pub(crate) mod renderer;

pub(crate) const CLEAR: &str = "\x1b[m\x1b[2J\x1b[H";
pub(crate) const DURATION: Duration = Duration::from_millis(1000);
pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// Owned by the client loop, which also owns all terminal writes after startup.
pub(crate) struct Splash {
    started: Instant,
    next_frame: Option<Instant>,
    renderer: renderer::Renderer,
}

impl Splash {
    pub fn deadline(&self) -> Instant {
        self.started + DURATION
    }

    pub fn waiting(&self, now: Instant) -> bool {
        now < self.deadline()
    }

    pub fn wake_at(&self) -> Instant {
        self.next_frame
            .unwrap_or(self.deadline())
            .min(self.deadline())
    }

    /// Paint at most one frame per wakeup, skipping frames missed during startup.
    /// Clear at the deadline even if the session has not rendered yet.
    pub fn tick(
        &mut self,
        os: &dyn ClientOsApi,
        sync: Option<SyncOutput>,
        now: Instant,
    ) -> Result<bool> {
        if self.next_frame.is_none() {
            return Ok(false);
        }
        if !self.waiting(now) {
            return self.clear(os, sync);
        }
        let Some(stage) = renderer::Stage::new(os.get_terminal_size()) else {
            return self.clear(os, sync);
        };
        let resized = stage != self.renderer.stage;
        if resized {
            self.renderer.resize(stage);
        }
        if resized || self.next_frame.is_some_and(|next| now >= next) {
            let output = self
                .renderer
                .frame(now.saturating_duration_since(self.started));
            write_synchronised(&mut *os.get_stdout_writer(), sync, output.as_bytes())
                .context("failed to draw the startup splash")?;
            self.next_frame = Some((now + FRAME_INTERVAL).min(self.deadline()));
        }
        Ok(true)
    }

    fn clear(&mut self, os: &dyn ClientOsApi, sync: Option<SyncOutput>) -> Result<bool> {
        write_synchronised(&mut *os.get_stdout_writer(), sync, CLEAR.as_bytes())
            .context("failed to clear the startup splash")?;
        self.next_frame = None;
        Ok(false)
    }
}

pub(crate) fn show(
    os_input: &dyn ClientOsApi,
    size: Size,
    config: &Config,
    options: &Options,
) -> Result<Option<Splash>> {
    if options.disable_splash_screen.unwrap_or(false) {
        return Ok(None);
    }
    let Some(mut renderer) = renderer(size, config, options) else {
        return Ok(None);
    };
    let started = Instant::now();
    let output = renderer.frame(Duration::ZERO);
    write_synchronised(&mut *os_input.get_stdout_writer(), None, output.as_bytes())
        .context("failed to draw the startup splash")?;
    Ok(Some(Splash {
        started,
        next_frame: Some(started + FRAME_INTERVAL),
        renderer,
    }))
}

pub(crate) fn renderer(
    size: Size,
    config: &Config,
    options: &Options,
) -> Option<renderer::Renderer> {
    let stage = renderer::Stage::new(size)?;
    Some(renderer_for_stage(stage, config, options))
}

pub(crate) fn renderer_for_stage(
    stage: renderer::Stage,
    config: &Config,
    options: &Options,
) -> renderer::Renderer {
    let theme = config
        .theme_config(options.theme.as_ref())
        .unwrap_or_else(|| default_palette().into());
    let mut style = theme.text_unselected;
    if let Some(terminal) = config.terminal_theme_config(options.theme.as_ref()) {
        style.base = PaletteColor::Rgb(terminal.foreground);
        style.background = PaletteColor::Rgb(terminal.background);
    }
    renderer::Renderer::new(stage, style, theme.frame_selected.base)
}

#[cfg(test)]
pub(crate) fn test_splash(started: Instant) -> Splash {
    let stage = renderer::Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let theme = yinz_types::style::Styling::from(default_palette());
    Splash {
        started,
        next_frame: Some(started + FRAME_INTERVAL),
        renderer: renderer::Renderer::new(stage, theme.text_unselected, theme.frame_selected.base),
    }
}

#[cfg(test)]
#[path = "unit/splash_tests.rs"]
mod tests;
