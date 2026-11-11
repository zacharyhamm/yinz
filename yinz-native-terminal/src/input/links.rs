use winit::keyboard::ModifiersState;

use crate::emulation::screen_buffer::ScreenBuffer;

#[cfg_attr(test, path = "links/fake_launch.rs")]
pub(crate) mod launch;
use launch::launch;

const ALLOWED_SCHEMES: [&str; 6] = ["http", "https", "ftp", "ftps", "mailto", "file"];
const DELIBERATE_SCHEMES: [&str; 3] = ["file", "ftp", "ftps"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRun {
    pub id: u8,
    pub uri: String,
    pub spans: Vec<(usize, usize, usize)>,
}

impl LinkRun {
    pub fn covers(&self, row: usize, col: usize) -> bool {
        self.spans
            .iter()
            .any(|(at, start, end)| *at == row && col >= *start && col <= *end)
    }
}

pub fn run_at(buffer: &ScreenBuffer, row: usize, col: usize) -> Option<LinkRun> {
    let size = buffer.size();
    if row >= size.rows || col >= size.cols {
        return None;
    }
    let id = buffer.link_id_at(row, col);
    let uri = buffer.link_at(row, col)?.to_owned();

    let same = |row: usize, col: usize| buffer.link_id_at(row, col) == id;
    let row_start = |row: usize, from: usize| {
        let mut start = from;
        while start > 0 && same(row, start - 1) {
            start -= 1;
        }
        start
    };
    let row_end = |row: usize, from: usize| {
        let mut end = from;
        while end + 1 < size.cols && same(row, end + 1) {
            end += 1;
        }
        end
    };

    let mut spans = vec![(row, row_start(row, col), row_end(row, col))];
    let mut first = row;
    while first > 0 && spans[0].1 == 0 && same(first - 1, size.cols - 1) {
        first -= 1;
        spans.insert(0, (first, row_start(first, size.cols - 1), size.cols - 1));
    }
    let mut last = row;
    while last + 1 < size.rows && spans[spans.len() - 1].2 == size.cols - 1 && same(last + 1, 0) {
        last += 1;
        spans.push((last, 0, row_end(last, 0)));
    }

    Some(LinkRun { id, uri, spans })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Armed {
    pub uri: String,
    pub cell: (u16, u16),
    pub deliberate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    pub enabled: bool,
    pub with_shift: bool,
}

#[cfg(test)]
impl Opening {
    pub const OFF: Opening = Opening {
        enabled: false,
        with_shift: false,
    };
    pub const WITHOUT_SHIFT: Opening = Opening {
        enabled: true,
        with_shift: false,
    };
    pub const WITH_SHIFT: Opening = Opening {
        enabled: true,
        with_shift: true,
    };
}

pub fn armed_by_press(
    opening: Opening,
    pane_wants_mouse: bool,
    modifiers: ModifiersState,
    cell: Option<(u16, u16)>,
    hovered: Option<&LinkRun>,
) -> Option<Armed> {
    if !opening.enabled {
        return None;
    }
    let (x, y) = cell?;
    let run = hovered?;
    if !run.covers(y as usize, x as usize)
        || !is_openable(&run.uri)
        || !click_opens(opening, pane_wants_mouse, modifiers, &run.uri)
    {
        return None;
    }
    Some(Armed {
        uri: run.uri.clone(),
        cell: (x, y),
        deliberate: needs_deliberate_click(&run.uri),
    })
}

pub fn needs_deliberate_click(uri: &str) -> bool {
    scheme_of(uri).is_some_and(|scheme| {
        DELIBERATE_SCHEMES
            .iter()
            .any(|deliberate| scheme.eq_ignore_ascii_case(deliberate))
    })
}

pub fn deliberate_modifier_held(modifiers: ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.super_key() && !modifiers.control_key() && !modifiers.alt_key()
    } else {
        modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
    }
}

fn click_opens(
    opening: Opening,
    pane_wants_mouse: bool,
    modifiers: ModifiersState,
    uri: &str,
) -> bool {
    if needs_deliberate_click(uri) {
        deliberate_modifier_held(modifiers)
    } else {
        gesture_matches(opening, pane_wants_mouse, modifiers)
    }
}

pub fn released_on(armed: Option<Armed>, cell: Option<(u16, u16)>) -> Option<String> {
    let armed = armed?;
    (Some(armed.cell) == cell).then_some(armed.uri)
}

pub fn gesture_matches(
    opening: Opening,
    pane_wants_mouse: bool,
    modifiers: ModifiersState,
) -> bool {
    if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() {
        return false;
    }
    if modifiers.shift_key() {
        opening.with_shift || pane_wants_mouse
    } else {
        !pane_wants_mouse
    }
}

pub fn is_reachable(
    opening: Opening,
    pane_wants_mouse: bool,
    modifiers: ModifiersState,
    hovered: Option<&LinkRun>,
) -> bool {
    opening.enabled
        && hovered.is_some_and(|run| {
            is_openable(&run.uri) && click_opens(opening, pane_wants_mouse, modifiers, &run.uri)
        })
}

pub fn scheme_of(uri: &str) -> Option<&str> {
    let (scheme, _) = uri.split_once(':')?;
    if scheme.is_empty() {
        return None;
    }
    Some(scheme)
}

pub fn is_openable(uri: &str) -> bool {
    if uri.is_empty() || uri.len() > 2048 {
        return false;
    }
    if uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some(scheme) = scheme_of(uri) else {
        return false;
    };
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return false;
    }
    ALLOWED_SCHEMES
        .iter()
        .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
}

pub fn opener() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("xdg-open")
    } else if cfg!(target_os = "macos") {
        Some("open")
    } else if cfg!(windows) {
        Some(SHELL_EXECUTE)
    } else {
        None
    }
}

const SHELL_EXECUTE: &str = "ShellExecuteW";

pub fn open(uri: &str) -> Result<(), String> {
    if !is_openable(uri) {
        return Err(format!("refusing to open {:?}", uri));
    }
    let Some(opener) = opener() else {
        return Err("this platform has no known way to open a link".to_owned());
    };
    launch(opener, uri)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub struct ShellExecuteArgs {
    pub verb: Vec<u16>,
    pub file: Vec<u16>,
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn shell_execute_args(uri: &str) -> Result<ShellExecuteArgs, String> {
    if uri.contains('\0') {
        return Err(format!("refusing to open {:?}: it contains a NUL", uri));
    }
    let wide = |text: &str| text.encode_utf16().chain(std::iter::once(0)).collect();
    Ok(ShellExecuteArgs {
        verb: wide("open"),
        file: wide(uri),
    })
}

#[cfg(test)]
#[path = "unit/links_tests.rs"]
mod tests;
