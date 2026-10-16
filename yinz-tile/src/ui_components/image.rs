//! Drawing an image in a plugin pane, over the kitty graphics protocol.
//!
//! A plugin is in the same position as any other program inside a multiplexer: it does not own the
//! cursor, and the host throws its whole viewport away between frames
//! (`PluginPane::handle_plugin_bytes`). So the only placement style that works is the one the
//! protocol provides for exactly that case — a **virtual placement** (`U=1`) plus *unicode
//! placeholder* cells, which the terminal re-derives from the cells on every frame. Concretely:
//!
//! 1. [`transmit_png`] once per image, per client — the pixels land in the pane's image store.
//! 2. [`place_image`] once, to declare the virtual placement the cells will refer to.
//! 3. [`print_image_with_coordinates`] every frame, which is only text.
//!
//! Step 3 is cheap enough to sit in a render loop; steps 1 and 2 are not. Note that a plugin pane
//! holds one grid *per attached client* and `render` runs once per client, so "once" means once
//! per client — see `get_plugin_ids().client_id`. A client that reconnects comes back with a new
//! client id and a fresh grid, so a transmission cache keyed by client id never needs clearing for
//! it. Ordinary frames preserve the namespace.
//!
//! Ids are the caller's to choose and are scoped to the pane, so they cannot collide with a
//! terminal program's. Keep them under [`MAX_IMAGE_ID`]: the placeholder encoding carries the low
//! three bytes of the id in the cell's foreground colour, and anything above that needs a third
//! diacritic this module does not emit.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::Engine as _;
use yinz_types::kitty_placeholders::{PLACEHOLDER, ROWCOLUMN_DIACRITICS};

/// The largest image id [`image_row`] can encode: the id travels in a 24-bit foreground colour.
pub const MAX_IMAGE_ID: u32 = 0x00ff_ffff;

/// The protocol's limit on the base64 payload of one APC command.
const MAX_CHUNK: usize = 4096;

/// Transmit `png` as image `id`, ready to be placed. Sent as `a=t,f=100,t=d,q=2` in 4096-byte
/// base64 chunks; `q=2` because a plugin pane has no channel the terminal could reply on.
///
/// Re-transmitting the same id replaces the image, so this is safe to repeat — just not cheap.
pub fn transmit_png(id: u32, png: &[u8]) {
    print!("{}", png_transmit_sequence(id, png));
}

/// The bytes [`transmit_png`] prints, for a caller composing a frame of its own rather than
/// printing as it goes.
pub fn png_transmit_sequence(id: u32, png: &[u8]) -> String {
    let encoded = BASE64.encode(png);
    let mut sequence = String::with_capacity(encoded.len() + 64);
    let mut chunks = encoded.as_bytes().chunks(MAX_CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        // `chunk` came out of a base64 string, so it is ASCII by construction.
        let payload = std::str::from_utf8(chunk).unwrap_or_default();
        // Only the first chunk carries the control data; the rest are `m` plus payload, which is
        // what the protocol asks for and what keeps the sequences small.
        if first {
            sequence.push_str(&format!(
                "\u{1b}_Ga=t,f=100,t=d,q=2,i={},m={};{}\u{1b}\\",
                id, more, payload
            ));
            first = false;
        } else {
            sequence.push_str(&format!("\u{1b}_Gm={};{}\u{1b}\\", more, payload));
        }
    }
    sequence
}

/// Declare the virtual placement that [`image_row`]'s cells refer to: the image is scaled to
/// `cols` by `rows` cells, whatever the terminal's cell size turns out to be.
pub fn place_image(id: u32, cols: usize, rows: usize) {
    print!("{}", place_image_sequence(id, cols, rows));
}

/// The bytes [`place_image`] prints, for a caller composing a frame of its own.
pub fn place_image_sequence(id: u32, cols: usize, rows: usize) -> String {
    format!("\u{1b}_Ga=p,U=1,q=2,i={},c={},r={}\u{1b}\\", id, cols, rows)
}

/// One row of an image as placeholder cells: a truecolor foreground carrying the id, then `cols`
/// copies of `U+10EEEE`, each followed by the diacritics naming its row and column in the image.
///
/// Returned rather than printed so a caller can compose it into a line it is building - the
/// foreground and underline colour are reset around it, so it is safe to concatenate.
pub fn image_row(id: u32, row: usize, cols: usize) -> String {
    let (red, green, blue) = ((id >> 16) as u8, (id >> 8) as u8, id as u8);
    let mut line = format!("\u{1b}[59;38;2;{};{};{}m", red, green, blue);
    let row_mark = match ROWCOLUMN_DIACRITICS.get(row) {
        Some(mark) => *mark,
        // Beyond the table the row cannot be spelled at all, so an empty string is the honest
        // answer: the caller draws nothing rather than a row of the wrong part of the image.
        None => return String::new(),
    };
    for column in 0..cols {
        let Some(column_mark) = ROWCOLUMN_DIACRITICS.get(column) else {
            break;
        };
        line.push(PLACEHOLDER);
        line.push(row_mark);
        line.push(*column_mark);
    }
    line.push_str("\u{1b}[39;59m");
    line
}

/// Draw the `cols` by `rows` placement of image `id` with its top left corner at `x`, `y`.
///
/// Cheap: this is text, and the image itself was transmitted once by [`transmit_png`].
pub fn print_image_with_coordinates(id: u32, x: usize, y: usize, cols: usize, rows: usize) {
    for row in 0..rows {
        // Absolute positioning, one row at a time, so this composes with the rest of a plugin's
        // frame the same way `print_text_with_coordinates` does.
        print!(
            "\u{1b}[{};{}H{}",
            y + row + 1,
            x + 1,
            image_row(id, row, cols)
        );
    }
}

#[cfg(test)]
mod tests;
