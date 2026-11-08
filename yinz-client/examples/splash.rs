//! Preview the startup animation: cargo run -p yinz-client --example splash
//! Add --light for Ayu Light instead of Dracula, or --frames to emit frames without pacing.
#[path = "../src/splash/geometry.rs"]
mod geometry;
#[path = "../src/splash/outlines.rs"]
mod outlines;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "shared renderer includes a screensaver-only footer helper"
    )
)]
#[path = "../src/splash/renderer.rs"]
mod renderer;

use crossterm::SynchronizedUpdate;
use std::io::Write;
use std::time::{Duration, Instant};
use yinz_config::assets::get_default_themes;
use yinz_runtime::errors::prelude::*;
use yinz_types::colors::PaletteColor;
use yinz_types::pane_size::Size;

fn main() -> Result<()> {
    let frames = std::env::args().any(|arg| arg == "--frames");
    let light = std::env::args().any(|arg| arg == "--light");
    let themes = get_default_themes();
    let name = if light { "ayu-light" } else { "dracula" };
    let theme = themes.get_theme(name).context("preview theme")?;
    let mut style = theme.palette.text_unselected;
    if let Some(terminal) = theme.terminal {
        style.base = PaletteColor::Rgb(terminal.foreground);
        style.background = PaletteColor::Rgb(terminal.background);
    }
    let (cols, rows) = if frames {
        (80, 24)
    } else {
        crossterm::terminal::size().context("terminal size")?
    };
    let stage = renderer::Stage::new(Size {
        rows: usize::from(rows),
        cols: usize::from(cols),
    })
    .context("preview needs at least 28 columns and 10 rows")?;
    let mut renderer = renderer::Renderer::new(stage, style, theme.palette.frame_selected.base);
    let started = Instant::now();
    let mut stdout = std::io::stdout().lock();
    let duration = Duration::from_millis(1000);
    for frame in 0..=20 {
        let mut elapsed = Duration::from_millis(frame * 50);
        if !frames {
            std::thread::sleep((started + elapsed).saturating_duration_since(Instant::now()));
            elapsed = started.elapsed();
            let (cols, rows) = crossterm::terminal::size().context("terminal size")?;
            if let Some(stage) = renderer::Stage::new(Size {
                rows: usize::from(rows),
                cols: usize::from(cols),
            }) {
                renderer.resize(stage);
            }
        }
        let output = if elapsed >= duration {
            "\x1b[m\x1b[2J\x1b[H".to_owned()
        } else {
            renderer.frame(elapsed)
        };
        // Present the completed frame together, including its screen clear.
        stdout
            .sync_update(|stdout| stdout.write_all(output.as_bytes()))
            .context("synchronize preview frame")?
            .context("draw preview")?;
        if elapsed >= duration {
            break;
        }
    }
    Ok(())
}
