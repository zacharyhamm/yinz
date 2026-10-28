//! Bounded, font-free rounded numerals for the agents quota footer.
use anyhow::{ensure, Context, Result};
use yinz_types::{colors::eightbit_to_rgb, data::PaletteColor, style::TerminalTheme};

pub fn render(
    value: u64,
    color: PaletteColor,
    terminal: Option<TerminalTheme>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let pixels = render_rgba(value, color, terminal, width, height)?;
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .context("encoding quota PNG header")?;
        writer
            .write_image_data(&pixels)
            .context("encoding quota PNG pixels")?;
    }
    Ok(png)
}

/// The bounded RGBA raster underlying the panel PNG and native overlay image.
pub fn render_rgba(
    value: u64,
    color: PaletteColor,
    terminal: Option<TerminalTheme>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    ensure!(
        width > 0
            && width <= 1024
            && height > 0
            && height <= 256
            && u64::from(width) * u64::from(height) <= 131_072,
        "quota image exceeds pixel budget"
    );
    let digits = value.to_string();
    ensure!(digits.len() <= 12, "quota value exceeds digit budget");
    let (r, g, b) = match color {
        PaletteColor::Rgb(rgb) => rgb,
        PaletteColor::EightBit(index) => terminal
            .and_then(|t| t.ansi.get(usize::from(index)).copied())
            .unwrap_or_else(|| eightbit_to_rgb(index)),
    };
    let strokes: Vec<_> = digits
        .bytes()
        .map(|digit| digit_strokes(digit - b'0'))
        .collect();
    let scale = (f64::from(width) / (digits.len() as f64 * 0.62 + 0.4)).min(f64::from(height));
    let left = f64::from(width) - scale * (digits.len() as f64 * 0.62 + 0.4);
    let top = (f64::from(height) - scale) / 2.0;
    let mut pixels = vec![0; width as usize * height as usize * 4];
    for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (i % width as usize) as f64 + 0.5;
        let y = (i / width as usize) as f64 + 0.5;
        let local_x = (x - left) / scale;
        let local_y = (y - top) / scale;
        let slot = (local_x / 0.62).floor() as usize;
        let distance = if slot < digits.len() {
            strokes[slot]
                .windows(2)
                .map(|pair| {
                    capsule(
                        local_x - slot as f64 * 0.62,
                        local_y,
                        pair[0],
                        pair[1],
                        0.026,
                    )
                })
                .fold(f64::INFINITY, f64::min)
        } else {
            let x = local_x - digits.len() as f64 * 0.62;
            capsule(x, local_y, (0.07, 0.9), (0.31, 0.5), 0.022)
                .min(((x - 0.075).hypot(local_y - 0.55) - 0.045).abs() - 0.018)
                .min(((x - 0.305).hypot(local_y - 0.85) - 0.045).abs() - 0.018)
        };
        pixel.copy_from_slice(&[
            r,
            g,
            b,
            ((0.5 - distance * scale).clamp(0.0, 1.0) * 255.0).round() as u8,
        ]);
    }
    Ok(pixels)
}

fn capsule(x: f64, y: f64, a: (f64, f64), b: (f64, f64), radius: f64) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a.0 - t * dx).hypot(y - a.1 - t * dy) - radius
}

fn digit_strokes(digit: u8) -> Vec<(f64, f64)> {
    // Each path alternates endpoints and quadratic controls; joined capsules keep round caps.
    let path: &[(f64, f64)] = match digit {
        0 => &[
            (0.30, 0.08),
            (0.08, 0.08),
            (0.08, 0.50),
            (0.08, 0.92),
            (0.30, 0.92),
            (0.52, 0.92),
            (0.52, 0.50),
            (0.52, 0.08),
            (0.30, 0.08),
        ],
        1 => &[
            (0.15, 0.24),
            (0.24, 0.16),
            (0.33, 0.08),
            (0.33, 0.50),
            (0.33, 0.92),
        ],
        2 => &[
            (0.08, 0.24),
            (0.13, 0.08),
            (0.30, 0.08),
            (0.52, 0.08),
            (0.52, 0.28),
            (0.52, 0.40),
            (0.34, 0.56),
            (0.08, 0.78),
            (0.08, 0.92),
            (0.30, 0.92),
            (0.52, 0.92),
        ],
        3 => &[
            (0.09, 0.17),
            (0.18, 0.08),
            (0.30, 0.08),
            (0.52, 0.08),
            (0.52, 0.28),
            (0.52, 0.48),
            (0.28, 0.48),
            (0.54, 0.48),
            (0.54, 0.70),
            (0.54, 0.92),
            (0.30, 0.92),
            (0.16, 0.92),
            (0.08, 0.82),
        ],
        4 => &[
            (0.44, 0.92),
            (0.44, 0.50),
            (0.44, 0.08),
            (0.26, 0.36),
            (0.08, 0.64),
            (0.31, 0.64),
            (0.54, 0.64),
        ],
        5 => &[
            (0.51, 0.08),
            (0.31, 0.08),
            (0.12, 0.08),
            (0.10, 0.28),
            (0.08, 0.48),
            (0.18, 0.42),
            (0.30, 0.42),
            (0.54, 0.42),
            (0.54, 0.67),
            (0.54, 0.92),
            (0.30, 0.92),
            (0.16, 0.92),
            (0.08, 0.82),
        ],
        6 => &[
            (0.48, 0.12),
            (0.41, 0.08),
            (0.32, 0.08),
            (0.08, 0.08),
            (0.08, 0.54),
            (0.08, 0.92),
            (0.30, 0.92),
            (0.52, 0.92),
            (0.52, 0.69),
            (0.52, 0.46),
            (0.30, 0.46),
            (0.16, 0.46),
            (0.08, 0.59),
        ],
        7 => &[
            (0.08, 0.08),
            (0.31, 0.08),
            (0.54, 0.08),
            (0.36, 0.50),
            (0.18, 0.92),
        ],
        8 => &[
            (0.30, 0.48),
            (0.10, 0.42),
            (0.10, 0.27),
            (0.10, 0.08),
            (0.30, 0.08),
            (0.50, 0.08),
            (0.50, 0.27),
            (0.50, 0.42),
            (0.30, 0.48),
            (0.08, 0.54),
            (0.08, 0.71),
            (0.08, 0.92),
            (0.30, 0.92),
            (0.52, 0.92),
            (0.52, 0.71),
            (0.52, 0.54),
            (0.30, 0.48),
        ],
        9 => &[
            (0.12, 0.88),
            (0.19, 0.92),
            (0.28, 0.92),
            (0.52, 0.92),
            (0.52, 0.46),
            (0.52, 0.08),
            (0.30, 0.08),
            (0.08, 0.08),
            (0.08, 0.31),
            (0.08, 0.54),
            (0.30, 0.54),
            (0.44, 0.54),
            (0.52, 0.41),
        ],
        _ => unreachable!("decimal digit"),
    };
    let mut points = vec![path[0]];
    for curve in path.windows(3).step_by(2) {
        for step in 1..=12 {
            let t = f64::from(step) / 12.0;
            let u = 1.0 - t;
            points.push((
                u * u * curve[0].0 + 2.0 * u * t * curve[1].0 + t * t * curve[2].0,
                u * u * curve[0].1 + 2.0 * u * t * curve[1].1 + t * t * curve[2].1,
            ));
        }
    }
    points
}

#[cfg(test)]
#[path = "unit/quota.rs"]
mod tests;

/// Panel quota labels and colors, computed from the last retained observation.
#[derive(Clone, Debug)]
pub struct Display {
    pub value: Option<u64>,
    pub remaining: Option<f64>,
    pub stale: bool,
    pub weekly_label: String,
    pub short: String,
    pub other: Vec<String>,
    pub compact: String,
    pub reset: String,
}
impl Display {
    pub fn new(usage: &yinz_types::agent::control::ProviderUsage, now: u64) -> Self {
        use yinz_types::agent::control::ProviderUsageStatus;
        let stale = usage.status != ProviderUsageStatus::Ok
            || usage
                .observed_at_ms
                .is_some_and(|at| now.saturating_sub(at) >= 120_000);
        let weekly = usage
            .windows
            .iter()
            .find(|w| matches!(w.label.as_str(), "Weekly limit" | "7d"));
        let short = usage
            .windows
            .iter()
            .find(|w| matches!(w.label.as_str(), "5-hour limit" | "5-hour" | "5h"));
        let remaining = weekly.map(|w| (100.0 - w.used_percent).clamp(0.0, 100.0));
        let value = remaining.map(|n| n.round() as u64);
        let short = short
            .map(|w| {
                format!(
                    "5hr · {}%",
                    (100.0 - w.used_percent).clamp(0.0, 100.0).round() as u64
                )
            })
            .unwrap_or_default();
        let mut parts = vec![];
        if let Some(value) = value {
            parts.push(format!("Weekly · {value}%"));
        }
        if !short.is_empty() {
            parts.push(short.clone());
        }
        let other: Vec<_> = usage
            .windows
            .iter()
            .filter(|w| {
                !matches!(
                    w.label.as_str(),
                    "Weekly limit" | "7d" | "5-hour limit" | "5-hour" | "5h"
                )
            })
            .map(|w| {
                format!(
                    "{} · {}%",
                    w.label,
                    (100.0 - w.used_percent).clamp(0.0, 100.0).round() as u64
                )
            })
            .collect();
        parts.extend(other.iter().cloned());
        if stale && !parts.is_empty() {
            parts.push("stale".into());
        }
        if parts.is_empty() {
            parts.push(usage.message.clone().unwrap_or_else(|| {
                match usage.status {
                    ProviderUsageStatus::Expired => "Credentials expired",
                    ProviderUsageStatus::Error => "Refresh failed",
                    ProviderUsageStatus::Unsupported => "Unsupported",
                    _ => "Unavailable",
                }
                .into()
            }));
        }
        let reset = weekly
            .and_then(|w| w.resets_at_ms)
            .or_else(|| usage.windows.iter().find_map(|w| w.resets_at_ms))
            .map(|reset| {
                let left = reset.saturating_sub(now);
                match left {
                    0 => "reset due".into(),
                    1..60_000 => "<1m left".into(),
                    60_000..3_600_000 => format!("{}m left", left / 60_000),
                    3_600_000..86_400_000 => format!("{}h left", left / 3_600_000),
                    _ => format!("{}d left", left / 86_400_000),
                }
            })
            .unwrap_or_default();
        Self {
            value,
            remaining,
            stale,
            weekly_label: if stale { "Weekly · stale" } else { "Weekly" }.into(),
            short,
            other,
            compact: parts.join(" · "),
            reset,
        }
    }
    pub fn color(&self, style: &yinz_types::style::Style) -> PaletteColor {
        let colors = &style.colors;
        match self.remaining.filter(|_| !self.stale) {
            Some(n) if n > 25.0 => colors.exit_code_success.base,
            Some(n) if n >= 10.0 => colors.text_unselected.emphasis_1,
            Some(_) => colors.exit_code_error.base,
            None => colors.text_unselected.base,
        }
    }
}
