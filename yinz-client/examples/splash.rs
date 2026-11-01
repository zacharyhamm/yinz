//! Preview the startup animation: cargo run -p yinz-client --example splash
//! Add --light for a light palette, or --frames to emit frames without pacing.
#[path = "../src/splash/geometry.rs"]
mod geometry;
#[path = "../src/splash/outlines.rs"]
mod outlines;
#[path = "../src/splash/renderer.rs"]
mod renderer;

use crossterm::SynchronizedUpdate;
use std::io::Write;
use std::time::{Duration, Instant};
use yinz_runtime::errors::prelude::*;
use yinz_types::colors::PaletteColor;
use yinz_types::pane_size::Size;
use yinz_types::style::StyleDeclaration;

fn main() -> Result<()> {
    let frames = std::env::args().any(|arg| arg == "--frames");
    let light = std::env::args().any(|arg| arg == "--light");
    let style = StyleDeclaration {
        background: PaletteColor::Rgb(if light { (245, 245, 245) } else { (20, 22, 26) }),
        base: PaletteColor::Rgb(if light { (35, 40, 50) } else { (210, 221, 235) }),
        ..Default::default()
    };
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
    let mut renderer = renderer::Renderer::new(stage, style);
    let started = Instant::now();
    let mut stdout = std::io::stdout().lock();
    for frame in 0..=60 {
        let elapsed = Duration::from_millis(frame * 50);
        if !frames {
            std::thread::sleep((started + elapsed).saturating_duration_since(Instant::now()));
            let (cols, rows) = crossterm::terminal::size().context("terminal size")?;
            if let Some(stage) = renderer::Stage::new(Size {
                rows: usize::from(rows),
                cols: usize::from(cols),
            }) {
                renderer.resize(stage);
            }
        }
        let output = renderer.frame(elapsed);
        // Present the completed frame together, including its screen clear.
        stdout
            .sync_update(|stdout| stdout.write_all(output.as_bytes()))
            .context("synchronize preview frame")?
            .context("draw preview")?;
    }
    Ok(())
}
