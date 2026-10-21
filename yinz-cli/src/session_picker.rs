//! The session picker `yinz attach` opens when no name is given and several
//! sessions are running, as plain state: keys go in, lines come out. Reading
//! the keyboard and drawing is the binary's job (`src/session_menu.rs`), which
//! keeps everything here testable without a terminal.
use crate::sessions::{format_session_line, SessionListing};
use std::time::Duration;
use unicode_width::UnicodeWidthChar;
use yinz_types::internal_ids::SessionName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Local(SessionName),
    Remote { host: String, name: SessionName },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerRow {
    pub label: String,
    pub destination: Option<Destination>,
    pub status: Option<String>,
    pub age: Duration,
    pub listing: SessionListing,
    /// The session this command is running inside.
    pub current: bool,
}

impl PickerRow {
    fn selectable(&self) -> bool {
        self.destination.is_some()
            && !self.current
            && !matches!(
                self.listing,
                SessionListing::Unresponsive | SessionListing::Denied
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKey {
    Char(char),
    Backspace,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Tab,
    BackTab,
    Enter,
    Escape,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Picked(Destination),
    /// Left without picking, with something to tell the person, if anything.
    Cancelled(Option<String>),
    Interrupted,
}

pub struct PickerState {
    rows: Vec<PickerRow>,
    /// Indices into `rows` that match `input`, in `rows` order.
    visible: Vec<usize>,
    input: String,
    /// Index into `visible`.
    cursor: usize,
    message: Option<String>,
    /// What Enter does, for the footer: "attach" or "switch".
    verb: &'static str,
    page: usize,
}

impl PickerState {
    pub fn new(rows: Vec<PickerRow>, verb: &'static str) -> Self {
        let visible: Vec<usize> = (0..rows.len()).collect();
        let cursor = rows.iter().position(PickerRow::selectable).unwrap_or(0);
        PickerState {
            rows,
            visible,
            input: String::new(),
            cursor,
            message: None,
            verb,
            page: 10,
        }
    }

    pub fn replace_rows(&mut self, rows: Vec<PickerRow>) {
        let held = self
            .selected()
            .filter(|row| row.selectable())
            .and_then(|row| row.destination.clone());
        self.rows = rows;
        self.visible.clear();
        self.refilter();
        if let Some(destination) = held
            && let Some(index) = self
                .visible
                .iter()
                .position(|&i| self.rows[i].destination.as_ref() == Some(&destination))
        {
            self.cursor = index;
        }
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    /// The row under the cursor, if any row is visible.
    pub fn selected(&self) -> Option<&PickerRow> {
        self.visible.get(self.cursor).map(|&i| &self.rows[i])
    }

    /// The names that match the filter, in display order.
    pub fn visible_names(&self) -> Vec<&str> {
        self.visible
            .iter()
            .map(|&i| self.rows[i].label.as_str())
            .collect()
    }

    /// How far PageUp and PageDown move; the driver sets it from the
    /// terminal's height.
    pub fn set_page_size(&mut self, page: usize) {
        self.page = page.max(1);
    }

    pub fn handle(&mut self, key: PickerKey) -> Outcome {
        self.message = None;
        let last = self.visible.len().saturating_sub(1);
        match key {
            PickerKey::Char(c) if !c.is_control() => {
                self.input.push(c);
                self.refilter();
            },
            PickerKey::Char(_) => {},
            PickerKey::Backspace => {
                if self.input.pop().is_some() {
                    self.refilter();
                }
            },
            PickerKey::Up => self.cursor = self.cursor.saturating_sub(1),
            PickerKey::Down => self.cursor = (self.cursor + 1).min(last),
            PickerKey::PageUp => self.cursor = self.cursor.saturating_sub(self.page),
            PickerKey::PageDown => self.cursor = (self.cursor + self.page).min(last),
            PickerKey::Home => self.cursor = 0,
            PickerKey::End => self.cursor = last,
            PickerKey::Tab => self.complete(true),
            PickerKey::BackTab => self.complete(false),
            PickerKey::Enter => return self.pick(),
            PickerKey::Escape => return Outcome::Cancelled(None),
            PickerKey::Interrupt => return Outcome::Interrupted,
        }
        Outcome::Continue
    }

    fn pick(&mut self) -> Outcome {
        let Some(row) = self.selected() else {
            return Outcome::Continue;
        };
        let name = row.label.as_str().to_owned();
        if row.destination.is_none() {
            Outcome::Continue
        } else if row.current {
            Outcome::Cancelled(Some(format!("Already in session {name:?}.")))
        } else if row.listing == SessionListing::Unresponsive {
            self.message = Some(
                if matches!(row.destination, Some(Destination::Remote { .. })) {
                    format!("{name} is not responding on its SSH host")
                } else {
                    format!("{name} is not responding; stop it with `yinz kill-session {name}`")
                },
            );
            Outcome::Continue
        } else if row.listing == SessionListing::Denied {
            self.message = Some(format!("{name} cannot be reached from here"));
            Outcome::Continue
        } else {
            Outcome::Picked(row.destination.clone().expect("selectable destination"))
        }
    }

    /// Shell-style completion. Extends the input to the longest prefix every
    /// candidate shares; once there is nothing left to add, steps the cursor
    /// through the candidates instead.
    fn complete(&mut self, forward: bool) {
        let candidates = self.candidates();
        if candidates.is_empty() {
            return;
        }
        if candidates.len() == 1 {
            let row = self.visible[candidates[0]];
            self.input = self.rows[row].label.as_str().to_owned();
            self.refilter();
            self.cursor_to_row(row);
            return;
        }
        let names: Vec<&str> = candidates
            .iter()
            .map(|&p| self.rows[self.visible[p]].label.as_str())
            .collect();
        let prefix = common_prefix(&names);
        let extends_input = prefix.chars().count() > self.input.chars().count()
            && prefix
                .to_lowercase()
                .starts_with(&self.input.to_lowercase());
        if extends_input {
            let first = self.visible[candidates[0]];
            let on_a_candidate = candidates.contains(&self.cursor);
            let held = self.visible.get(self.cursor).copied();
            self.input = prefix;
            self.refilter();
            match held {
                Some(row) if on_a_candidate => self.cursor_to_row(row),
                _ => self.cursor_to_row(first),
            }
            return;
        }
        let next = if forward {
            candidates
                .iter()
                .find(|&&p| p > self.cursor)
                .unwrap_or(&candidates[0])
        } else {
            candidates
                .iter()
                .rev()
                .find(|&&p| p < self.cursor)
                .unwrap_or(&candidates[candidates.len() - 1])
        };
        self.cursor = *next;
    }

    /// Positions in `visible` whose names start with the input, or every
    /// visible position when none do (the input only matched mid-name).
    fn candidates(&self) -> Vec<usize> {
        let input = self.input.to_lowercase();
        let prefixed: Vec<usize> = (0..self.visible.len())
            .filter(|&p| {
                self.rows[self.visible[p]]
                    .label
                    .as_str()
                    .to_lowercase()
                    .starts_with(&input)
            })
            .collect();
        if prefixed.is_empty() {
            (0..self.visible.len()).collect()
        } else {
            prefixed
        }
    }

    fn refilter(&mut self) {
        let held = self.visible.get(self.cursor).copied();
        let needle = self.input.to_lowercase();
        self.visible = (0..self.rows.len())
            .filter(|&i| self.rows[i].label.as_str().to_lowercase().contains(&needle))
            .collect();
        self.cursor = held
            .and_then(|row| self.visible.iter().position(|&i| i == row))
            .or_else(|| self.visible.iter().position(|&i| self.rows[i].selectable()))
            .unwrap_or(0);
    }

    fn cursor_to_row(&mut self, row: usize) {
        if let Some(p) = self.visible.iter().position(|&i| i == row) {
            self.cursor = p;
        }
    }

    /// The menu as lines, none wider than `width` columns and at most
    /// `max_lines` of them, so a redraw can clear exactly what it drew.
    /// On a terminal too short for all three parts the footer goes first,
    /// then the list; the input line always shows.
    pub fn render(&self, width: usize, max_lines: usize) -> Vec<String> {
        let max_lines = max_lines.max(1);
        let footer = max_lines >= 3;
        let body = max_lines - 1 - usize::from(footer);
        let mut lines = vec![format!(
            "\u{1b}[1mSession:\u{1b}[m {}\u{1b}[7m \u{1b}[m",
            self.input
        )];
        if body == 0 {
            // Only the input line fits.
        } else if self.visible.is_empty() {
            lines.push("  (no matching sessions)".to_owned());
        } else {
            let start = self
                .cursor
                .saturating_sub(body / 2)
                .min(self.visible.len().saturating_sub(body));
            for (p, &i) in self.visible.iter().enumerate().skip(start).take(body) {
                let row = &self.rows[i];
                let marker = if p == self.cursor {
                    "\u{1b}[1m>\u{1b}[m "
                } else {
                    "  "
                };
                let line = match &row.status {
                    Some(status) => format!("{} ({status})", row.label),
                    None if matches!(row.destination, Some(Destination::Remote { .. }))
                        && row.listing == SessionListing::Unresponsive =>
                    {
                        format!(
                            "{} (UNRESPONSIVE)",
                            format_session_line(
                                &row.label,
                                row.age,
                                SessionListing::Running,
                                false,
                                true
                            )
                        )
                    },
                    None => format_session_line(
                        row.label.as_str(),
                        row.age,
                        row.listing,
                        row.current,
                        true,
                    ),
                };
                lines.push(format!("{marker}{line}"));
            }
        }
        if footer {
            lines.push(match &self.message {
                Some(message) => format!("\u{1b}[31;1m{message}\u{1b}[m"),
                None => format!(
                    "\u{1b}[2m({} of {} · Tab complete · ↑↓ move · Enter {} · Esc quit)\u{1b}[m",
                    self.visible.len(),
                    self.rows.len(),
                    self.verb
                ),
            });
        }
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width))
            .collect()
    }
}

/// The longest prefix all `names` share, compared character by character.
fn common_prefix(names: &[&str]) -> String {
    let Some((first, rest)) = names.split_first() else {
        return String::new();
    };
    let mut len = first.len();
    for name in rest {
        len = first
            .char_indices()
            .zip(name.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map(|((i, a), _)| i + a.len_utf8())
            .unwrap_or(0)
            .min(len);
    }
    first[..len].to_owned()
}

/// Turns bytes read from a terminal in key mode into keys. A key's bytes can
/// straddle two reads - a slow link splits an escape sequence, a paste splits
/// wherever the buffer ends - so an unfinished tail is held for the next
/// `feed`. Only the driver can tell a lone ESC from the start of a sequence,
/// by the silence after it; it calls `flush` then, and that ESC is the Escape
/// key.
#[derive(Debug, Default)]
pub struct KeyParser {
    pending: Vec<u8>,
}

impl KeyParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<PickerKey> {
        self.pending.extend_from_slice(bytes);
        let bytes = std::mem::take(&mut self.pending);
        let mut keys = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match next_key(&bytes[i..]) {
                Step::Key(key, len) => {
                    keys.push(key);
                    i += len;
                },
                Step::Skip(len) => i += len,
                Step::Incomplete => {
                    self.pending = bytes[i..].to_vec();
                    break;
                },
            }
        }
        keys
    }

    /// Whether a key is part-read, so the driver should wait only briefly
    /// before calling `flush`.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Nothing more came: a held ESC was the Escape key, and any other
    /// unfinished tail is dropped.
    pub fn flush(&mut self) -> Vec<PickerKey> {
        match std::mem::take(&mut self.pending).as_slice() {
            [0x1b] => vec![PickerKey::Escape],
            _ => Vec::new(),
        }
    }
}

enum Step {
    /// A key, and how many bytes it took.
    Key(PickerKey, usize),
    /// Bytes that are no key this menu knows.
    Skip(usize),
    /// The start of a key whose remaining bytes have not arrived.
    Incomplete,
}

fn next_key(bytes: &[u8]) -> Step {
    let b = bytes[0];
    let key = match b {
        0x1b => return escape_sequence(bytes),
        0x03 => PickerKey::Interrupt,
        b'\t' => PickerKey::Tab,
        b'\r' | b'\n' => PickerKey::Enter,
        0x7f | 0x08 => PickerKey::Backspace,
        0x10 => PickerKey::Up,   // Ctrl-P
        0x0e => PickerKey::Down, // Ctrl-N
        _ if b < 0x20 => return Step::Skip(1),
        _ => return utf8_char(bytes),
    };
    Step::Key(key, 1)
}

/// `bytes` starts with ESC. Followed by anything but `[` or `O`, including
/// another ESC, it is the Escape key on its own.
fn escape_sequence(bytes: &[u8]) -> Step {
    match bytes.get(1) {
        None => Step::Incomplete,
        Some(b'[') | Some(b'O') => {
            let Some(len) = bytes[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                return Step::Incomplete;
            };
            let params = &bytes[2..2 + len];
            let used = 2 + len + 1;
            let key = match (bytes[2 + len], params) {
                (b'A', _) => PickerKey::Up,
                (b'B', _) => PickerKey::Down,
                (b'H', _) | (b'~', b"1") | (b'~', b"7") => PickerKey::Home,
                (b'F', _) | (b'~', b"4") | (b'~', b"8") => PickerKey::End,
                (b'Z', _) => PickerKey::BackTab,
                (b'~', b"5") => PickerKey::PageUp,
                (b'~', b"6") => PickerKey::PageDown,
                _ => return Step::Skip(used),
            };
            Step::Key(key, used)
        },
        Some(_) => Step::Key(PickerKey::Escape, 1),
    }
}

fn utf8_char(bytes: &[u8]) -> Step {
    let len = match bytes[0] {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    };
    if bytes.len() < len {
        return Step::Incomplete;
    }
    match std::str::from_utf8(&bytes[..len])
        .ok()
        .and_then(|s| s.chars().next())
    {
        Some(c) => Step::Key(PickerKey::Char(c), len),
        // Only the bad byte, so a stray lead byte cannot eat the key after it.
        None => Step::Skip(1),
    }
}

/// Cuts `line` to at most `width` visible columns. It skips over the SGR
/// escapes this module writes, and resets attributes after a cut so a
/// truncated colour does not bleed into the next line.
pub fn truncate_to_width(line: &str, width: usize) -> String {
    let mut out = String::with_capacity(line.len());
    let mut used = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            out.push(c);
            for e in chars.by_ref() {
                out.push(e);
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        let w = c.width().unwrap_or(0);
        if used + w > width {
            out.push_str("\u{1b}[m");
            return out;
        }
        used += w;
        out.push(c);
    }
    out
}
