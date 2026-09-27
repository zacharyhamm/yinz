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
