use std::io::{Read, Seek};
use std::path::Path;

pub const MAX_DECODED_BYTES: usize = 104_857_600;
const MAX_ENCODED_BYTES: usize = MAX_DECODED_BYTES.div_ceil(3) * 4;
pub const MAX_DIMENSION: u32 = 10_000;

/// How deep a chain of relative placements may go before the terminal refuses with `ETOODEEP`.
///
/// Spec, "Relative placements": "the terminal is free to impose a limit on the depth of the chain,
/// but it must support a depth of at least 8". Resolving a chain is a walk per placement per
/// refresh, so the limit is what keeps that bounded.
pub const MAX_RELATIVE_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyAction {
    Transmit,
    TransmitAndDisplay,
    Display,
    Delete,
    Query,
    /// `a=f`: transmit one animation frame of an existing image.
    Frame,
    /// `a=a`: control playback of an image's animation.
    Animate,
    /// `a=c`: compose a rectangle of one frame onto another.
    Compose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyFormat {
    Rgb24,
    Rgba32,
    Png,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyMedium {
    Direct,
    File,
    TempFile,
    SharedMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyErrorCode {
    Einval,
    Enoent,
    Ebadf,
    Ebadpng,
    Enodata,
    Enospc,
    Enotsupported,
    /// Spec, "Relative placements": the parent image or placement a `P`/`Q` pair names does not
    /// exist.
    Enoparent,
    /// The parent chain a relative placement would join loops back to itself.
    Ecycle,
    /// The parent chain is longer than [`MAX_RELATIVE_DEPTH`].
    Etoodeep,
}

impl KittyErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            KittyErrorCode::Einval => "EINVAL",
            KittyErrorCode::Enoent => "ENOENT",
            KittyErrorCode::Ebadf => "EBADF",
            KittyErrorCode::Ebadpng => "EBADPNG",
            KittyErrorCode::Enodata => "ENODATA",
            KittyErrorCode::Enospc => "ENOSPC",
            KittyErrorCode::Enotsupported => "ENOTSUPPORTED",
            KittyErrorCode::Enoparent => "ENOPARENT",
            KittyErrorCode::Ecycle => "ECYCLE",
            KittyErrorCode::Etoodeep => "ETOODEEP",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KittyError {
    pub code: KittyErrorCode,
    pub message: String,
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub quiet: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedImage {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: KittyFormat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KittyCommand {
    pub action: KittyAction,
    pub format: KittyFormat,
    pub medium: KittyMedium,
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub compressed: bool,
    pub more_chunks: bool,
    pub data_size: Option<u32>,
    pub data_offset: Option<u32>,
    pub source_x: u32,
    pub source_y: u32,
    pub source_w: u32,
    pub source_h: u32,
    pub columns: u32,
    pub rows: u32,
    pub cell_offset_x: u32,
    pub cell_offset_y: u32,
    pub z_index: i32,
    pub quiet: u8,
    pub suppress_cursor_movement: bool,
    pub delete_specifier: Option<char>,
    /// The `U` key: this is a *virtual* placement, a prototype that unicode placeholder cells
    /// instantiate rather than something drawn at the cursor.
    pub virtual_placement: bool,
    /// The `P` key: the image id of the placement this one is positioned relative to. `P=0` is
    /// read as "absent" rather than as image zero, matching how `i=0` is read elsewhere.
    pub parent_image_id: Option<u32>,
    /// The `Q` key: which placement *of* [`Self::parent_image_id`] to hang off. Absent means any
    /// of them, which is what lets a client name a virtual parent it never gave a placement id.
    pub parent_placement_id: Option<u32>,
    /// The `H` and `V` keys: the offset from the parent's top-left corner in cells, right and down
    /// positive.
    pub relative_offset_cells: (i32, i32),
    pub image: Option<DecodedImage>,
}

impl KittyCommand {
    /// Whether this command asks for a placement positioned from another placement rather than
    /// from the cursor.
    pub fn is_relative(&self) -> bool {
        self.parent_image_id.is_some()
    }
}

/// The animation actions read the same key letters as the placement actions, with entirely
/// different meanings. Because `a=` may arrive *after* the keys it reinterprets, the keys are
/// parsed into the fields above regardless of action and given their animation meaning here, at
/// the point of use.
///
/// | key | field | `a=f` frame loading | `a=a` control | `a=c` composition |
/// |-----|-------|---------------------|---------------|-------------------|
/// | `c` | `columns` | base frame to copy | frame to make current | destination frame |
/// | `r` | `rows` | frame to edit | frame whose gap changes | source frame |
/// | `x` | `source_x` | left edge of the data | — | destination left edge |
/// | `y` | `source_y` | top edge of the data | — | destination top edge |
/// | `w` | `source_w` | — | — | rectangle width |
/// | `h` | `source_h` | — | — | rectangle height |
/// | `s` | `pixel_width` | data width (unchanged) | playback state | — |
/// | `v` | `pixel_height` | data height (unchanged) | loop count | — |
/// | `X` | `cell_offset_x` | composition mode | — | source left edge |
/// | `Y` | `cell_offset_y` | background colour | — | source top edge |
/// | `z` | `z_index` | gap in milliseconds | gap in milliseconds | — |
impl KittyCommand {
    /// `c`: the frame whose data is the base canvas for a new frame (`a=f`), or the frame to make
    /// current (`a=a`), or the destination of a composition (`a=c`). Zero means absent.
    pub fn base_frame(&self) -> Option<u32> {
        Some(self.columns).filter(|frame| *frame != 0)
    }
    /// `r`: the frame being edited (`a=f`, `a=a`) or read from (`a=c`). Zero means absent, which
    /// for `a=f` is the request to make a new frame.
    pub fn frame_target(&self) -> Option<u32> {
        Some(self.rows).filter(|frame| *frame != 0)
    }
    /// `z`: the gap in milliseconds before the next frame. Negative marks a gapless frame, one
    /// that is never displayed and exists only as base data for later frames.
    pub fn frame_gap_ms(&self) -> i32 {
        self.z_index
    }
    /// `s` under `a=a`: 1 stops the animation, 2 runs it while waiting for more frames, 3 runs it
    /// normally.
    pub fn animation_state(&self) -> Option<u32> {
        self.pixel_width.filter(|state| *state != 0)
    }
    /// `v` under `a=a`: how many times to loop. 0 is ignored, 1 is infinite.
    pub fn animation_loop_count(&self) -> Option<u32> {
        self.pixel_height.filter(|count| *count != 0)
    }
    /// `X` under `a=f`: 1 overwrites the destination pixels, anything else alpha-blends onto them.
    pub fn frame_overwrites(&self) -> bool {
        self.cell_offset_x == 1
    }
    /// `Y` under `a=f`: the 32-bit RGBA colour the parts of a new frame the data does not cover are
    /// filled with. The default, 0, is a transparent black pixel.
    pub fn frame_background(&self) -> u32 {
        self.cell_offset_y
    }
}

impl Default for KittyCommand {
    fn default() -> Self {
        KittyCommand {
            action: KittyAction::Transmit,
            format: KittyFormat::Rgba32,
            medium: KittyMedium::Direct,
            image_id: None,
            image_number: None,
            placement_id: None,
            pixel_width: None,
            pixel_height: None,
            compressed: false,
            more_chunks: false,
            data_size: None,
            data_offset: None,
            source_x: 0,
            source_y: 0,
            source_w: 0,
            source_h: 0,
            columns: 0,
            rows: 0,
            cell_offset_x: 0,
            cell_offset_y: 0,
            z_index: 0,
            quiet: 0,
            suppress_cursor_movement: false,
            delete_specifier: None,
            virtual_placement: false,
            parent_image_id: None,
            parent_placement_id: None,
            relative_offset_cells: (0, 0),
            image: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct EchoFields {
    image_id: Option<u32>,
    image_number: Option<u32>,
    placement_id: Option<u32>,
    quiet: u8,
}

impl EchoFields {
    fn from_command(command: &KittyCommand) -> Self {
        EchoFields {
            image_id: command.image_id,
            image_number: command.image_number,
            placement_id: command.placement_id,
            quiet: command.quiet,
        }
    }
    fn error(&self, code: KittyErrorCode, message: &str) -> KittyError {
        KittyError {
            code,
            message: message.to_owned(),
            image_id: self.image_id,
            image_number: self.image_number,
            placement_id: self.placement_id,
            quiet: self.quiet,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KittyCommandParser {
    pending: Option<PendingUpload>,
}

#[derive(Debug, Clone)]
struct PendingUpload {
    command: KittyCommand,
    payload_b64: Vec<u8>,
}

impl KittyCommandParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        KittyCommandParser { pending: None }
    }
    pub fn parse(&mut self, raw: &[u8]) -> Option<Result<KittyCommand, KittyError>> {
        let (control, payload) = split_control_payload(raw);
        if self.pending.is_some() {
            let pairs = match tokenize(control) {
                Ok(pairs) => pairs,
                Err(()) => {
                    let pending = self.pending.take().unwrap();
                    let echo = EchoFields::from_command(&pending.command);
                    return Some(Err(
                        echo.error(KittyErrorCode::Einval, "malformed control data")
                    ));
                },
            };
            let is_delete = pairs
                .iter()
                .any(|(key, value)| *key == b"a" && *value == b"d");
            if is_delete {
                self.pending = None;
            } else {
                let mut more_chunks = false;
                for (key, value) in &pairs {
                    if *key == b"m" {
                        match parse_u32(value) {
                            Some(0) => more_chunks = false,
                            Some(1) => more_chunks = true,
                            _ => {
                                let pending = self.pending.take().unwrap();
                                let echo = EchoFields::from_command(&pending.command);
                                return Some(Err(
                                    echo.error(KittyErrorCode::Einval, "malformed control data")
                                ));
                            },
                        }
                    }
                }
                for (key, value) in &pairs {
                    if *key == b"q"
                        && let Some(q @ 0..=2) = parse_u32(value)
                    {
                        self.pending.as_mut().unwrap().command.quiet = q as u8;
                    }
                }
                let pending = self.pending.as_mut().unwrap();
                if payload.len() > MAX_ENCODED_BYTES.saturating_sub(pending.payload_b64.len()) {
                    let error = EchoFields::from_command(&pending.command)
                        .error(KittyErrorCode::Einval, "image too large");
                    self.pending = None;
                    return Some(Err(error));
                }
                pending.payload_b64.extend_from_slice(payload);
                if more_chunks {
                    return None;
                }
                let mut pending = self.pending.take().unwrap();
                pending.command.more_chunks = false;
                return Some(run_payload_pipeline(pending.command, &pending.payload_b64));
            }
        }
        let command = match parse_control_data(control) {
            Ok(command) => command,
            Err(err) => return Some(Err(err)),
        };
        match command.action {
            KittyAction::Delete
            | KittyAction::Display
            | KittyAction::Animate
            | KittyAction::Compose => Some(Ok(command)),
            // Spec: frame data "is transmitted in exactly the same way as image data", so `a=f`
            // goes through chunking and the payload pipeline unchanged - `f`, `s`, `v`, `o`, `t`,
            // `S` and `O` all mean for a frame what they mean for an image.
            KittyAction::Frame
            | KittyAction::Transmit
            | KittyAction::TransmitAndDisplay
            | KittyAction::Query => {
                if payload.len() > MAX_ENCODED_BYTES {
                    return Some(Err(EchoFields::from_command(&command)
                        .error(KittyErrorCode::Einval, "image too large")));
                }
                if command.more_chunks {
                    self.pending = Some(PendingUpload {
                        command,
                        payload_b64: payload.to_vec(),
                    });
                    None
                } else {
                    Some(run_payload_pipeline(command, payload))
                }
            },
        }
    }
    pub fn abort_pending(&mut self) {
        self.pending = None;
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

fn split_control_payload(raw: &[u8]) -> (&[u8], &[u8]) {
    match raw.iter().position(|b| *b == b';') {
        Some(pos) => (&raw[..pos], &raw[pos + 1..]),
        None => (raw, &raw[raw.len()..]),
    }
}

/// The `key=value` pairs of a kitty graphics control string, borrowed from it.
type ControlPairs<'a> = Vec<(&'a [u8], &'a [u8])>;

fn tokenize(control: &[u8]) -> Result<ControlPairs<'_>, ()> {
    let mut pairs = Vec::new();
    if control.is_empty() {
        return Ok(pairs);
    }
    for segment in control.split(|b| *b == b',') {
        if segment.is_empty() {
            continue;
        }
        let eq_pos = segment.iter().position(|b| *b == b'=').ok_or(())?;
        if eq_pos == 0 {
            return Err(());
        }
        pairs.push((&segment[..eq_pos], &segment[eq_pos + 1..]));
    }
    Ok(pairs)
}

fn parse_u32(value: &[u8]) -> Option<u32> {
    std::str::from_utf8(value).ok()?.parse::<u32>().ok()
}

fn parse_i32(value: &[u8]) -> Option<i32> {
    std::str::from_utf8(value).ok()?.parse::<i32>().ok()
}

fn scan_echo_fields(control: &[u8]) -> EchoFields {
    let mut echo = EchoFields::default();
    for segment in control.split(|b| *b == b',') {
        if let Some(eq_pos) = segment.iter().position(|b| *b == b'=') {
            let key = &segment[..eq_pos];
            let value = &segment[eq_pos + 1..];
            match key {
                b"q" => {
                    if let Some(q @ 0..=2) = parse_u32(value) {
                        echo.quiet = q as u8;
                    }
                },
                b"i" => echo.image_id = parse_u32(value).or(echo.image_id),
                b"I" => echo.image_number = parse_u32(value).or(echo.image_number),
                b"p" => echo.placement_id = parse_u32(value).or(echo.placement_id),
                _ => {},
            }
        }
    }
    echo
}

pub fn parse_control_data(control: &[u8]) -> Result<KittyCommand, KittyError> {
    let echo = scan_echo_fields(control);
    let pairs = tokenize(control)
        .map_err(|_| echo.error(KittyErrorCode::Einval, "malformed control data"))?;
    let mut command = KittyCommand::default();
    for (key, value) in pairs {
        match key {
            b"a" => {
                if value.len() != 1 {
                    return Err(echo.error(KittyErrorCode::Einval, "invalid action"));
                }
                command.action = match value[0] {
                    b't' => KittyAction::Transmit,
                    b'T' => KittyAction::TransmitAndDisplay,
                    b'p' => KittyAction::Display,
                    b'd' => KittyAction::Delete,
                    b'q' => KittyAction::Query,
                    b'f' => KittyAction::Frame,
                    b'a' => KittyAction::Animate,
                    b'c' => KittyAction::Compose,
                    _ => return Err(echo.error(KittyErrorCode::Einval, "invalid action")),
                };
            },
            b"f" => {
                command.format = match parse_u32(value) {
                    Some(24) => KittyFormat::Rgb24,
                    Some(32) => KittyFormat::Rgba32,
                    Some(100) => KittyFormat::Png,
                    _ => return Err(echo.error(KittyErrorCode::Einval, "invalid format")),
                };
            },
            b"t" => {
                if value.len() != 1 {
                    return Err(echo.error(KittyErrorCode::Einval, "invalid transmission medium"));
                }
                command.medium = match value[0] {
                    b'd' => KittyMedium::Direct,
                    b'f' => KittyMedium::File,
                    b't' => KittyMedium::TempFile,
                    b's' => KittyMedium::SharedMemory,
                    _ => {
                        return Err(
                            echo.error(KittyErrorCode::Einval, "invalid transmission medium")
                        );
                    },
                };
            },
            b"s" => {
                command.pixel_width = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 's'")
                })?);
            },
            b"v" => {
                command.pixel_height = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'v'")
                })?);
            },
            b"o" => {
                if value != b"z" {
                    return Err(echo.error(KittyErrorCode::Einval, "invalid compression"));
                }
                command.compressed = true;
            },
            b"m" => {
                command.more_chunks = match parse_u32(value) {
                    Some(0) => false,
                    Some(1) => true,
                    _ => {
                        return Err(echo.error(KittyErrorCode::Einval, "invalid value for key 'm'"));
                    },
                };
            },
            b"S" => {
                command.data_size = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'S'")
                })?);
            },
            b"O" => {
                command.data_offset = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'O'")
                })?);
            },
            b"i" => {
                command.image_id = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'i'")
                })?);
            },
            b"I" => {
                command.image_number = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'I'")
                })?);
            },
            b"p" => {
                command.placement_id = Some(parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'p'")
                })?);
            },
            b"x" => {
                command.source_x = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'x'")
                })?;
            },
            b"y" => {
                command.source_y = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'y'")
                })?;
            },
            b"w" => {
                command.source_w = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'w'")
                })?;
            },
            b"h" => {
                command.source_h = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'h'")
                })?;
            },
            b"c" => {
                command.columns = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'c'")
                })?;
            },
            b"r" => {
                command.rows = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'r'")
                })?;
            },
            b"X" => {
                command.cell_offset_x = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'X'")
                })?;
            },
            b"Y" => {
                command.cell_offset_y = parse_u32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'Y'")
                })?;
            },
            b"z" => {
                command.z_index = parse_i32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'z'")
                })?;
            },
            b"q" => {
                command.quiet = match parse_u32(value) {
                    Some(q @ 0..=2) => q as u8,
                    _ => {
                        return Err(echo.error(KittyErrorCode::Einval, "invalid value for key 'q'"));
                    },
                };
            },
            b"C" => {
                command.suppress_cursor_movement = match parse_u32(value) {
                    Some(0) => false,
                    Some(1) => true,
                    _ => {
                        return Err(echo.error(KittyErrorCode::Einval, "invalid value for key 'C'"));
                    },
                };
            },
            b"d" => {
                if value.len() != 1 || !b"aAiInNcCpPqQxXyYzZrRfF".contains(&value[0]) {
                    return Err(echo.error(KittyErrorCode::Einval, "invalid delete specifier"));
                }
                command.delete_specifier = Some(value[0] as char);
            },
            b"U" => match parse_u32(value) {
                Some(virtual_placement) => command.virtual_placement = virtual_placement != 0,
                None => {
                    return Err(echo.error(KittyErrorCode::Einval, "invalid value for key 'U'"));
                },
            },
            // Spec, "Relative placements". `P=0` means "no parent" the way `i=0` means "no image
            // id": zero is the key's documented default, so spelling it out cannot be a request to
            // hang off image zero.
            b"P" => {
                command.parent_image_id = match parse_u32(value) {
                    Some(0) => None,
                    Some(parent_image_id) => Some(parent_image_id),
                    None => {
                        return Err(echo.error(KittyErrorCode::Einval, "invalid value for key 'P'"));
                    },
                };
            },
            b"Q" => {
                command.parent_placement_id = match parse_u32(value) {
                    Some(0) => None,
                    Some(parent_placement_id) => Some(parent_placement_id),
                    None => {
                        return Err(echo.error(KittyErrorCode::Einval, "invalid value for key 'Q'"));
                    },
                };
            },
            b"H" => {
                command.relative_offset_cells.0 = parse_i32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'H'")
                })?;
            },
            b"V" => {
                command.relative_offset_cells.1 = parse_i32(value).ok_or_else(|| {
                    echo.error(KittyErrorCode::Einval, "invalid value for key 'V'")
                })?;
            },
            b"N" => {},
            _ => {},
        }
    }
    if command.image_id.unwrap_or(0) != 0 && command.image_number.is_some() {
        return Err(echo.error(
            KittyErrorCode::Einval,
            "must not specify both image id and image number",
        ));
    }
    Ok(command)
}

/// Spec, "the transmission medium": a `t=t` transfer may delete the file only when it is in a
/// known temporary directory **and** its path contains `tty-graphics-protocol`. The marker is what
/// distinguishes a file the client created for this transfer from any other file it happened to
/// name, so both conditions have to hold before deleting somebody's data.
fn should_delete_temp_file(path: &Path) -> bool {
    // Resolve `..` and directory symlinks, but keep the final entry: remove_file unlinks
    // that entry, whereas reading the image follows a final symlink to its target.
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    let Ok(parent) = parent.join(".").canonicalize() else {
        return false;
    };
    let path = parent.join(name);
    let in_a_temporary_directory = [std::env::temp_dir(), "/tmp".into(), "/dev/shm".into()]
        .iter()
        .filter_map(|directory| directory.canonicalize().ok())
        .any(|directory| path.starts_with(directory));
    in_a_temporary_directory && path.to_string_lossy().contains("tty-graphics-protocol")
}

/// Read the `O`/`S` window of an already-open file, shared by every medium that names one.
fn read_range(
    mut file: std::fs::File,
    len: u64,
    command: &KittyCommand,
    echo: &EchoFields,
    unreadable: &str,
) -> Result<Vec<u8>, KittyError> {
    let start = command.data_offset.unwrap_or(0) as u64;
    if start > len {
        return Err(echo.error(KittyErrorCode::Ebadf, unreadable));
    }
    let available = len - start;
    let length = match command.data_size {
        Some(size) if size > 0 => std::cmp::min(size as u64, available),
        _ => available,
    };
    if length > MAX_DECODED_BYTES as u64 {
        return Err(echo.error(KittyErrorCode::Einval, "image too large"));
    }
    let mut read = || -> std::io::Result<Vec<u8>> {
        if start > 0 {
            file.seek(std::io::SeekFrom::Start(start))?;
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(length)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    read().map_err(|_| echo.error(KittyErrorCode::Ebadf, unreadable))
}

/// Read a `t=s` POSIX shared memory object and unlink it, as the spec requires: "The terminal
/// emulator must read the data from the memory object and then unlink and close it on POSIX."
/// The unlink happens whatever the read did, so a client that sent an unreadable object does not
/// leak it until reboot.
#[cfg(unix)]
fn read_shared_memory_object(
    name: &str,
    command: &KittyCommand,
    echo: &EchoFields,
) -> Result<Vec<u8>, KittyError> {
    use std::os::unix::io::FromRawFd;

    const UNREADABLE: &str = "could not read shared memory object";
    let name =
        std::ffi::CString::new(name).map_err(|_| echo.error(KittyErrorCode::Ebadf, UNREADABLE))?;
    // SAFETY: `name` is a valid NUL-terminated C string for the duration of the call, and the
    // returned descriptor is handed straight to `File`, which owns and closes it.
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(echo.error(KittyErrorCode::Ebadf, UNREADABLE));
    }
    // SAFETY: `fd` is a freshly opened descriptor this function exclusively owns.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let read_result = match file.metadata() {
        Ok(metadata) => read_range(file, metadata.len(), command, echo, UNREADABLE),
        Err(_) => Err(echo.error(KittyErrorCode::Ebadf, UNREADABLE)),
    };
    // SAFETY: same string, still valid; unlinking a name that is already gone is harmless.
    unsafe { libc::shm_unlink(name.as_ptr()) };
    read_result
}

#[cfg(not(unix))]
fn read_shared_memory_object(
    _name: &str,
    _command: &KittyCommand,
    echo: &EchoFields,
) -> Result<Vec<u8>, KittyError> {
    Err(echo.error(
        KittyErrorCode::Enotsupported,
        "shared memory transfer is not supported",
    ))
}

/// Decodes base64 whose padding has already been trimmed. Like the `base64` crate's strict
/// decoding, it rejects whitespace, stray padding and non-zero trailing bits.
pub(crate) fn decode_base64(unpadded: &[u8]) -> Result<Vec<u8>, base64_simd::Error> {
    base64_simd::STANDARD_NO_PAD.decode_to_vec(unpadded)
}

fn run_payload_pipeline(
    mut command: KittyCommand,
    payload_b64: &[u8],
) -> Result<KittyCommand, KittyError> {
    let echo = EchoFields::from_command(&command);
    let trimmed_len = payload_b64
        .iter()
        .rposition(|b| *b != b'=')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let decoded = decode_base64(&payload_b64[..trimmed_len])
        .map_err(|_| echo.error(KittyErrorCode::Einval, "invalid base64 payload"))?;
    let data = match command.medium {
        KittyMedium::Direct => decoded,
        KittyMedium::SharedMemory => {
            let name = String::from_utf8(decoded).map_err(|_| {
                echo.error(KittyErrorCode::Ebadf, "invalid shared memory object name")
            })?;
            read_shared_memory_object(&name, &command, &echo)?
        },
        KittyMedium::File | KittyMedium::TempFile => {
            let path_string = String::from_utf8(decoded)
                .map_err(|_| echo.error(KittyErrorCode::Ebadf, "invalid file path"))?;
            let path = std::path::PathBuf::from(path_string);
            let metadata = std::fs::metadata(&path)
                .map_err(|_| echo.error(KittyErrorCode::Ebadf, "could not read file"))?;
            if !metadata.is_file() {
                return Err(echo.error(KittyErrorCode::Ebadf, "could not read file"));
            }
            let read_result = std::fs::File::open(&path)
                .map_err(|_| echo.error(KittyErrorCode::Ebadf, "could not read file"))
                .and_then(|file| {
                    read_range(file, metadata.len(), &command, &echo, "could not read file")
                });
            if command.medium == KittyMedium::TempFile && should_delete_temp_file(&path) {
                let _ = std::fs::remove_file(&path);
            }
            read_result?
        },
    };
    let data = if command.compressed {
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(&data[..])
            .take((MAX_DECODED_BYTES + 1) as u64)
            .read_to_end(&mut inflated)
            .map_err(|_| echo.error(KittyErrorCode::Einval, "could not inflate payload"))?;
        if inflated.len() > MAX_DECODED_BYTES {
            return Err(echo.error(KittyErrorCode::Einval, "image too large"));
        }
        inflated
    } else {
        data
    };
    let image = match command.format {
        KittyFormat::Rgb24 | KittyFormat::Rgba32 => {
            let bpp: u64 = if command.format == KittyFormat::Rgb24 {
                3
            } else {
                4
            };
            let width = command.pixel_width.unwrap_or(0);
            let height = command.pixel_height.unwrap_or(0);
            if width == 0 || height == 0 {
                return Err(echo.error(
                    KittyErrorCode::Einval,
                    "dimensions missing for raw image data",
                ));
            }
            if width > MAX_DIMENSION || height > MAX_DIMENSION {
                return Err(echo.error(KittyErrorCode::Einval, "image dimensions too large"));
            }
            let expected = width as u64 * height as u64 * bpp;
            if expected > MAX_DECODED_BYTES as u64 {
                return Err(echo.error(KittyErrorCode::Einval, "image too large"));
            }
            if (data.len() as u64) < expected {
                return Err(echo.error(
                    KittyErrorCode::Enodata,
                    "insufficient image data for dimensions",
                ));
            }
            let mut data = data;
            data.truncate(expected as usize);
            DecodedImage {
                bytes: data,
                width,
                height,
                format: command.format,
            }
        },
        KittyFormat::Png => decode_png(&data, &echo)?,
    };
    command.image = Some(image);
    Ok(command)
}

fn decode_png(data: &[u8], echo: &EchoFields) -> Result<DecodedImage, KittyError> {
    let limits = png::Limits {
        bytes: MAX_DECODED_BYTES,
    };
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(data), limits);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|_| echo.error(KittyErrorCode::Ebadpng, "invalid png data"))?;
    let width = reader.info().width;
    let height = reader.info().height;
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(echo.error(KittyErrorCode::Einval, "image dimensions too large"));
    }
    if width as u64 * height as u64 * 4 > MAX_DECODED_BYTES as u64 {
        return Err(echo.error(KittyErrorCode::Einval, "image too large"));
    }
    let mut buf = vec![0; reader.output_buffer_size()];
    let out = reader
        .next_frame(&mut buf)
        .map_err(|_| echo.error(KittyErrorCode::Ebadpng, "invalid png data"))?;
    buf.truncate(out.buffer_size());
    let rgba = match out.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(buf.len() / 3 * 4);
            for pixel in buf.as_chunks::<3>().0 {
                rgba.extend_from_slice(pixel);
                rgba.push(255);
            }
            rgba
        },
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity(buf.len() * 4);
            for g in buf {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
            rgba
        },
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity(buf.len() * 2);
            for pixel in buf.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            rgba
        },
        _ => return Err(echo.error(KittyErrorCode::Ebadpng, "invalid png data")),
    };
    if rgba.len() > MAX_DECODED_BYTES {
        return Err(echo.error(KittyErrorCode::Einval, "image too large"));
    }
    Ok(DecodedImage {
        bytes: rgba,
        width: out.width,
        height: out.height,
        format: KittyFormat::Rgba32,
    })
}

#[cfg(test)]
#[path = "./unit/parser_tests.rs"]
mod parser_tests;

#[cfg(test)]
#[path = "./unit/parser_spec_tests.rs"]
mod parser_spec_tests;
