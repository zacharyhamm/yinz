use std::fmt::{Display, Error, Formatter};
use std::time::{Duration, Instant};

use yinz_runtime::errors::prelude::*;
use yinz_types::data::PaletteColor;
use yinz_types::style::Styling;

/// How long each of the loading indicator's dots stays up.
///
/// The dots are a coarse "still working" signal rather than motion, so they run far slower than
/// anything driven by a tween - they just take their frames from the same clock, which slows down
/// to this period when they are the only thing left animating. See
/// `Screen::animation_clock_period`.
pub const DOT_PERIOD: Duration = Duration::from_millis(500);
/// One more than the most dots shown, so the count cycles `0, 1, 2, 3`.
const DOT_STATES: u128 = 4;

#[derive(Debug, Clone, Default)]
pub struct LoadingIndication {
    pub ended: bool,
    error: Option<String>,
    plugin_name: String,
    terminal_emulator_colors: Option<Styling>,
    override_previous_error: bool,
    started_at: Option<Instant>,
}

impl LoadingIndication {
    pub fn new(plugin_name: String) -> Self {
        let started_at = Some(Instant::now());
        LoadingIndication {
            plugin_name,
            started_at,
            ..Default::default()
        }
    }
    pub fn set_name(&mut self, plugin_name: String) {
        self.plugin_name = plugin_name;
    }
    pub fn with_colors(mut self, terminal_emulator_colors: Styling) -> Self {
        self.terminal_emulator_colors = Some(terminal_emulator_colors);
        self
    }
    pub fn merge(&mut self, other: LoadingIndication) {
        // `started_at` is what the dots are derived from, so carrying it across is what stops them
        // restarting on every merge - exactly the job the preserved `animation_offset` used to do
        // back when the offset was counted rather than derived.
        //
        // The *earlier* of the two, not simply the one already here: a load began when the first
        // thing to say so said it, and an indication raised before the pane existed carries an
        // earlier stamp than the pane's own. Taking the later one would restart the dots and, more
        // visibly, restart the 400ms grace below - so an indication that had already earned its
        // way onto the screen would blink back off.
        let current_started_at = self.started_at;
        let current_terminal_emulator_colors = self.terminal_emulator_colors.take();
        let mut current_error = self.error.take();
        let override_previous_error = other.override_previous_error;
        drop(std::mem::replace(self, other));
        self.started_at = match (current_started_at, self.started_at) {
            (Some(current), Some(incoming)) => Some(current.min(incoming)),
            (current, incoming) => current.or(incoming),
        };
        self.terminal_emulator_colors = current_terminal_emulator_colors;
        if let Some(current_error) = current_error.take() {
            // we do this so that only the first error (usually the root cause) will be shown
            // when plugins support scrolling, we might want to do an append here
            if !override_previous_error {
                self.error = Some(current_error);
            }
        }
    }
    /// How many dots to draw at `now`.
    ///
    /// Derived from `started_at` rather than counted per tick, so a dropped, late or doubled
    /// wakeup changes nothing about what is drawn - the same property that makes the toast
    /// animations frame-rate independent, for the same reason.
    pub fn animation_offset(&self, now: Instant) -> usize {
        self.started_at.map_or(0, |started_at| {
            (now.saturating_duration_since(started_at).as_millis() / DOT_PERIOD.as_millis()
                % DOT_STATES) as usize
        })
    }
    pub fn indicate_loading_error(&mut self, error_text: String) {
        self.error = Some(error_text);
    }
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
    pub fn override_previous_error(&mut self) {
        self.override_previous_error = true;
    }
}

macro_rules! style {
    ($fg:expr_2021) => {
        ansi_term::Style::new().fg(match $fg {
            PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
            PaletteColor::EightBit(color) => ansi_term::Color::Fixed(color),
        })
    };
}

impl Display for LoadingIndication {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        let cyan = match self.terminal_emulator_colors {
            Some(terminal_emulator_colors) => {
                style!(terminal_emulator_colors.exit_code_success.emphasis_0).bold()
            },
            None => ansi_term::Style::new(),
        };
        let red = match self.terminal_emulator_colors {
            Some(terminal_emulator_colors) => {
                style!(terminal_emulator_colors.exit_code_error.base).bold()
            },
            None => ansi_term::Style::new(),
        };
        let plugin_name = &self.plugin_name;
        let add_dots = |stringified: &mut String| {
            for _ in 0..self.animation_offset(Instant::now()) {
                stringified.push('.');
            }
            stringified.push(' ');
        };
        let mut stringified = String::new();

        if let Some(error_text) = &self.error {
            stringified.push_str(&format!(
                "\n\r{} {}",
                red.bold().paint("ERROR: "),
                error_text.replace('\n', "\n\r")
            ));
            // we add this additional line explicitly to make it easier to realize when something
            // is wrong in very small plugins (eg. the tab-bar and status-bar)
            stringified.push_str(&format!(
                "\n\r{}",
                red.bold()
                    .paint("ERROR IN PLUGIN - check logs for more info")
            ));
        } else {
            let loading_text = "Loading";
            stringified.push_str(&format!("{} {}", loading_text, cyan.paint(plugin_name)));
            add_dots(&mut stringified);
        }
        if self
            .started_at
            .map(|s| s.elapsed() > std::time::Duration::from_millis(400))
            .unwrap_or(true)
            || self.error.is_some()
        {
            write!(f, "{}", stringified)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "unit/loading_indication.rs"]
mod tests;
