use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// `text` cut to `max_width` columns, with a trailing `…` standing in for what was dropped.
///
/// Returns `text` untouched when it already fits. The ellipsis costs a column of its own, so a
/// truncated result is at most `max_width` wide - at `max_width` of 1 that leaves the ellipsis
/// alone, and at 0 there is no room for even that.
pub fn ellipsize(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let budget = max_width - 1;
    let mut truncated = String::new();
    let mut used = 0;
    for ch in text.chars() {
        // a zero-width combining mark would never advance `used`, so count it as a column
        let ch_width = ch.width().unwrap_or(0).max(1);
        if used + ch_width > budget {
            break;
        }
        truncated.push(ch);
        used += ch_width;
    }
    truncated.push('…');
    truncated
}

#[macro_export]
macro_rules! rgb {
    ($a:expr_2021) => {
        ansi_term::Color::Rgb($a.0, $a.1, $a.2)
    };
}

#[macro_export]
macro_rules! palette_match {
    ($palette_color:expr_2021) => {
        match $palette_color {
            PaletteColor::Rgb((r, g, b)) => RGB(r, g, b),
            PaletteColor::EightBit(color) => Fixed(color),
        }
    };
}

#[macro_export]
macro_rules! style {
    ($fg:expr_2021, $bg:expr_2021) => {
        ansi_term::Style::new()
            .fg(match $fg {
                PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
                PaletteColor::EightBit(color) => ansi_term::Color::Fixed(color),
            })
            .on(match $bg {
                PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
                PaletteColor::EightBit(color) => ansi_term::Color::Fixed(color),
            })
    };
}

#[cfg(test)]
#[path = "unit/lib.rs"]
mod tests;
