use crate::agents;
use unicode_width::UnicodeWidthStr;

/// The row the divider draws, never wider than `cols`.
///
/// Split out from the drawing because it is the one line in the pane that has to fit three
/// things onto a row rather than one, and a string is what a test can measure.
///
/// A label is text from anywhere - a todo, a run's title, a refusal - and one carrying a line
/// feed or an escape would move the terminal's cursor rather than print: a line feed on the
/// pane's last row scrolls the whole pane up. So every control character is drawn as a space.
pub fn divider_line(label: &str, above: usize, below: usize, cols: usize) -> String {
    let label: String = label
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let position = scroll_readout(above, below);
    // The readout shares the row with the label, so it is taken out of the label's room rather
    // than out of the rule: how far back the history is wound is worth more than the last few
    // characters of a kind, and this is the only place that knows both.
    let label = agents::fit_label(label, cols.saturating_sub(position.width()));
    let mut line = match label.is_empty() {
        true => "──".to_owned(),
        false => format!("─ {label} "),
    };
    let rule = cols
        .saturating_sub(line.width())
        .saturating_sub(position.width());
    line.push_str(&"─".repeat(rule));
    line.push_str(&position);
    line
}

/// The scroll position [`divider_line`] draws at the right-hand end of the rule: both ends of
/// it, so a half wound back into the past says how far and how much is still ahead of it. No
/// scrollbar: nothing in the tree draws one.
///
/// One function for every surface that fits a label into what the readout leaves of the row,
/// so the room they fit to and the readout drawn cannot disagree.
pub fn scroll_readout(above: usize, below: usize) -> String {
    match (above, below) {
        (0, 0) => String::new(),
        (above, 0) => format!(" ↑{above} "),
        (0, below) => format!(" ↓{below} "),
        (above, below) => format!(" ↑{above} ↓{below} "),
    }
}

/// `text` broken into lines of at most `width` columns, between words where it can be and
/// through a word only where one is wider than the line. Plain text, not markdown: a refusal
/// quotes paths and messages, whose `*` and `_` mean nothing.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_owned();
        loop {
            let needed = match line.is_empty() {
                true => word.width(),
                false => line.width() + 1 + word.width(),
            };
            if needed <= width {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&word);
                break;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            // A word too wide for a line of its own: as much of it as fits, and the rest on
            // the next.
            let head = agents::cut_to_width(word.clone(), width);
            let taken = head.chars().count().max(1);
            lines.push(word.chars().take(taken).collect());
            word = word.chars().skip(taken).collect();
            if word.is_empty() {
                break;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

use yinz_tile::prelude::{print_text_with_coordinates, Text};

pub const MIN_HISTORY_ROWS: usize = 2;
pub const FOCUS_MARK_COLS: usize = 2;
pub const PANEL_KEYS: &[(&str, &str)] = &[
    ("Ctrl e o", "move the panel (Ctrl-o when locked)"),
    ("?", "shut this sheet"),
];

pub fn paint(text: Text, x: usize, y: usize, width: Option<usize>, height: Option<usize>) {
    print_text_with_coordinates(text, x, y, width, height);
}

pub fn render_divider(
    label: &str,
    above: usize,
    below: usize,
    focused: bool,
    y: usize,
    cols: usize,
) {
    paint(
        divider_text(label, above, below, focused, cols),
        0,
        y,
        Some(cols),
        None,
    );
}

pub fn divider_text(label: &str, above: usize, below: usize, focused: bool, cols: usize) -> Text {
    let label = match (focused, label.is_empty()) {
        (true, true) => "▸".to_owned(),
        (true, false) => format!("▸ {label}"),
        (false, true) => String::new(),
        (false, false) => format!("  {label}"),
    };
    let line = divider_line(&label, above, below, cols);
    // Found rather than counted: a label cut to fit may have lost the marker, and no kind can
    // contain one (`valid_kind` allows no punctuation).
    let marker = label
        .contains('!')
        .then(|| line.chars().position(|c| c == '!'))
        .flatten();
    let text = Text::new(line);
    // Keep the waiting marker bright so it stands out from an unfocused rule.
    let text = match (focused, marker) {
        (true, _) => text.color_range(2, ..),
        (false, Some(at)) => text.dim_range(..at).dim_range(at + 1..),
        (false, None) => text.dim_range(..),
    };
    match marker {
        Some(at) => text.error_color_range(at..at + 1),
        None => text,
    }
}

pub fn split(rows: usize) -> (usize, usize) {
    let history_rows = rows / 2;
    let list_rows = rows.saturating_sub(history_rows + 1);
    // Measured rather than thresholded, so the two minimums stay the thing that decides and
    // there is no second expression of the arithmetic to keep in step with this one.
    if list_rows < 3 || history_rows < MIN_HISTORY_ROWS {
        return (rows, 0);
    }
    (list_rows, history_rows)
}

pub fn single_line(text: &str) -> String {
    let pieces: Vec<&str> = text.split(['\n', '\r']).collect();
    let last = pieces.len() - 1;
    let mut line = String::with_capacity(text.len());
    for (index, piece) in pieces.into_iter().enumerate() {
        let piece: String = piece
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let piece = match (index == 0, index == last) {
            (true, true) => piece.as_str(),
            (true, false) => piece.trim_end(),
            (false, true) => piece.trim_start(),
            (false, false) => piece.trim(),
        };
        if piece.is_empty() {
            continue;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(piece);
    }
    line
}

pub fn sheet(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(keys, what)| ((*keys).to_owned(), (*what).to_owned()))
        .collect()
}

pub fn key_sheet_lines(
    keys: &str,
    what: &str,
    key_width: usize,
    cols: usize,
) -> Vec<(String, usize)> {
    let keys = wrap_words(keys, key_width);
    let what = wrap_words(what, cols.saturating_sub(1 + key_width + 2));
    (0..keys.len().max(what.len()).max(1))
        .map(|row| {
            let piece = keys.get(row).map_or("", String::as_str);
            let mut line = format!(" {piece}");
            let key_chars = match piece.is_empty() {
                true => 0,
                false => line.chars().count(),
            };
            if let Some(what) = what.get(row) {
                line.push_str(&" ".repeat(key_width.saturating_sub(piece.width()) + 2));
                line.push_str(what);
            }
            (agents::cut_to_width(line, cols), key_chars)
        })
        .collect()
}
