use crate::emulation::color::Paints;
use crate::emulation::palette::xparse_color;
use crate::input::clipboard::{self, ClipboardHandle};
use crate::session::Geometry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Query {
    TextAreaPixelSize,
    CharacterCellPixelSize,
    DefaultForeground,
    DefaultBackground,
    PaletteRegister(u8),
}

pub fn answer(
    query_bytes: &[u8],
    geometry: Geometry,
    paints: &Paints,
    clipboard: Option<&ClipboardHandle>,
) -> Vec<u8> {
    if let Some(reply) =
        clipboard.and_then(|clipboard| clipboard::osc52_reply(query_bytes, clipboard))
    {
        return reply;
    }
    match parse(query_bytes) {
        Some((query, terminator)) => reply(query, geometry, paints, terminator),
        None => Vec::new(),
    }
}

fn reply(query: Query, geometry: Geometry, paints: &Paints, terminator: &[u8]) -> Vec<u8> {
    let mut reply = match query {
        Query::TextAreaPixelSize => format!(
            "\x1b[4;{};{}t",
            geometry.rows * geometry.cell_height,
            geometry.cols * geometry.cell_width
        )
        .into_bytes(),
        Query::CharacterCellPixelSize => {
            format!("\x1b[6;{};{}t", geometry.cell_height, geometry.cell_width).into_bytes()
        },
        Query::DefaultForeground => {
            format!("\x1b]10;{}", xparse_color(paints.foreground)).into_bytes()
        },
        Query::DefaultBackground => {
            format!("\x1b]11;{}", xparse_color(paints.background)).into_bytes()
        },
        Query::PaletteRegister(index) => {
            format!("\x1b]4;{};{}", index, xparse_color(paints.indexed(index))).into_bytes()
        },
    };
    reply.extend_from_slice(terminator);
    reply
}

fn parse(bytes: &[u8]) -> Option<(Query, &'static [u8])> {
    match bytes {
        b"\x1b[14t" => return Some((Query::TextAreaPixelSize, b"")),
        b"\x1b[16t" => return Some((Query::CharacterCellPixelSize, b"")),
        _ => {},
    }
    let rest = bytes.strip_prefix(b"\x1b]")?;
    if let Some(rest) = rest.strip_prefix(b"10;?") {
        return terminator(rest).map(|terminator| (Query::DefaultForeground, terminator));
    }
    if let Some(rest) = rest.strip_prefix(b"11;?") {
        return terminator(rest).map(|terminator| (Query::DefaultBackground, terminator));
    }
    let rest = rest.strip_prefix(b"4;")?;
    let separator = rest.iter().position(|byte| *byte == b';')?;
    let index: u8 = std::str::from_utf8(&rest[..separator]).ok()?.parse().ok()?;
    let rest = rest[separator + 1..].strip_prefix(b"?")?;
    terminator(rest).map(|terminator| (Query::PaletteRegister(index), terminator))
}

fn terminator(bytes: &[u8]) -> Option<&'static [u8]> {
    match bytes {
        b"\x07" => Some(b"\x07"),
        b"\x1b\\" => Some(b"\x1b\\"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "unit/host_reply_tests.rs"]
mod tests;
