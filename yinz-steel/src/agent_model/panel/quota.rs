//! Bounded, font-free rounded digital numerals for the agents quota footer.
use anyhow::{ensure, Context, Result};
use yinz_types::{colors::eightbit_to_rgb, data::PaletteColor, style::TerminalTheme};

pub(super) fn render(
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
    let scale = (f64::from(width) / (digits.len() as f64 * 0.66 + 0.5)).min(f64::from(height));
    let left = f64::from(width) - scale * (digits.len() as f64 * 0.66 + 0.5);
    let top = (f64::from(height) - scale) / 2.0;
    let mut pixels = vec![0; width as usize * height as usize * 4];
    for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (i % width as usize) as f64 + 0.5;
        let y = (i / width as usize) as f64 + 0.5;
        let local_x = (x - left) / scale;
        let local_y = (y - top) / scale;
        let slot = (local_x / 0.66).floor() as usize;
        let distance = if slot < digits.len() {
            digit_distance(
                digits.as_bytes()[slot] - b'0',
                local_x - slot as f64 * 0.66,
                local_y,
            )
        } else {
            let x = local_x - digits.len() as f64 * 0.66;
            capsule(x, local_y, (0.08, 0.76), (0.4, 0.28), 0.028)
                .min(((x - 0.09).hypot(local_y - 0.32) - 0.065).abs() - 0.025)
                .min(((x - 0.39).hypot(local_y - 0.72) - 0.065).abs() - 0.025)
        };
        pixel.copy_from_slice(&[
            r,
            g,
            b,
            ((0.5 - distance * scale).clamp(0.0, 1.0) * 255.0).round() as u8,
        ]);
    }
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

fn capsule(x: f64, y: f64, a: (f64, f64), b: (f64, f64), radius: f64) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a.0 - t * dx).hypot(y - a.1 - t * dy) - radius
}

fn digit_distance(digit: u8, x: f64, y: f64) -> f64 {
    let mask = [
        0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111, 0b1110000,
        0b1111111, 0b1111011,
    ][usize::from(digit)];
    let segments = [
        ((0.13, 0.08), (0.45, 0.08)),
        ((0.51, 0.15), (0.51, 0.43)),
        ((0.51, 0.57), (0.51, 0.85)),
        ((0.13, 0.92), (0.45, 0.92)),
        ((0.07, 0.57), (0.07, 0.85)),
        ((0.07, 0.15), (0.07, 0.43)),
        ((0.13, 0.5), (0.45, 0.5)),
    ];
    segments
        .into_iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << (6 - i)) != 0)
        .map(|(_, (a, b))| capsule(x, y, a, b, 0.045))
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
#[path = "../../unit/quota.rs"]
mod tests;
