use yinz_types::{data::PaletteColor, pane_size::Size};

/// Maps a tab's logical coordinates onto one client's terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presentation {
    pub terminal: Size,
    pub content: Size,
    pub background: PaletteColor,
}

impl Presentation {
    pub fn offset(self) -> (usize, usize) {
        (
            self.terminal.cols.saturating_sub(self.content.cols) / 2,
            self.terminal.rows.saturating_sub(self.content.rows) / 2,
        )
    }

    fn margins(self) -> String {
        let (dx, dy) = self.offset();
        // Repaint margins after display erases. Geometry/theme changes request a full
        // repaint, so incremental and control-only frames need no margin output. Save both cursor
        // position and rendition so control-only frames do not disturb the application.
        let mut margins = String::from("\x1b7\x1b[0m");
        match self.background {
            PaletteColor::Rgb((r, g, b)) => margins.push_str(&format!("\x1b[48;2;{r};{g};{b}m")),
            PaletteColor::EightBit(n) => margins.push_str(&format!("\x1b[48;5;{n}m")),
        }
        for y in 0..self.terminal.rows {
            let spans = if y < dy || y >= dy + self.content.rows {
                [(0, self.terminal.cols), (0, 0)]
            } else {
                [
                    (0, dx),
                    (
                        dx + self.content.cols,
                        self.terminal.cols.saturating_sub(dx + self.content.cols),
                    ),
                ]
            };
            for (x, width) in spans {
                if width > 0 {
                    margins.push_str(&format!("\x1b[{};{}H{}", y + 1, x + 1, " ".repeat(width)));
                }
            }
        }
        margins.push_str("\x1b8");
        margins
    }

    /// Translate the renderer's absolute cursor commands, leaving string control payloads
    /// (OSC, DCS and APC, including graphics and hyperlinks) opaque. Relative movements
    /// and saved cursors already operate in the translated coordinate system.
    pub fn present(self, source: String) -> String {
        if self.terminal.cols <= self.content.cols && self.terminal.rows <= self.content.rows {
            return source;
        }
        let (dx, dy) = self.offset();
        let mut output = String::with_capacity(source.len());
        let mut margins = None;
        let bytes = source.as_bytes();
        let mut start = 0;
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] != 27 {
                i += 1;
                continue;
            }
            match bytes[i + 1] {
                b']' | b'P' | b'_' | b'^' | b'X' => {
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 7 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 27 && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                },
                b'[' => {
                    let begin = i;
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i == bytes.len() {
                        break;
                    }
                    let params = &source[begin + 2..i];
                    if matches!(bytes[i], b'H' | b'f') {
                        let mut parts = params.split(';');
                        let row = parts
                            .next()
                            .unwrap_or("")
                            .parse::<usize>()
                            .unwrap_or(1)
                            .max(1);
                        let col = parts
                            .next()
                            .unwrap_or("")
                            .parse::<usize>()
                            .unwrap_or(1)
                            .max(1);
                        output.push_str(&source[start..begin]);
                        output.push_str(&format!("\x1b[{};{}H", row + dy, col + dx));
                        start = i + 1;
                    } else if bytes[i] == b'J' && matches!(params, "2" | "3") {
                        output.push_str(&source[start..=i]);
                        output.push_str(margins.get_or_insert_with(|| self.margins()));
                        start = i + 1;
                    }
                    i += 1;
                },
                _ => i += 2,
            }
        }
        output.push_str(&source[start..]);
        output
    }
}

#[cfg(test)]
#[path = "unit/presentation_tests.rs"]
mod tests;
