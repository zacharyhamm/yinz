use crate::os_input_output::ClientOsApi;
use crate::terminal_escapes::{CLEAR_SCREEN, RESET_STYLE};
use std::io::Write;
use std::time::{Duration, Instant};
use yinz_config::config::Config;
use yinz_runtime::errors::prelude::*;
use yinz_types::colors::PaletteColor;
use yinz_types::input::options::Options;
use yinz_types::pane_size::Size;
use yinz_types::style::{default_palette, StyleDeclaration};

const ART: [&str; 11] = [
    "           ░██░                      ",
    "                                     ",
    "░█░▓  ▒░░▒ █▓▓█ ▄░░░▒▓▒░█▄ ▄░░░▒▓▒░█▄",
    "░▒▒▓  ▓▒▒░ ░▒▒░ ░▒▒▓▀▀▓▒▒░ ░▒▒▓▀▀▓▒▒░",
    "▒▓▓▒  ▒▓▓▒ ▓▓▓▒ ▒▓▓▒  ▒▓▓▒    ▄▄▄▓▓▓▀",
    "█▓▓▒  ▒▓▓█ ▓▓▓▓ █▓▓▒  ▒▓▓█ ▄▄▓▓▓▀▀▀▀ ",
    "▒▓▓▒  ░▓▓█ ▒▓▓█ █▓▓░  ▒▓▓▒ ▒▓▓░▀ ▄▄▄▄",
    "▀▓▓▓▓▓▓▒▒▓ ░▓▓▓ ▓▒▒░  ▒▓▓▒ ▀▒▒▓▓▓▓▓▓▓",
    " ▀▀▀▀▀░▒▒▓ ▀▀▀▀ ▀▀▀▀  ▀▀▀▀  ▀▀▀▀▀▀▀▀▀",
    "▒░░░░░░░▒▀                           ",
    "▀▀▀▀▀▀▀▀",
];

pub(crate) const CLEAR: &str = "\x1b[m\x1b[2J\x1b[H";

pub(crate) fn show(
    os_input: &dyn ClientOsApi,
    size: Size,
    config: &Config,
    options: &Options,
) -> Result<Option<Instant>> {
    let style = config
        .theme_config(options.theme.as_ref())
        .unwrap_or_else(|| default_palette().into())
        .text_unselected;
    let Some(output) = render(size, style) else {
        return Ok(None);
    };
    let mut stdout = os_input.get_stdout_writer();
    stdout
        .write_all(output.as_bytes())
        .context("failed to draw the startup splash")?;
    stdout
        .flush()
        .context("failed to flush the startup splash")?;
    Ok(Some(Instant::now() + Duration::from_secs(1)))
}

fn render(size: Size, style: StyleDeclaration) -> Option<String> {
    // Every character in this fixed artwork is a single terminal cell.
    let width = ART.iter().map(|row| row.chars().count()).max()?;
    if size.cols < width + 2 || size.rows < ART.len() + 2 {
        return None;
    }
    let top = (size.rows - ART.len()) / 2 + 1;
    let left = (size.cols - width) / 2 + 1;
    let colors = [
        style.emphasis_0,
        style.emphasis_1,
        style.emphasis_2,
        style.emphasis_3,
    ];
    let mut output = format!(
        "{RESET_STYLE}\x1b[48;{}m{CLEAR_SCREEN}",
        color(style.background)
    );
    let mut band = 0;
    for (row, text) in ART.iter().enumerate() {
        if !text.trim().is_empty() {
            output.push_str(&format!("\x1b[38;{}m", color(colors[band % colors.len()])));
            band += 1;
        }
        output.push_str(&format!("\x1b[{};{left}H{text}", top + row));
    }
    output.push_str(RESET_STYLE);
    Some(output)
}

fn color(color: PaletteColor) -> String {
    match color {
        PaletteColor::Rgb((r, g, b)) => format!("2;{r};{g};{b}"),
        PaletteColor::EightBit(index) => format!("5;{index}"),
    }
}

#[cfg(test)]
#[path = "unit/splash_tests.rs"]
mod tests;
