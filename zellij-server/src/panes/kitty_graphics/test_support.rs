//! Shared fixtures for the kitty graphics conformance suite.
//!
//! Every level of the suite that lives inside `zellij-server` — the `kitty_graphics/unit/` tests,
//! the grid tests under `panes/unit/grid_tests/`, and the host-emission tests under
//! `output/unit/` — reaches this module through `crate::panes::kitty_graphics::test_support`.
//!
//! The whole-app tests in `zellij-integration-tests` cannot: that crate depends on
//! `zellij-server`, `#[cfg(test)]` items are not in the rlib, and the reverse dependency edge
//! would be a cycle. Its byte-level subset is duplicated in
//! `zellij-integration-tests/src/kitty_protocol.rs`; keep the two reply grammars in step. The one
//! deliberate difference is that this parser panics on a malformed reply while that one skips it,
//! because that one reads a live pty stream from inside polling predicates.
//!
//! Naming mirrors kitty's own `graphics_tests.py` so ported assertions stay recognisable:
//! `apc_b64` is its `send_command`, [`KittyReply`] is its `Response`, and [`byte_block`] is its
//! filler generator.

use base64::engine::general_purpose::STANDARD as BASE64_ENCODER;
use base64::Engine as _;
use std::path::{Path, PathBuf};

use super::parser::{KittyCommand, KittyCommandParser, KittyError};

// ---- building command bytes ----

/// `ESC _G <control> ESC \` — a command with no payload.
pub fn apc(control: &str) -> Vec<u8> {
    apc_raw(control, &[])
}

/// `ESC _G <control> ; base64(payload) ESC \` — kitty's `send_command`.
pub fn apc_b64(control: &str, payload: &[u8]) -> Vec<u8> {
    apc_raw(control, BASE64_ENCODER.encode(payload).as_bytes())
}

/// Like [`apc_b64`] but the payload is emitted verbatim, for malformed-base64 cases.
pub fn apc_raw(control: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b_G");
    out.extend_from_slice(control.as_bytes());
    if !payload.is_empty() {
        out.push(b';');
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

// ---- parsing replies ----

/// A terminal-to-application graphics response, mirroring kitty's `parse_full_response`.
///
/// The protocol shape is `ESC _G <key>=<value>,... ; <code>[:<message>] ESC \`, where the code is
/// the literal `OK` on success and an errno-style name otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KittyReply {
    pub code: String,
    pub message: String,
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    /// The `r` key, used by animation frame responses.
    pub frame_number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KittyReplyParseError {
    NotAnApc,
    Unterminated,
    MissingSemicolon,
    MalformedKey(String),
    /// A key we do not know how to interpret. Treated as an error on purpose: it is what lets
    /// tests assert that every reply the implementation emits is well formed.
    UnknownKey(String),
    NonNumericValue(String),
    /// Spec: the response "message data will be a ASCII encoded string containing only printable
    /// characters and spaces". Checked here rather than in one test so that every level of the
    /// suite — including the generated input in `kitty_properties` — rejects a reply whose message
    /// could corrupt the framing of the ones after it.
    NonPrintableMessage(String),
}

impl KittyReply {
    pub fn parse(bytes: &[u8]) -> Result<Self, KittyReplyParseError> {
        let body = bytes
            .strip_prefix(b"\x1b_G".as_slice())
            .ok_or(KittyReplyParseError::NotAnApc)?;
        let body = body
            .strip_suffix(b"\x1b\\".as_slice())
            .ok_or(KittyReplyParseError::Unterminated)?;
        let semicolon = body
            .iter()
            .position(|byte| *byte == b';')
            .ok_or(KittyReplyParseError::MissingSemicolon)?;
        let (keys, status) = (&body[..semicolon], &body[semicolon + 1..]);

        let status = String::from_utf8_lossy(status).into_owned();
        let (code, message) = match status.split_once(':') {
            Some((code, message)) => (code.to_owned(), message.to_owned()),
            None => (status, String::new()),
        };

        if !message
            .bytes()
            .all(|byte| (0x20..0x7f).contains(&byte) && byte != b';')
        {
            return Err(KittyReplyParseError::NonPrintableMessage(message));
        }

        let mut reply = KittyReply {
            code,
            message,
            ..Default::default()
        };
        for segment in keys.split(|byte| *byte == b',') {
            if segment.is_empty() {
                continue;
            }
            let segment = String::from_utf8_lossy(segment).into_owned();
            let (key, value) = segment
                .split_once('=')
                .ok_or_else(|| KittyReplyParseError::MalformedKey(segment.clone()))?;
            let value = value
                .parse::<u32>()
                .map_err(|_| KittyReplyParseError::NonNumericValue(segment.clone()))?;
            match key {
                "i" => reply.image_id = Some(value),
                "I" => reply.image_number = Some(value),
                "p" => reply.placement_id = Some(value),
                "r" => reply.frame_number = Some(value),
                _ => return Err(KittyReplyParseError::UnknownKey(key.to_owned())),
            }
        }
        Ok(reply)
    }

    pub fn is_ok(&self) -> bool {
        self.code == "OK"
    }
}

/// Split a byte stream into APC sequences and parse each one. Panics with the offending bytes if
/// any of them is not a well-formed reply, so a malformed reply fails the test that produced it
/// rather than silently disappearing.
pub fn parse_replies(bytes: &[u8]) -> Vec<KittyReply> {
    let mut replies = Vec::new();
    let mut rest = bytes;
    while let Some(start) = find(rest, b"\x1b_G") {
        let after_start = &rest[start..];
        let end = match find(after_start, b"\x1b\\") {
            Some(end) => end + 2,
            None => panic!(
                "unterminated kitty reply in {:?}",
                String::from_utf8_lossy(after_start)
            ),
        };
        let sequence = &after_start[..end];
        match KittyReply::parse(sequence) {
            Ok(reply) => replies.push(reply),
            Err(e) => panic!(
                "malformed kitty reply {:?}: {:?}",
                String::from_utf8_lossy(sequence),
                e
            ),
        }
        rest = &after_start[end..];
    }
    replies
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---- parser-level helpers ----

/// Feed one complete command to a parser. Returns `None` while an upload is still in flight.
pub fn parse_one(
    parser: &mut KittyCommandParser,
    raw: &[u8],
) -> Option<Result<KittyCommand, KittyError>> {
    parser.parse(raw)
}

/// Strip the `ESC _G` / `ESC \` framing so raw command bytes can be handed to the parser, which
/// takes the payload of the APC rather than the APC itself.
pub fn unwrap_apc(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_prefix(b"\x1b_G".as_slice())
        .and_then(|rest| rest.strip_suffix(b"\x1b\\".as_slice()))
        .expect("not an APC sequence")
}

// ---- grid-level helpers ----

/// Drive bytes through the interceptor exactly as `TerminalPane::handle_pty_bytes` does: kitty
/// APCs are captured and handed to the grid, everything else reaches vte.
pub fn feed_bytes(
    grid: &mut crate::panes::grid::Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut super::interceptor::KittyApcInterceptor,
    bytes: &[u8],
) {
    use super::interceptor::InterceptorResult;
    for byte in bytes {
        match interceptor.advance(*byte) {
            InterceptorResult::Forward(forwarded) => {
                vte_parser.advance(grid, forwarded.as_slice());
            },
            InterceptorResult::Swallow => {},
            InterceptorResult::Captured(command) => {
                grid.handle_kitty_apc(&command);
            },
        }
    }
}

/// Send one graphics command and return the replies it produced, draining the grid's outbound
/// queue first so only this command's replies are returned. kitty's `send_command`.
pub fn send_kitty(
    grid: &mut crate::panes::grid::Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut super::interceptor::KittyApcInterceptor,
    control: &str,
    payload: &[u8],
) -> Vec<KittyReply> {
    grid.pending_messages_to_pty.clear();
    feed_bytes(grid, vte_parser, interceptor, &apc_b64(control, payload));
    drain_replies(grid)
}

/// [`send_kitty`] with the payload already base64-encoded. Chunked uploads have to split the
/// *encoded* stream on a four-character boundary; encoding each raw slice separately only happens
/// to reassemble when every slice but the last is a multiple of three bytes.
pub fn send_kitty_raw(
    grid: &mut crate::panes::grid::Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut super::interceptor::KittyApcInterceptor,
    control: &str,
    payload_b64: &str,
) -> Vec<KittyReply> {
    grid.pending_messages_to_pty.clear();
    feed_bytes(
        grid,
        vte_parser,
        interceptor,
        &apc_raw(control, payload_b64.as_bytes()),
    );
    drain_replies(grid)
}

/// [`send_kitty`], asserting the command produced at most one reply.
pub fn send_kitty_one(
    grid: &mut crate::panes::grid::Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut super::interceptor::KittyApcInterceptor,
    control: &str,
    payload: &[u8],
) -> Option<KittyReply> {
    let replies = send_kitty(grid, vte_parser, interceptor, control, payload);
    assert!(
        replies.len() <= 1,
        "expected at most one reply to {:?}, got {:?}",
        control,
        replies
    );
    replies.into_iter().next()
}

/// Take everything the grid has queued for the application and parse it as replies.
pub fn drain_replies(grid: &mut crate::panes::grid::Grid) -> Vec<KittyReply> {
    let queued: Vec<u8> = grid
        .pending_messages_to_pty
        .drain(..)
        .flatten()
        .collect::<Vec<u8>>();
    parse_replies(&queued)
}

// ---- deterministic payloads ----

/// kitty's `byte_block`: `n` bytes cycling `0..=255`.
pub fn byte_block(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 256) as u8).collect()
}

/// `width * height * 3` deterministic RGB bytes.
pub fn rgb_block(width: usize, height: usize) -> Vec<u8> {
    byte_block(width * height * 3)
}

/// `width * height * 4` deterministic RGBA bytes.
pub fn rgba_block(width: usize, height: usize) -> Vec<u8> {
    byte_block(width * height * 4)
}

// ---- PNG fixtures ----

/// The PNG colour types the suite exercises. The spec requires any PNG to be accepted and
/// normalised to RGBA, so all five must decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngKind {
    Rgb8,
    Rgba8,
    Gray8,
    GrayAlpha8,
    Palette8,
}

impl PngKind {
    fn colour_type(&self) -> u8 {
        match self {
            PngKind::Gray8 => 0,
            PngKind::Rgb8 => 2,
            PngKind::Palette8 => 3,
            PngKind::GrayAlpha8 => 4,
            PngKind::Rgba8 => 6,
        }
    }
    fn samples_per_pixel(&self) -> usize {
        match self {
            PngKind::Gray8 | PngKind::Palette8 => 1,
            PngKind::GrayAlpha8 => 2,
            PngKind::Rgb8 => 3,
            PngKind::Rgba8 => 4,
        }
    }
}

/// Encode a deterministic PNG of the given kind.
///
/// Written by hand rather than with the `png` crate's encoder: `png` is declared
/// `default-features = false` here, so its encoder is not guaranteed to be compiled in, and
/// enabling it would be a production dependency change made for tests. Uses no filtering
/// (filter byte 0 per scanline) and a single zlib-compressed IDAT.
pub fn png(width: u32, height: u32, kind: PngKind) -> Vec<u8> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    let samples = kind.samples_per_pixel();
    let mut raw = Vec::with_capacity((height as usize) * (1 + width as usize * samples));
    let mut next = 0u8;
    for _ in 0..height {
        raw.push(0); // filter type: None
        for _ in 0..width {
            for _ in 0..samples {
                raw.push(next);
                next = next.wrapping_add(1);
            }
        }
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).expect("zlib encode");
    let idat = encoder.finish().expect("zlib finish");

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(kind.colour_type());
    ihdr.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace
    push_png_chunk(&mut out, b"IHDR", &ihdr);

    if kind == PngKind::Palette8 {
        // 256 entries so every possible sample value indexes a real colour.
        let mut plte = Vec::with_capacity(256 * 3);
        for i in 0..256u32 {
            plte.push(i as u8);
            plte.push((i * 2 % 256) as u8);
            plte.push((i * 3 % 256) as u8);
        }
        push_png_chunk(&mut out, b"PLTE", &plte);
    }

    push_png_chunk(&mut out, b"IDAT", &idat);
    push_png_chunk(&mut out, b"IEND", &[]);
    out
}

fn push_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

// ---- file / temp-file fixtures ----

/// A file fixture that cleans itself up if the terminal did not already delete it. Tests assert
/// on [`FileFixture::exists`] after the transfer; the drop is only there to keep `/tmp` tidy.
pub struct FileFixture {
    pub path: PathBuf,
    _enclosing_dir: Option<tempfile::TempDir>,
}

impl FileFixture {
    pub fn exists(&self) -> bool {
        self.path.exists()
    }
    /// The base64 payload identifying this file to a `t=f` / `t=t` transfer.
    pub fn payload(&self) -> Vec<u8> {
        path_payload(&self.path)
    }
}

impl Drop for FileFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A file in a temp directory, with the `tty-graphics-protocol` marker in its name. Per the spec
/// both conditions must hold before a `t=t` transfer may delete the file, so this is the only
/// fixture a conforming terminal deletes.
pub fn marked_temp_file(bytes: &[u8]) -> FileFixture {
    write_fixture(std::env::temp_dir(), "tty-graphics-protocol-zellij", bytes)
}

/// A file in a temp directory *without* the marker in its path.
pub fn unmarked_temp_file(bytes: &[u8]) -> FileFixture {
    write_fixture(std::env::temp_dir(), "zellij-kitty-plain", bytes)
}

/// A file outside any temp directory, carrying the marker in its name.
pub fn marked_non_temp_file(bytes: &[u8]) -> FileFixture {
    let dir = tempfile::Builder::new()
        .prefix("kitty-conformance")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("create non-temp dir");
    let mut fixture = write_fixture(dir.path().to_path_buf(), "tty-graphics-protocol", bytes);
    fixture._enclosing_dir = Some(dir);
    fixture
}

/// A plain file outside any temp directory, for `t=f` transfers, which must never delete.
pub fn regular_file(bytes: &[u8]) -> FileFixture {
    let dir = tempfile::Builder::new()
        .prefix("kitty-conformance")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("create non-temp dir");
    let mut fixture = write_fixture(dir.path().to_path_buf(), "image", bytes);
    fixture._enclosing_dir = Some(dir);
    fixture
}

fn write_fixture(dir: PathBuf, prefix: &str, bytes: &[u8]) -> FileFixture {
    let path = dir.join(format!("{}-{}-{}", prefix, std::process::id(), unique()));
    std::fs::write(&path, bytes).expect("write file fixture");
    FileFixture {
        path,
        _enclosing_dir: None,
    }
}

/// Base64 of a path, which is how the `t=f`/`t=t`/`t=s` payload carries a file or object name.
pub fn path_payload(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---- unicode placeholder fixtures ----

/// `U+10EEEE`, the placeholder character that stands in for a virtual placement's cell.
pub const PLACEHOLDER: char = super::placeholders::PLACEHOLDER;

/// The first entries of kitty's `rowcolumn-diacritics.txt`. Index is the encoded row/column (and,
/// for the third diacritic on a cell, the most significant byte of the image id).
///
/// A prefix of the real table rather than a second copy of it: an encoder that disagreed with the
/// decoder would make every placeholder test assert against the wrong image row. It was a
/// hand-written copy until the table landed in `placeholders.rs`, and it had drifted — it carried
/// a `U+059A` at index 42 that kitty's file does not list at all, which shifted every entry after
/// it by one. `placeholders_tests::the_table_matches_the_fixture_the_conformance_suite_encodes_with`
/// is the guard against that happening again.
pub const ROWCOLUMN_DIACRITICS: [char; 44] = {
    let mut prefix = ['\0'; 44];
    let mut index = 0;
    while index < prefix.len() {
        prefix[index] = super::placeholders::ROWCOLUMN_DIACRITICS[index];
        index += 1;
    }
    prefix
};

/// The diacritic encoding the value `n`.
pub fn diacritic(n: usize) -> char {
    ROWCOLUMN_DIACRITICS[n]
}

/// One placeholder cell: an explicit row/column, and optionally the third diacritic carrying the
/// most significant byte of the image id. `None` fields exercise the inheritance rules, under
/// which a cell takes its row from the cell to its left and its column from that cell's plus one.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaceholderCell {
    pub row: Option<usize>,
    pub column: Option<usize>,
    pub id_most_significant_byte: Option<usize>,
}

impl PlaceholderCell {
    pub fn at(row: usize, column: usize) -> Self {
        PlaceholderCell {
            row: Some(row),
            column: Some(column),
            id_most_significant_byte: None,
        }
    }
    /// A cell carrying no diacritics at all, inheriting everything from its left neighbour.
    pub fn inherited() -> Self {
        PlaceholderCell::default()
    }
    pub fn with_id_msb(mut self, msb: usize) -> Self {
        self.id_most_significant_byte = Some(msb);
        self
    }
}

/// How the image id is encoded into the cell's foreground colour.
#[derive(Debug, Clone, Copy)]
pub enum PlaceholderColor {
    /// `ESC[38;5;<n>m` — the low byte of the id only.
    Indexed(u8),
    /// `ESC[38;2;<r>;<g>;<b>m` — the low three bytes of the id.
    Rgb(u8, u8, u8),
}

/// Build the text that materialises a virtual placement: an SGR run setting the foreground (and
/// optionally the underline colour, which selects among several placements of one image),
/// followed by one placeholder character per cell with its combining marks.
pub fn placeholder_cells(
    colour: PlaceholderColor,
    underline_colour: Option<u8>,
    cells: &[PlaceholderCell],
) -> String {
    let mut out = String::new();
    match colour {
        PlaceholderColor::Indexed(n) => out.push_str(&format!("\x1b[38;5;{}m", n)),
        PlaceholderColor::Rgb(r, g, b) => out.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b)),
    }
    if let Some(underline) = underline_colour {
        out.push_str(&format!("\x1b[58;5;{}m", underline));
    }
    for cell in cells {
        out.push(PLACEHOLDER);
        if let Some(row) = cell.row {
            out.push(diacritic(row));
        }
        if let Some(column) = cell.column {
            out.push(diacritic(column));
        }
        if let Some(msb) = cell.id_most_significant_byte {
            out.push(diacritic(msb));
        }
    }
    out.push_str("\x1b[0m");
    out
}
