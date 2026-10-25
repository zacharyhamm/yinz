mod translucency;
pub use translucency::TranslucentOverlayBacking;

use crate::panes::Row;
use crate::ui::damage::CellRect;

use crate::panes::Selection;
use crate::{
    panes::kitty_graphics::store::{InternalImageId, KittyImageStore, ScaledImageKey},
    panes::sixel::SixelImageStore,
    panes::terminal_character::{AnsiCode, CharacterStyles, RcCharacterStyles},
    panes::{LinkHandler, PaneId, TerminalCharacter, DEFAULT_STYLES, EMPTY_TERMINAL_CHARACTER},
    ClientId,
};
use std::cell::RefCell;
use std::fmt::Write;
use std::rc::Rc;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    str,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::kitty_placeholders::PLACEHOLDER;
use yinz_types::pane_size::SizeInPixels;
use yinz_types::pane_size::{PaneGeom, Size};
use {
    yinz_types::data::HighlightLayer, yinz_types::data::PaneContents,
    yinz_types::data::PaneRenderReport,
};

fn vte_goto_instruction(x_coords: usize, y_coords: usize, vte_output: &mut String) -> Result<()> {
    write!(
        vte_output,
        "\u{1b}[{};{}H\u{1b}[m",
        y_coords + 1, // + 1 because VTE is 1 indexed
        x_coords + 1,
    )
    .with_context(|| {
        format!(
            "failed to execute VTE instruction to go to ({}, {})",
            x_coords, y_coords
        )
    })
}

fn vte_hide_cursor_instruction(vte_output: &mut String) -> Result<()> {
    write!(vte_output, "\u{1b}[?25l").context("failed to execute VTE instruction to hide cursor")
}

/// A selection region with associated styling for highlights and text selection.
#[derive(Debug, Clone, Copy)]
pub struct HighlightSelection {
    pub selection: Selection,
    pub bg: Option<AnsiCode>,
    pub fg: Option<AnsiCode>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub layer: HighlightLayer,
}

fn adjust_styles_for_possible_selection(
    chunk_selection_and_colors: &[HighlightSelection],
    character_styles: CharacterStyles,
    chunk_y: usize,
    chunk_width: usize,
) -> CharacterStyles {
    chunk_selection_and_colors
        .iter()
        .find(|hs| hs.selection.contains(chunk_y, chunk_width))
        .map(|hs| {
            let mut styles = character_styles;
            if let Some(bg) = hs.bg {
                styles = styles.background(Some(bg));
            }
            if let Some(fg) = hs.fg {
                styles = styles.foreground(Some(fg));
            }
            if hs.bold {
                styles = styles.bold(Some(AnsiCode::On));
            }
            if hs.italic {
                styles = styles.italic(Some(AnsiCode::On));
            }
            if hs.underline {
                styles = styles.underline(Some(AnsiCode::Underline(None)));
            }
            styles
        })
        .unwrap_or(character_styles)
}

fn adjust_styles_for_custom_bg_fg(
    character_styles: CharacterStyles,
    pane_default_fg: Option<AnsiCode>,
    pane_default_bg: Option<AnsiCode>,
) -> CharacterStyles {
    let mut character_styles = character_styles;
    if (character_styles.foreground.is_none()
        || character_styles.foreground == Some(AnsiCode::Reset))
        && let Some(fg) = pane_default_fg
    {
        character_styles.foreground = Some(fg);
    }
    if (character_styles.background.is_none()
        || character_styles.background == Some(AnsiCode::Reset))
        && let Some(bg) = pane_default_bg
    {
        character_styles.background = Some(bg);
    }
    character_styles
}

fn write_changed_styles(
    character_styles: &mut CharacterStyles,
    current_character_styles: CharacterStyles,
    chunk_changed_colors: Option<[Option<AnsiCode>; 256]>,
    link_handler: Option<&std::cell::Ref<LinkHandler>>,
    osc8_hyperlinks: bool,
    vte_output: &mut String,
) -> Result<()> {
    let err_context = "failed to format changed styles to VTE string";

    if let Some(new_styles) =
        character_styles.update_and_return_diff(&current_character_styles, chunk_changed_colors)
    {
        if osc8_hyperlinks {
            if let Some(osc8_link) =
                link_handler.and_then(|l_h| l_h.output_osc8(new_styles.link_anchor))
            {
                write!(vte_output, "{}{}", new_styles, osc8_link).context(err_context)?;
            } else {
                write!(vte_output, "{}", new_styles).context(err_context)?;
            }
        } else {
            write!(vte_output, "{}", new_styles).context(err_context)?;
        }
    }
    Ok(())
}

fn serialize_chunks_with_newlines(
    character_chunks: Vec<(CharacterChunk, bool, Option<usize>)>,
    link_handler: Option<&mut Rc<RefCell<LinkHandler>>>,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    max_size: Option<Size>,
    mut handoff: Option<&mut yinz_types::agent::theme::AgentThemeHandoff>,
) -> Result<String> {
    let err_context = || "failed to serialize input chunks".to_string();

    let mut vte_output = String::new();
    let link_handler = link_handler.map(|l_h| l_h.borrow());
    let mut character_styles = DEFAULT_STYLES.enable_styled_underlines(styled_underlines);
    let mut padding_cursor = None;
    for (character_chunk, is_canonical, padding_start) in character_chunks {
        if let Some((_, styles)) = padding_cursor.take_if(|(y, _)| *y != character_chunk.y) {
            vte_output.push_str("\x1b8");
            character_styles = styles;
        }
        // Skip chunks that are completely outside the size bounds
        if let Some(size) = max_size {
            if character_chunk.y >= size.rows {
                continue; // Chunk is below visible area
            }
            if character_chunk.x >= size.cols {
                continue; // Chunk starts outside visible area
            }
        }

        let chunk_changed_colors = character_chunk.changed_colors();
        let pane_default_fg = character_chunk.pane_default_fg;
        let pane_default_bg = character_chunk.pane_default_bg;
        if let Some(handoff) = handoff.as_deref_mut() {
            handoff.associate(
                vte_output.len(),
                character_chunk
                    .agent_theme
                    .as_ref()
                    .and_then(|p| p.snapshot())
                    .as_deref(),
            );
        }
        if is_canonical || vte_output.is_empty() {
            vte_output.push_str("\n\r");
            if handoff.is_some() {
                // A budget cut starts at a canonical row. Make its styles independent
                // of the discarded prefix, just as its palette association is.
                vte_output.push_str("\x1b[0m");
                character_styles = DEFAULT_STYLES.enable_styled_underlines(styled_underlines);
            }
        }
        let mut chunk_width = character_chunk.x;
        for t_character in character_chunk.terminal_characters.iter() {
            // Stop rendering if the next character would exceed max_size.cols
            if let Some(size) = max_size
                && chunk_width + t_character.width() > size.cols
            {
                break; // Stop rendering this chunk
            }

            if padding_start == Some(chunk_width) {
                // Paint implicit cells without advancing the continuation cursor or
                // changing its styles. DECSC/DECRC also preserve pending wrap.
                vte_output.push_str("\x1b7");
                padding_cursor = Some((character_chunk.y, character_styles));
            }

            let current_character_styles = adjust_styles_for_custom_bg_fg(
                adjust_styles_for_possible_selection(
                    character_chunk.selection_and_colors(),
                    if handoff.is_some() {
                        *t_character.styles
                    } else {
                        character_chunk.application_styles(*t_character.styles)
                    },
                    character_chunk.y,
                    chunk_width,
                ),
                pane_default_fg,
                pane_default_bg,
            );
            write_changed_styles(
                &mut character_styles,
                current_character_styles,
                chunk_changed_colors,
                link_handler.as_ref(),
                osc8_hyperlinks,
                &mut vte_output,
            )
            .with_context(err_context)?;
            chunk_width += t_character.width();
            vte_output.push(t_character.character);
            if t_character.is_implicit_blank() {
                // Printing handles wrapping at the replay width; the marker only restores
                // provenance, leaving the cursor and styles untouched.
                vte_output.push_str("\x1b]777;yinz;implicit-blank\x1b\\");
            }
        }
    }
    if padding_cursor.is_some() {
        vte_output.push_str("\x1b8");
    }
    Ok(vte_output)
}
#[allow(clippy::too_many_arguments)]
fn serialize_chunks(
    vte_output: &mut String,
    character_chunks: Vec<CharacterChunk>,
    sixel_chunks: Option<&Vec<SixelImageChunk>>,
    link_handler: Option<&Rc<RefCell<LinkHandler>>>,
    sixel_image_store: Option<&mut SixelImageStore>,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    max_size: Option<Size>,
    kitty_input: Option<KittyFrameInput>,
) -> Result<()> {
    let err_context = || "failed to serialize input chunks".to_string();

    let mut sixel_vte: Option<String> = None;
    // A unicode placeholder is grid text that only means anything beside the image bytes that
    // accompany it. `kitty_input` is `None` in exactly the cases where those bytes are being
    // withheld - the client's host terminal cannot show them, or the client is a watcher - so the
    // cells would land on a terminal that renders U+10EEEE as garbage, which is what a mosh
    // client sees. Blank them instead; the placeholder is width 1, so the row still lines up.
    let suppress_kitty_placeholders = kitty_input.is_none();
    let link_handler = link_handler.map(|handler| handler.borrow());
    for character_chunk in character_chunks {
        // Skip chunks that are completely outside the size bounds
        if let Some(size) = max_size {
            if character_chunk.y >= size.rows {
                continue; // Chunk is below visible area
            }
            if character_chunk.x >= size.cols {
                continue; // Chunk starts outside visible area
            }
        }

        let chunk_changed_colors = character_chunk.changed_colors();
        let pane_default_fg = character_chunk.pane_default_fg;
        let pane_default_bg = character_chunk.pane_default_bg;
        let mut character_styles = DEFAULT_STYLES.enable_styled_underlines(styled_underlines);
        vte_goto_instruction(character_chunk.x, character_chunk.y, vte_output)
            .with_context(err_context)?;
        let mut chunk_width = character_chunk.x;
        // Selections vary by column; everything else that adjusts a cell's style is fixed
        // for the chunk. Without them, a cell sharing the previous cell's styles needs no SGR.
        let has_selections = !character_chunk.selection_and_colors().is_empty();
        let mut previous_styles: Option<&RcCharacterStyles> = None;
        for t_character in character_chunk.terminal_characters.iter() {
            // Stop rendering if the next character would exceed max_size.cols
            if let Some(size) = max_size
                && chunk_width + t_character.width() > size.cols
            {
                break; // Stop rendering this chunk
            }

            let styles_unchanged = !has_selections
                && previous_styles.is_some_and(|previous| previous.ptr_eq(&t_character.styles));
            if !styles_unchanged {
                let current_character_styles = adjust_styles_for_custom_bg_fg(
                    adjust_styles_for_possible_selection(
                        character_chunk.selection_and_colors(),
                        character_chunk.application_styles(*t_character.styles),
                        character_chunk.y,
                        chunk_width,
                    ),
                    pane_default_fg,
                    pane_default_bg,
                );
                write_changed_styles(
                    &mut character_styles,
                    current_character_styles,
                    chunk_changed_colors,
                    link_handler.as_ref(),
                    osc8_hyperlinks,
                    vte_output,
                )
                .with_context(err_context)?;
            }
            previous_styles = Some(&t_character.styles);
            chunk_width += t_character.width();
            if suppress_kitty_placeholders && t_character.character == PLACEHOLDER {
                vte_output.push(' ');
            } else {
                vte_output.push(t_character.character);
            }
        }
    }
    if let Some(sixel_image_store) = sixel_image_store
        && let Some(sixel_chunks) = sixel_chunks
    {
        for sixel_chunk in sixel_chunks {
            // Skip sixel chunks that are completely outside the size bounds
            if let Some(size) = max_size {
                if sixel_chunk.cell_y >= size.rows {
                    continue; // Sixel chunk is below visible area
                }
                if sixel_chunk.cell_x >= size.cols {
                    continue; // Sixel chunk starts outside visible area
                }
            }

            let serialized_sixel_image = sixel_image_store.serialize_image(
                sixel_chunk.sixel_image_id,
                sixel_chunk.sixel_image_pixel_x,
                sixel_chunk.sixel_image_pixel_y,
                sixel_chunk.sixel_image_pixel_width,
                sixel_chunk.sixel_image_pixel_height,
            );
            if let Some(serialized_sixel_image) = serialized_sixel_image {
                let sixel_vte = sixel_vte.get_or_insert_with(Default::default);
                vte_goto_instruction(sixel_chunk.cell_x, sixel_chunk.cell_y, sixel_vte)
                    .with_context(err_context)?;
                sixel_vte.push_str(&serialized_sixel_image);
            }
        }
    }
    if let Some(ref sixel_vte) = sixel_vte {
        // we do this at the end because of the implied z-index,
        // images should be above text unless the text was explicitly inserted after them (the
        // latter being a case we handle in our own internal state and not in the output)
        let save_cursor_position = "\u{1b}[s";
        let restore_cursor_position = "\u{1b}[u";
        vte_output.push_str(save_cursor_position);
        vte_output.push_str(sixel_vte);
        vte_output.push_str(restore_cursor_position);
    }
    if let Some(kitty_input) = kitty_input {
        // Written in place, since a transmission is megabytes; withdrawn if there was nothing.
        let before = vte_output.len();
        vte_output.push_str("\u{1b}[s");
        let commands_start = vte_output.len();
        serialize_kitty_frame(kitty_input, vte_output).with_context(err_context)?;
        if vte_output.len() == commands_start {
            vte_output.truncate(before);
        } else {
            vte_output.push_str("\u{1b}[u");
        }
    }
    Ok(())
}

type AbsoluteMiddleStart = usize;
type AbsoluteMiddleEnd = usize;
type PadLeftEndBy = usize;
type PadRightStartBy = usize;
fn adjust_middle_segment_for_wide_chars(
    middle_start: usize,
    middle_end: usize,
    terminal_characters: &[TerminalCharacter],
) -> Result<(
    AbsoluteMiddleStart,
    AbsoluteMiddleEnd,
    PadLeftEndBy,
    PadRightStartBy,
)> {
    let err_context = || {
        format!(
            "failed to adjust middle segment (from {} to {}) for wide chars: '{:?}'",
            middle_start, middle_end, terminal_characters
        )
    };

    // a boundary at column 0 is already reached before the first character - without
    // this the loop treats the first character as straddling it and pads over the cut
    let mut absolute_middle_start_index = if middle_start == 0 { Some(0) } else { None };
    let mut absolute_middle_end_index = if middle_end == 0 { Some(0) } else { None };
    let mut current_x = 0;
    let mut pad_left_end_by = 0;
    let mut pad_right_start_by = 0;
    for (absolute_index, t_character) in terminal_characters.iter().enumerate() {
        current_x += t_character.width();
        if current_x >= middle_start && absolute_middle_start_index.is_none() {
            if current_x > middle_start {
                pad_left_end_by = current_x - middle_start;
                absolute_middle_start_index = Some(absolute_index);
            } else {
                absolute_middle_start_index = Some(absolute_index + 1);
            }
        }
        if current_x >= middle_end && absolute_middle_end_index.is_none() {
            absolute_middle_end_index = Some(absolute_index + 1);
            if current_x > middle_end {
                pad_right_start_by = current_x - middle_end;
            }
        }
    }
    Ok((
        absolute_middle_start_index.with_context(err_context)?,
        absolute_middle_end_index.with_context(err_context)?,
        pad_left_end_by,
        pad_right_start_by,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KittyChunkKey {
    pub pane_id: PaneId,
    pub placement_uid: u64,
    pub sub_index: u32,
}

/// An image as the host holds it: the internal image and the crop/size variant it was transmitted as.
pub type HostImageKey = (InternalImageId, Option<ScaledImageKey>);

#[derive(Debug, Clone)]
pub struct HostPlacementRecord {
    pub host_image_id: u32,
    pub host_placement_id: u32,
    pub image_key: HostImageKey,
    pub geometry: KittyImageChunk,
}

#[derive(Debug, Clone)]
pub struct HostImageRecord {
    pub host_image_id: u32,
    pub raster_revision: u64,
}

#[derive(Debug, Clone)]
pub struct HostKittyState {
    pub transmitted: HashMap<HostImageKey, HostImageRecord>,
    pub live_placements: HashMap<KittyChunkKey, HostPlacementRecord>,
    next_host_image_id: u32,
    next_host_placement_id: u32,
}

impl Default for HostKittyState {
    fn default() -> Self {
        HostKittyState {
            transmitted: HashMap::new(),
            live_placements: HashMap::new(),
            next_host_image_id: 2_000_000_000,
            next_host_placement_id: 1,
        }
    }
}

pub struct KittyFrameInput<'a> {
    pub chunks_by_pane: HashMap<PaneId, Vec<KittyImageChunk>>,
    pub rendered_panes: HashSet<PaneId>,
    pub visible_panes: Option<&'a HashSet<PaneId>>,
    pub kitty_image_store: &'a mut KittyImageStore,
    pub host_state: &'a mut HostKittyState,
    pub host_display_cleared: bool,
}

fn pane_sort_key(pane_id: PaneId) -> (u8, u32) {
    match pane_id {
        PaneId::Terminal(id) => (0, id.get()),
        PaneId::Plugin(id) => (1, id.get()),
    }
}

fn emit_kitty_transmit(
    out: &mut String,
    host_image_id: u32,
    width: usize,
    height: usize,
    b64: &str,
) -> Result<()> {
    let err_context = "failed to serialize kitty transmit";
    // An image is megabytes of base64: size the buffer once and copy the payload in directly,
    // rather than growing it chunk by chunk through the formatter.
    const CHUNK: usize = 4096;
    const CHUNK_OVERHEAD: usize = 64;
    let chunk_count = b64.len().div_ceil(CHUNK).max(1);
    out.reserve(b64.len() + chunk_count * CHUNK_OVERHEAD);
    let last = chunk_count - 1;
    for part_index in 0..chunk_count {
        let start = part_index * CHUNK;
        let part = &b64[start..b64.len().min(start + CHUNK)];
        let more = if part_index == last { 0 } else { 1 };
        if part_index == 0 {
            write!(
                out,
                "\u{1b}_Ga=t,q=2,f=32,o=z,t=d,i={},s={},v={},m={};",
                host_image_id, width, height, more
            )
            .context(err_context)?;
        } else {
            write!(out, "\u{1b}_Gq=2,m={};", more).context(err_context)?;
        }
        out.push_str(part);
        out.push_str("\u{1b}\\");
    }
    Ok(())
}

/// Appends the frame's Kitty commands to `out`.
fn serialize_kitty_frame(kitty_input: KittyFrameInput, out: &mut String) -> Result<()> {
    let err_context = "failed to serialize kitty frame";
    let KittyFrameInput {
        mut chunks_by_pane,
        rendered_panes,
        visible_panes,
        kitty_image_store,
        host_state,
        host_display_cleared,
    } = kitty_input;
    if host_display_cleared {
        host_state.live_placements.clear();
    }
    let mut freed: Vec<(HostImageKey, u32)> = host_state
        .transmitted
        .iter()
        // A full repaint may erase placements without freeing host image data. Delete our
        // old host IDs before forgetting them and transmitting replacement IDs.
        .filter(|(image_key, record)| {
            host_display_cleared
                || kitty_image_store.raster_revision(image_key.0) != Some(record.raster_revision)
                || image_key.1.is_some_and(|variant| {
                    kitty_image_store
                        .scaled_variant(image_key.0, variant)
                        .is_none()
                })
        })
        .map(|(image_key, record)| (*image_key, record.host_image_id))
        .collect();
    freed.sort_by_key(|(_, host_id)| *host_id);
    for (image_key, host_id) in freed {
        write!(out, "\u{1b}_Ga=d,q=2,d=I,i={}\u{1b}\\", host_id).context(err_context)?;
        host_state.transmitted.remove(&image_key);
        host_state
            .live_placements
            .retain(|_, record| record.image_key != image_key);
    }
    if let Some(visible_panes) = visible_panes {
        let mut to_delete: Vec<(KittyChunkKey, u32, u32)> = host_state
            .live_placements
            .iter()
            .filter(|(key, _)| !visible_panes.contains(&key.pane_id))
            .map(|(key, record)| (key.clone(), record.host_image_id, record.host_placement_id))
            .collect();
        to_delete.sort_by_key(|(key, _, _)| {
            (pane_sort_key(key.pane_id), key.placement_uid, key.sub_index)
        });
        for (key, host_image_id, host_placement_id) in to_delete {
            write!(
                out,
                "\u{1b}_Ga=d,q=2,d=i,i={},p={}\u{1b}\\",
                host_image_id, host_placement_id
            )
            .context(err_context)?;
            host_state.live_placements.remove(&key);
        }
    }
    let mut rendered: Vec<PaneId> = rendered_panes.into_iter().collect();
    rendered.sort_by_key(|pane_id| pane_sort_key(*pane_id));
    for pane_id in rendered {
        let chunks = chunks_by_pane.remove(&pane_id).unwrap_or_default();
        let mut by_uid: BTreeMap<u64, Vec<KittyImageChunk>> = BTreeMap::new();
        for chunk in chunks {
            by_uid.entry(chunk.placement_uid).or_default().push(chunk);
        }
        let mut current: BTreeMap<KittyChunkKey, KittyImageChunk> = BTreeMap::new();
        for (placement_uid, mut group) in by_uid {
            group.sort_by_key(|chunk| (chunk.cell_y, chunk.cell_x));
            for (sub_index, chunk) in group.into_iter().enumerate() {
                current.insert(
                    KittyChunkKey {
                        pane_id,
                        placement_uid,
                        sub_index: sub_index as u32,
                    },
                    chunk,
                );
            }
        }
        let mut stale: Vec<(KittyChunkKey, u32, u32)> = host_state
            .live_placements
            .iter()
            .filter(|(key, _)| key.pane_id == pane_id && !current.contains_key(key))
            .map(|(key, record)| (key.clone(), record.host_image_id, record.host_placement_id))
            .collect();
        stale.sort_by_key(|(key, _, _)| (key.placement_uid, key.sub_index));
        for (key, host_image_id, host_placement_id) in stale {
            write!(
                out,
                "\u{1b}_Ga=d,q=2,d=i,i={},p={}\u{1b}\\",
                host_image_id, host_placement_id
            )
            .context(err_context)?;
            host_state.live_placements.remove(&key);
        }
        for (key, chunk) in current {
            let variant = if chunk.scaled_px.is_some() {
                Some(chunk.variant_key)
            } else {
                None
            };
            let image_key = (chunk.internal_image_id, variant);
            let host_image_id = match host_state.transmitted.get(&image_key) {
                Some(record) => record.host_image_id,
                None => {
                    let raster_dims = match variant {
                        Some(variant) => kitty_image_store
                            .ensure_scaled_variant(chunk.internal_image_id, variant)
                            .then_some(variant.size),
                        None => kitty_image_store
                            .get(chunk.internal_image_id)
                            .map(|image| (image.width as usize, image.height as usize)),
                    };
                    let (width, height) = match raster_dims {
                        Some(dims) => dims,
                        None => continue,
                    };
                    let b64 = match kitty_image_store
                        .zlib_base64_for(chunk.internal_image_id, variant)
                        .context("failed to compress kitty image")?
                    {
                        Some(b64) => b64,
                        None => continue,
                    };
                    let host_image_id = host_state.next_host_image_id;
                    host_state.next_host_image_id += 1;
                    emit_kitty_transmit(out, host_image_id, width, height, &b64)?;
                    host_state.transmitted.insert(
                        image_key,
                        HostImageRecord {
                            host_image_id,
                            raster_revision: kitty_image_store
                                .raster_revision(chunk.internal_image_id)
                                .context("transmitted kitty image is missing")?,
                        },
                    );
                    host_image_id
                },
            };
            let existing = host_state.live_placements.get(&key).map(|record| {
                (
                    record.host_placement_id,
                    record.geometry,
                    record.host_image_id,
                )
            });
            let host_placement_id = match existing {
                Some((_, geometry, existing_host_image_id))
                    if geometry == chunk && existing_host_image_id == host_image_id =>
                {
                    continue;
                },
                Some((host_placement_id, _, existing_host_image_id)) => {
                    if existing_host_image_id != host_image_id {
                        write!(
                            out,
                            "\u{1b}_Ga=d,q=2,d=i,i={},p={}\u{1b}\\",
                            existing_host_image_id, host_placement_id
                        )
                        .context(err_context)?;
                    }
                    host_placement_id
                },
                None => {
                    let host_placement_id = host_state.next_host_placement_id;
                    host_state.next_host_placement_id += 1;
                    host_placement_id
                },
            };
            vte_goto_instruction(chunk.cell_x, chunk.cell_y, out).context(err_context)?;
            write!(
                out,
                "\u{1b}_Ga=p,q=2,i={},p={},x={},y={},w={},h={},X={},Y={},z={},C=1\u{1b}\\",
                host_image_id,
                host_placement_id,
                chunk.source_px_x,
                chunk.source_px_y,
                chunk.source_px_width,
                chunk.source_px_height,
                chunk.cell_offset_x,
                chunk.cell_offset_y,
                chunk.z_index
            )
            .context(err_context)?;
            host_state.live_placements.insert(
                key,
                HostPlacementRecord {
                    host_image_id,
                    host_placement_id,
                    image_key,
                    geometry: chunk,
                },
            );
        }
    }
    Ok(())
}

/// Shared by `Output::serialize_with_size`, which acts on the answer, and
/// `Output::cursor_is_cropped_for`, which is how the caller asks for it - one expression, so the
/// two cannot drift.
fn cursor_is_cropped(cursor_coordinates: Option<(usize, usize)>, max_size: Size) -> bool {
    cursor_coordinates
        .is_some_and(|(cursor_x, cursor_y)| cursor_y >= max_size.rows || cursor_x >= max_size.cols)
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    pre_vte_instructions: HashMap<ClientId, Vec<String>>,
    post_vte_instructions: HashMap<ClientId, Vec<String>>,
    client_character_chunks: HashMap<ClientId, Vec<CharacterChunk>>,
    compositions: HashMap<ClientId, translucency::Composition>,
    sixel_chunks: HashMap<ClientId, Vec<SixelImageChunk>>,
    client_kitty_chunks: HashMap<ClientId, HashMap<PaneId, Vec<KittyImageChunk>>>,
    client_rendered_kitty_panes: HashMap<ClientId, HashSet<PaneId>>,
    client_kitty_visible_panes: HashMap<ClientId, HashSet<PaneId>>,
    clients_with_cleared_host_display: HashSet<ClientId>,
    client_link_handlers: HashMap<ClientId, Rc<RefCell<LinkHandler>>>,
    sixel_image_store: Rc<RefCell<SixelImageStore>>,
    kitty_image_store: Rc<RefCell<KittyImageStore>>,
    kitty_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    kitty_host_state: Rc<RefCell<HashMap<ClientId, HostKittyState>>>,
    sixel_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    floating_panes_stack: Option<FloatingPanesStack>,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    pane_render_report: PaneRenderReport,
    /// Record consumed output only for the client the watchers follow.
    pub pane_change_client: Option<ClientId>,
    pub rendered_pane_contents: HashSet<PaneId>,
    pub rendered_pane_frames: HashSet<PaneId>,
    pub rendered_pane_boundaries: bool,
    pub rendered_margin_damage: Vec<CellRect>,
    // per client: a smaller client (eg. a watcher) must be cropped against its own cursor, not
    // against whichever client happened to render last
    cursor_coordinates: HashMap<ClientId, (usize, usize)>,
    clients_with_hidden_cursor: HashSet<ClientId>,
    /// Whether each client's host terminal presents this frame atomically (DECSET 2026). Such a
    /// host never displays anything between the frame's first and last byte, so the cursor cannot
    /// be seen following the chunks as they paint and does not have to be hidden while they do.
    /// A client absent from the map has not said, and is hidden the old way.
    synchronized_output_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    /// One answer for every client in this output, when the map above cannot give the right one.
    /// The watchers' frame is keyed by the *followed* client's id but is delivered to the
    /// watchers' terminals, so what that client's host can do says nothing about whether these
    /// bytes may leave the cursor up while they paint.
    atomic_frames_override: Option<bool>,
    // what the session's toasts paint over this frame. they are not panes and so are not in
    // `floating_panes_stack`, but they are drawn above everything and must occlude the cursor the
    // same way a floating pane does
    notification_rects: HashMap<ClientId, Vec<CellRect>>,
    // What each client's own context menu paints over this frame.
    context_menu_rects: HashMap<ClientId, Vec<CellRect>>,
    // A native input overlay owns the cursor even outside its painted rectangle.
    input_overlay_clients: HashSet<ClientId>,
}

impl Output {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        kitty_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
        kitty_host_state: Rc<RefCell<HashMap<ClientId, HostKittyState>>>,
        sixel_host_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
        synchronized_output_capabilities: Rc<RefCell<HashMap<ClientId, bool>>>,
    ) -> Self {
        Output {
            sixel_image_store,
            character_cell_size,
            styled_underlines,
            osc8_hyperlinks,
            kitty_image_store,
            kitty_host_capabilities,
            kitty_host_state,
            sixel_host_capabilities,
            synchronized_output_capabilities,
            ..Default::default()
        }
    }

    pub fn add_clients(
        &mut self,
        client_ids: &HashSet<ClientId>,
        link_handler: Rc<RefCell<LinkHandler>>,
        floating_panes_stack: Option<FloatingPanesStack>,
    ) {
        self.floating_panes_stack = floating_panes_stack;
        for client_id in client_ids {
            self.client_link_handlers
                .insert(*client_id, link_handler.clone());
            self.client_character_chunks.insert(*client_id, vec![]);
        }
    }
    /// The cursor is only positioned once all panes contributed to the frame, so this can be
    /// reached from more than one place - hide each client's cursor at most once per frame.
    ///
    /// A client whose host presents the frame atomically is skipped entirely: nothing between the
    /// frame's first and last byte is ever displayed there, so the cursor is never seen trailing
    /// the chunks, and the hide/show pair it would need to undo costs a `?25h` and a cursor-shape
    /// CSI on every frame - which restarts the host's blink phase. It is deliberately *not*
    /// recorded in `clients_with_hidden_cursor`, because that set is what tells the cursor pass
    /// whether it has a hide to undo.
    pub fn hide_cursor_for_clients(&mut self, client_ids: impl Iterator<Item = ClientId>) {
        let needing_a_hide: Vec<ClientId> = client_ids
            .filter(|client_id| !self.host_presents_frames_atomically(*client_id))
            .collect();
        let client_ids: Vec<ClientId> = needing_a_hide
            .into_iter()
            .filter(|client_id| self.clients_with_hidden_cursor.insert(*client_id))
            .collect();
        for client_id in client_ids {
            self.add_pre_vte_instruction_to_client(client_id, "\u{1b}[?25l");
        }
    }

    /// Whether this client's host terminal shows a whole frame at once (DECSET 2026). Unknown
    /// clients answer `false`, so a client that has not reported keeps the old behaviour.
    pub fn host_presents_frames_atomically(&self, client_id: ClientId) -> bool {
        if let Some(atomic_frames_override) = self.atomic_frames_override {
            return atomic_frames_override;
        }
        self.synchronized_output_capabilities
            .borrow()
            .get(&client_id)
            .copied()
            .unwrap_or(false)
    }

    /// Answer `host_presents_frames_atomically` with this, for every client, whatever the shared
    /// map says. For an output whose client ids are not the ids of the terminals it is going to -
    /// the watchers' frame, which carries the followed client's id.
    pub fn set_host_presents_frames_atomically(&mut self, atomic: bool) {
        self.atomic_frames_override = Some(atomic);
    }

    /// Whether this frame hid `client_id`'s cursor, and so owes it a `?25h`.
    pub fn cursor_was_hidden_for(&self, client_id: ClientId) -> bool {
        self.clients_with_hidden_cursor.contains(&client_id)
    }

    /// Whether anything in this frame moved `client_id`'s hardware cursor, and so obliges the
    /// cursor pass to put it back.
    ///
    /// Chunks do, obviously. So do the post-VTE instructions, which are full of absolute gotos -
    /// a pane's raw VTE passthrough, another client's fake cursor, a repainted frame, a terminal
    /// title. `pre_vte_instructions` is deliberately excluded: the only thing in there is this
    /// pass's own cursor hide and the display clear, and counting the hide is what made
    /// `is_dirty()` unconditionally true here and the cursor memo dead.
    pub fn frame_moved_cursor_for(&self, client_id: ClientId) -> bool {
        self.post_vte_instructions
            .get(&client_id)
            .is_some_and(|instructions| !instructions.is_empty())
            || self
                .client_character_chunks
                .get(&client_id)
                .is_some_and(|chunks| !chunks.is_empty())
            || self
                .sixel_chunks
                .get(&client_id)
                .is_some_and(|chunks| !chunks.is_empty())
            || self
                .client_kitty_chunks
                .get(&client_id)
                .is_some_and(|by_pane| by_pane.values().any(|chunks| !chunks.is_empty()))
    }
    /// `add_clients` sets the stack of whichever tab is rendering, so a later pass over the tabs
    /// (eg. positioning the cursor) needs to point the output back at its own tab's stack.
    /// Swap in a new occlusion stack, returning the one it replaced so callers that need to emit
    /// unclipped chunks (eg. notifications) can put it back afterwards.
    pub fn set_floating_panes_stack(
        &mut self,
        floating_panes_stack: Option<FloatingPanesStack>,
    ) -> Option<FloatingPanesStack> {
        std::mem::replace(&mut self.floating_panes_stack, floating_panes_stack)
    }
    /// Start a frame's rectangle-limited text compositor for these clients. The ordinary
    /// occlusion stack remains responsible for cursor and graphics clipping.
    pub fn begin_translucent_composition(
        &mut self,
        clients: &HashSet<ClientId>,
        overlays: Vec<(usize, CellRect, u8)>,
        foreground: yinz_types::colors::PaletteColor,
        background: yinz_types::colors::PaletteColor,
        tint: yinz_types::colors::PaletteColor,
        terminal_emulator_color_codes: &HashMap<usize, String>,
    ) {
        if overlays.is_empty() {
            return;
        }
        let host_colors = translucency::host_colors(terminal_emulator_color_codes);
        let composition =
            translucency::Composition::new(overlays, foreground, background, tint, host_colors);
        for client in clients {
            self.compositions.insert(*client, composition.clone());
        }
    }
    pub fn is_translucent_layer(&self, client: ClientId, z_index: Option<usize>) -> bool {
        self.compositions
            .get(&client)
            .is_some_and(|composition| composition.contains_layer(z_index.unwrap_or(0)))
    }
    /// Keep background text out of the visible cursor's cell in a translucent pane.
    pub fn protect_translucent_cursor_cell(
        &mut self,
        client: ClientId,
        x: usize,
        y: usize,
        z_index: Option<usize>,
    ) {
        if let Some(composition) = self.compositions.get_mut(&client) {
            // Cursor indices address the occlusion stack; text layer zero is the tiled panes.
            composition.protect_cursor_cell(x, y, z_index.map(|z| z + 1).unwrap_or(0));
        }
    }

    /// Flush before screen menus and notifications contribute their opaque cells.
    pub fn finish_translucent_composition(&mut self) {
        for (client, composition) in self.compositions.drain() {
            if let Some(chunks) = self.client_character_chunks.get_mut(&client) {
                chunks.extend(composition.finish());
            }
        }
    }

    /// Compose a screen overlay over this client's already-composed panes and notifications.
    /// The caller repaints intersecting panes when `backing.prepare()` requests it.
    pub fn add_translucent_overlay(
        &mut self,
        client_id: ClientId,
        rect: CellRect,
        opacity: u8,
        overlay_chunks: Vec<CharacterChunk>,
        cursor: Option<(usize, usize)>,
        palette: yinz_types::style::Palette,
        tint: yinz_types::colors::PaletteColor,
        terminal_emulator_color_codes: &HashMap<usize, String>,
        backing: &mut TranslucentOverlayBacking,
    ) -> Result<()> {
        let display_cleared = self.host_display_cleared_for_client(client_id);
        let Some(chunks) = self.client_character_chunks.get_mut(&client_id) else {
            return Ok(());
        };
        backing.prepare(rect, opacity, palette, tint, terminal_emulator_color_codes);
        let mut composition = backing.update(chunks, display_cleared)?;
        composition.add(&overlay_chunks, 1);
        if let Some((x, y)) = cursor {
            composition.protect_cursor_cell(x, y, 1);
        }
        let mut outside = vec![];
        // Clip one at a time to preserve paint order where notifications overlap pane chunks.
        for chunk in std::mem::take(chunks) {
            outside.extend(
                composition
                    .outside_chunks(vec![chunk], 0)
                    .context("failed to clip text under screen overlay")?,
            );
        }
        outside.extend(composition.finish());
        *chunks = outside;

        let clip = FloatingPanesStack {
            layers: vec![PaneGeom {
                x: rect.x,
                y: rect.y,
                cols: yinz_types::pane_size::Dimension::fixed(rect.cols),
                rows: yinz_types::pane_size::Dimension::fixed(rect.rows),
                ..Default::default()
            }],
        };
        if let Some(cell_size) = *self.character_cell_size.borrow() {
            if let Some(chunks) = self.sixel_chunks.get_mut(&client_id) {
                *chunks = clip.visible_sixel_image_chunks(std::mem::take(chunks), None, &cell_size);
            }
            if let Some(panes) = self.client_kitty_chunks.get_mut(&client_id) {
                for chunks in panes.values_mut() {
                    *chunks =
                        clip.visible_kitty_image_chunks(std::mem::take(chunks), None, &cell_size);
                }
            }
        }
        Ok(())
    }

    pub fn set_input_overlay_clients(&mut self, clients: impl Iterator<Item = ClientId>) {
        self.input_overlay_clients = clients.collect();
    }
    pub fn add_character_chunks_to_client(
        &mut self,
        client_id: ClientId,
        mut character_chunks: Vec<CharacterChunk>,
        z_index: Option<usize>,
    ) -> Result<()> {
        if let Some(composition) = self.compositions.get_mut(&client_id) {
            composition.add(&character_chunks, z_index.unwrap_or(0));
            character_chunks =
                composition.outside_chunks(character_chunks, z_index.unwrap_or(0))?;
        }
        if let Some(client_character_chunks) = self.client_character_chunks.get_mut(&client_id) {
            if let Some(floating_panes_stack) = &self.floating_panes_stack {
                let mut visible_character_chunks = floating_panes_stack
                    .visible_character_chunks(character_chunks, z_index)
                    .with_context(|| {
                        format!("failed to add character chunks for client {}", client_id)
                    })?;
                client_character_chunks.append(&mut visible_character_chunks);
            } else {
                client_character_chunks.append(&mut character_chunks);
            }
        }
        Ok(())
    }
    pub fn add_character_chunks_to_multiple_clients(
        &mut self,
        mut character_chunks: Vec<CharacterChunk>,
        client_ids: impl Iterator<Item = ClientId>,
        z_index: Option<usize>,
    ) -> Result<()> {
        // This runs for every visible pane on every frame. Only the clients before the
        // last get a copy; the last one takes the chunks themselves, so a single-client
        // session never clones a frame.
        let mut client_ids = client_ids.peekable();
        while let Some(client_id) = client_ids.next() {
            let chunks = if client_ids.peek().is_some() {
                character_chunks.clone()
            } else {
                std::mem::take(&mut character_chunks)
            };
            self.add_character_chunks_to_client(client_id, chunks, z_index)
                .context("failed to add character chunks for multiple clients")?;
        }
        Ok(())
    }
    pub fn add_post_vte_instruction_to_multiple_clients(
        &mut self,
        client_ids: impl Iterator<Item = ClientId>,
        vte_instruction: &str,
    ) {
        for client_id in client_ids {
            self.add_post_vte_instruction_to_client(client_id, vte_instruction);
        }
    }
    pub fn add_pre_vte_instruction_to_multiple_clients(
        &mut self,
        client_ids: impl Iterator<Item = ClientId>,
        vte_instruction: &str,
    ) {
        for client_id in client_ids {
            self.add_pre_vte_instruction_to_client(client_id, vte_instruction);
        }
    }
    pub fn mark_host_display_cleared_for_clients(
        &mut self,
        client_ids: impl Iterator<Item = ClientId>,
    ) {
        for client_id in client_ids {
            self.clients_with_cleared_host_display.insert(client_id);
        }
    }
    pub fn mark_host_display_cleared_for_client(&mut self, client_id: ClientId) {
        self.clients_with_cleared_host_display.insert(client_id);
    }
    pub fn host_display_cleared_for_client(&self, client_id: ClientId) -> bool {
        self.clients_with_cleared_host_display.contains(&client_id)
    }
    pub fn any_host_display_cleared(&self) -> bool {
        !self.clients_with_cleared_host_display.is_empty()
    }
    pub fn add_post_vte_instruction_to_client(
        &mut self,
        client_id: ClientId,
        vte_instruction: &str,
    ) {
        // Control-only output needs a client entry even when no panes rendered.
        self.client_character_chunks.entry(client_id).or_default();
        let entry = self.post_vte_instructions.entry(client_id).or_default();
        entry.push(String::from(vte_instruction));
    }
    pub fn add_pre_vte_instruction_to_client(
        &mut self,
        client_id: ClientId,
        vte_instruction: &str,
    ) {
        self.client_character_chunks.entry(client_id).or_default();
        let entry = self.pre_vte_instructions.entry(client_id).or_default();
        entry.push(String::from(vte_instruction));
    }
    pub fn add_sixel_image_chunks_to_client(
        &mut self,
        client_id: ClientId,
        sixel_image_chunks: Vec<SixelImageChunk>,
        z_index: Option<usize>,
    ) {
        if let Some(character_cell_size) = *self.character_cell_size.borrow() {
            let mut sixel_chunks = if let Some(floating_panes_stack) = &self.floating_panes_stack {
                floating_panes_stack.visible_sixel_image_chunks(
                    sixel_image_chunks,
                    z_index,
                    &character_cell_size,
                )
            } else {
                sixel_image_chunks
            };
            let entry = self.sixel_chunks.entry(client_id).or_default();
            entry.append(&mut sixel_chunks);
        }
    }
    pub fn add_sixel_image_chunks_to_multiple_clients(
        &mut self,
        sixel_image_chunks: Vec<SixelImageChunk>,
        client_ids: impl Iterator<Item = ClientId>,
        z_index: Option<usize>,
    ) {
        if let Some(character_cell_size) = *self.character_cell_size.borrow() {
            let sixel_chunks = if let Some(floating_panes_stack) = &self.floating_panes_stack {
                floating_panes_stack.visible_sixel_image_chunks(
                    sixel_image_chunks,
                    z_index,
                    &character_cell_size,
                )
            } else {
                sixel_image_chunks
            };
            let mut sixel_chunks = sixel_chunks;
            let mut client_ids = client_ids.peekable();
            while let Some(client_id) = client_ids.next() {
                let mut chunks = if client_ids.peek().is_some() {
                    sixel_chunks.clone()
                } else {
                    std::mem::take(&mut sixel_chunks)
                };
                let entry = self.sixel_chunks.entry(client_id).or_default();
                entry.append(&mut chunks);
            }
        }
    }
    pub fn add_kitty_image_chunks_to_client(
        &mut self,
        client_id: ClientId,
        pane_id: PaneId,
        kitty_image_chunks: Vec<KittyImageChunk>,
        z_index: Option<usize>,
    ) {
        self.client_rendered_kitty_panes
            .entry(client_id)
            .or_default()
            .insert(pane_id);
        let mut kitty_chunks = match (
            *self.character_cell_size.borrow(),
            &self.floating_panes_stack,
        ) {
            (Some(character_cell_size), Some(floating_panes_stack)) => floating_panes_stack
                .visible_kitty_image_chunks(kitty_image_chunks, z_index, &character_cell_size),
            _ => kitty_image_chunks,
        };
        self.client_kitty_chunks
            .entry(client_id)
            .or_default()
            .entry(pane_id)
            .or_default()
            .append(&mut kitty_chunks);
    }
    pub fn add_kitty_image_chunks_to_multiple_clients(
        &mut self,
        pane_id: PaneId,
        kitty_image_chunks: Vec<KittyImageChunk>,
        client_ids: impl Iterator<Item = ClientId>,
        z_index: Option<usize>,
    ) {
        let kitty_chunks = match (
            *self.character_cell_size.borrow(),
            &self.floating_panes_stack,
        ) {
            (Some(character_cell_size), Some(floating_panes_stack)) => floating_panes_stack
                .visible_kitty_image_chunks(kitty_image_chunks, z_index, &character_cell_size),
            _ => kitty_image_chunks,
        };
        let mut kitty_chunks = kitty_chunks;
        let mut client_ids = client_ids.peekable();
        while let Some(client_id) = client_ids.next() {
            let mut chunks = if client_ids.peek().is_some() {
                kitty_chunks.clone()
            } else {
                std::mem::take(&mut kitty_chunks)
            };
            self.client_rendered_kitty_panes
                .entry(client_id)
                .or_default()
                .insert(pane_id);
            self.client_kitty_chunks
                .entry(client_id)
                .or_default()
                .entry(pane_id)
                .or_default()
                .append(&mut chunks);
        }
    }
    pub fn set_kitty_visible_panes(&mut self, client_id: ClientId, pane_ids: HashSet<PaneId>) {
        self.client_kitty_visible_panes.insert(client_id, pane_ids);
    }
    pub fn serialize(&mut self) -> Result<HashMap<ClientId, String>> {
        let err_context = || "failed to serialize output to clients".to_string();

        let mut serialized_render_instructions = HashMap::new();

        for (client_id, client_character_chunks) in self.client_character_chunks.drain() {
            let mut client_serialized_render_instructions = String::new();

            // append pre-vte instructions for this client
            let host_display_cleared = self.clients_with_cleared_host_display.remove(&client_id);
            if let Some(pre_vte_instructions_for_client) =
                self.pre_vte_instructions.remove(&client_id)
            {
                for vte_instruction in pre_vte_instructions_for_client {
                    client_serialized_render_instructions.push_str(&vte_instruction);
                }
            }

            let kitty_chunks_by_pane = self.client_kitty_chunks.remove(&client_id);
            let kitty_rendered_panes = self.client_rendered_kitty_panes.remove(&client_id);
            let kitty_visible_panes = self.client_kitty_visible_panes.remove(&client_id);
            let client_host_is_kitty_capable = self
                .kitty_host_capabilities
                .borrow()
                .get(&client_id)
                .copied()
                .unwrap_or(false);
            let client_host_is_sixel_capable = self
                .sixel_host_capabilities
                .borrow()
                .get(&client_id)
                .copied()
                .unwrap_or(false);
            let mut kitty_image_store;
            let mut kitty_host_state_map;
            let kitty_input = if client_host_is_kitty_capable {
                kitty_image_store = self.kitty_image_store.borrow_mut();
                kitty_host_state_map = self.kitty_host_state.borrow_mut();
                Some(KittyFrameInput {
                    chunks_by_pane: kitty_chunks_by_pane.unwrap_or_default(),
                    rendered_panes: kitty_rendered_panes.unwrap_or_default(),
                    visible_panes: kitty_visible_panes.as_ref(),
                    kitty_image_store: &mut kitty_image_store,
                    host_state: kitty_host_state_map.entry(client_id).or_default(),
                    host_display_cleared,
                })
            } else {
                None
            };

            // append the actual vte
            let sixel_chunks_for_client = if client_host_is_sixel_capable {
                self.sixel_chunks.get(&client_id)
            } else {
                None
            };
            serialize_chunks(
                &mut client_serialized_render_instructions,
                client_character_chunks,
                sixel_chunks_for_client,
                self.client_link_handlers.get(&client_id),
                Some(&mut self.sixel_image_store.borrow_mut()),
                self.styled_underlines,
                self.osc8_hyperlinks,
                None, // No size constraints for regular rendering
                kitty_input,
            )
            .with_context(err_context)?;

            // append post-vte instructions for this client
            if let Some(post_vte_instructions_for_client) =
                self.post_vte_instructions.remove(&client_id)
            {
                for vte_instruction in post_vte_instructions_for_client {
                    client_serialized_render_instructions.push_str(&vte_instruction);
                }
            }
            if !client_serialized_render_instructions.is_empty() {
                serialized_render_instructions
                    .insert(client_id, client_serialized_render_instructions);
            }
        }
        Ok(serialized_render_instructions)
    }
    pub fn serialize_with_size(
        &mut self,
        max_size: Option<Size>,
    ) -> Result<HashMap<ClientId, String>> {
        let err_context =
            || "failed to serialize output to clients with size constraints".to_string();

        let mut serialized_render_instructions = HashMap::new();

        for (client_id, client_character_chunks) in self.client_character_chunks.drain() {
            let mut client_serialized_render_instructions = String::new();

            // append pre-vte instructions for this client
            if let Some(pre_vte_instructions_for_client) =
                self.pre_vte_instructions.remove(&client_id)
            {
                for vte_instruction in pre_vte_instructions_for_client {
                    client_serialized_render_instructions.push_str(&vte_instruction);
                }
            }

            // append the actual vte with size constraints
            let client_host_is_sixel_capable = self
                .sixel_host_capabilities
                .borrow()
                .get(&client_id)
                .copied()
                .unwrap_or(false);
            let sixel_chunks_for_client = if client_host_is_sixel_capable {
                self.sixel_chunks.get(&client_id)
            } else {
                None
            };
            serialize_chunks(
                &mut client_serialized_render_instructions,
                client_character_chunks,
                sixel_chunks_for_client,
                self.client_link_handlers.get(&client_id),
                Some(&mut self.sixel_image_store.borrow_mut()),
                self.styled_underlines,
                self.osc8_hyperlinks,
                max_size,
                None,
            )
            .with_context(err_context)?;

            // append post-vte instructions for this client
            if let Some(post_vte_instructions_for_client) =
                self.post_vte_instructions.remove(&client_id)
            {
                for vte_instruction in post_vte_instructions_for_client {
                    client_serialized_render_instructions.push_str(&vte_instruction);
                }
            }

            // Check if cursor was cropped and hide it if necessary
            if let Some(max_size) = max_size
                && cursor_is_cropped(self.cursor_coordinates.get(&client_id).copied(), max_size)
            {
                vte_hide_cursor_instruction(&mut client_serialized_render_instructions)
                    .with_context(err_context)?;
            }

            if !client_serialized_render_instructions.is_empty() {
                serialized_render_instructions
                    .insert(client_id, client_serialized_render_instructions);
            }
        }
        Ok(serialized_render_instructions)
    }
    /// Whether `client_id`'s cursor falls outside `max_size`, and so is hidden by
    /// `serialize_with_size` rather than placed.
    ///
    /// That hide reaches one watcher's byte stream only, and the memo of what the host was told
    /// lives in the tab, keyed by the *followed* client - so the caller has to ask, or the frame
    /// that brings the cursor back into range decides it has nothing to say and the watcher's
    /// cursor stays down for good.
    pub fn cursor_is_cropped_for(&self, client_id: ClientId, max_size: Size) -> bool {
        cursor_is_cropped(self.cursor_coordinates.get(&client_id).copied(), max_size)
    }
    pub fn is_dirty(&self) -> bool {
        !self.pre_vte_instructions.is_empty()
            || !self.post_vte_instructions.is_empty()
            || !self.compositions.is_empty()
            || self.client_character_chunks.values().any(|c| !c.is_empty())
            || self.sixel_chunks.values().any(|c| !c.is_empty())
            || self
                .client_kitty_chunks
                .values()
                .any(|chunks_by_pane| chunks_by_pane.values().any(|c| !c.is_empty()))
            || self.has_pending_kitty_host_deletes()
    }
    /// Whether a client in this frame still has a freed image on its host. Only those clients'
    /// deletes are serialized; a client held back from this frame pays its own with the next
    /// frame it is in, and must not make this one dirty for everybody else meanwhile.
    pub fn has_pending_kitty_host_deletes(&self) -> bool {
        let kitty_image_store = self.kitty_image_store.borrow();
        let kitty_host_state = self.kitty_host_state.borrow();
        self.client_character_chunks
            .keys()
            .filter_map(|client_id| kitty_host_state.get(client_id))
            .any(|host_state| {
                host_state
                    .transmitted
                    .keys()
                    .any(|(internal_id, _)| kitty_image_store.get(*internal_id).is_none())
            })
    }
    pub fn has_rendered_assets(&self) -> bool {
        // pre_vte and post_vte are not considered rendered assets as they should not be visible
        !self.compositions.is_empty()
            || self.client_character_chunks.values().any(|c| !c.is_empty())
            || self.sixel_chunks.values().any(|c| !c.is_empty())
            || self
                .client_kitty_chunks
                .values()
                .any(|chunks_by_pane| chunks_by_pane.values().any(|c| !c.is_empty()))
    }
    /// Where `client_id`'s own cursor was placed, so `serialize_with_size` can tell whether it fell
    /// outside *this* client's size and must be hidden. Only the real cursor is recorded - a fake
    /// cursor belongs to another client and must not decide this one's crop.
    pub fn set_cursor_coordinates(
        &mut self,
        client_id: ClientId,
        cursor_x: usize,
        cursor_y: usize,
    ) {
        self.cursor_coordinates
            .insert(client_id, (cursor_x, cursor_y));
    }
    /// Where each client's notifications paint this frame, in absolute session coordinates.
    ///
    /// Set once for the whole frame, before anything renders, so both the real cursor and the fake
    /// cursors standing in for other clients are tested against the same geometry - and so a fake
    /// cursor, which is a post-VTE instruction and would otherwise be drawn *after* the toast's
    /// own cells, is not painted on top of the box.
    pub fn set_notification_rects(&mut self, notification_rects: HashMap<ClientId, Vec<CellRect>>) {
        self.notification_rects = notification_rects;
    }
    pub fn cursor_is_covered_by_notification(
        &self,
        x: usize,
        y: usize,
        client_id: ClientId,
    ) -> bool {
        self.notification_rects
            .get(&client_id)
            .into_iter()
            .flatten()
            .any(|rect| rect.contains(x, y))
    }
    /// Where each client's context menu paints this frame, in absolute session coordinates.
    ///
    /// Set before anything renders, for the reason `set_notification_rects` gives: a fake cursor is
    /// decided while the panes draw and is emitted as a post-VTE instruction, so it would otherwise
    /// be painted over a menu that had not been drawn yet when it was decided.
    pub fn set_context_menu_rects(&mut self, context_menu_rects: HashMap<ClientId, Vec<CellRect>>) {
        self.context_menu_rects = context_menu_rects;
    }
    /// Whether `client_id` may be shown a cursor at this cell - its own, or the fake one standing
    /// in for another client. `client_id` is the client doing the *looking*: a menu is drawn only
    /// for the client that opened it, so only that client's menu can hide anything from them.
    pub fn cursor_is_visible(
        &self,
        cursor_x: usize,
        cursor_y: usize,
        z_index: Option<usize>,
        client_id: ClientId,
    ) -> bool {
        if self.input_overlay_clients.contains(&client_id) {
            return false;
        }
        if self.cursor_is_covered_by_notification(cursor_x, cursor_y, client_id) {
            return false;
        }
        if self
            .context_menu_rects
            .get(&client_id)
            .is_some_and(|rects| rects.iter().any(|rect| rect.contains(cursor_x, cursor_y)))
        {
            return false;
        }
        self.floating_panes_stack
            .as_ref()
            .map(|s| s.cursor_is_visible(cursor_x, cursor_y, z_index))
            .unwrap_or(true)
    }
    pub fn add_pane_contents(
        &mut self,
        client_ids: &[ClientId],
        pane_id: PaneId,
        pane_contents: PaneContents,
    ) {
        self.pane_render_report
            .add_pane_contents(client_ids, pane_id, pane_contents);
    }
    pub fn drain_pane_render_report(&mut self) -> PaneRenderReport {
        let empty_pane_render_report = PaneRenderReport::default();
        std::mem::replace(&mut self.pane_render_report, empty_pane_render_report)
    }
}

// this struct represents the geometry of a group of floating panes
// we use it to filter out CharacterChunks who are behind these geometries
// and so would not be visible. If a chunk is partially covered, it is adjusted
// to include only the non-covered parts
#[derive(Debug, Clone, Default)]
pub struct FloatingPanesStack {
    pub layers: Vec<PaneGeom>,
}

#[derive(Clone, Copy)]
struct ImageChunkGeometry {
    cell_x: usize,
    cell_y: usize,
    source_px_x: usize,
    source_px_y: usize,
    source_px_width: usize,
    source_px_height: usize,
}

impl ImageChunkGeometry {
    fn remove_covered_parts<T>(
        &self,
        pane_geom: &PaneGeom,
        character_cell_size: &SizeInPixels,
        to_chunk: impl Fn(Self) -> T,
    ) -> Vec<T> {
        // Round up to the nearest cell edge. An exact multiple (including an empty image) is
        // kept as is, so a zero cell size never divides an empty source.
        let round_up = |pixels: usize, cell: usize| {
            if pixels.is_multiple_of(cell) {
                pixels
            } else {
                pixels.next_multiple_of(cell)
            }
        };
        let rounded_height = round_up(self.source_px_height, character_cell_size.height);
        let rounded_width = round_up(self.source_px_width, character_cell_size.width);

        let pane_top_edge = pane_geom.y * character_cell_size.height;
        let pane_left_edge = pane_geom.x * character_cell_size.width;
        let pane_bottom_edge = (pane_geom.y + pane_geom.rows.as_usize().saturating_sub(1))
            * character_cell_size.height;
        let pane_right_edge =
            (pane_geom.x + pane_geom.cols.as_usize().saturating_sub(1)) * character_cell_size.width;
        let image_top_edge = self.cell_y * character_cell_size.height;
        let image_bottom_edge = image_top_edge + rounded_height;
        let image_left_edge = self.cell_x * character_cell_size.width;
        let image_right_edge = image_left_edge + rounded_width;

        let mut uncovered_chunks = vec![];
        let pane_covers_chunk_completely = pane_top_edge <= image_top_edge
            && pane_bottom_edge >= image_bottom_edge
            && pane_left_edge <= image_left_edge
            && pane_right_edge >= image_right_edge;
        let pane_intersects_with_chunk_vertically = (pane_left_edge >= image_left_edge
            && pane_left_edge <= image_right_edge)
            || (pane_right_edge >= image_left_edge && pane_right_edge <= image_right_edge)
            || (pane_left_edge <= image_left_edge && pane_right_edge >= image_right_edge);
        let pane_intersects_with_chunk_horizontally = (pane_top_edge >= image_top_edge
            && pane_top_edge <= image_bottom_edge)
            || (pane_bottom_edge >= image_top_edge && pane_bottom_edge <= image_bottom_edge)
            || (pane_top_edge <= image_top_edge && pane_bottom_edge >= image_bottom_edge);
        if pane_covers_chunk_completely {
            return uncovered_chunks;
        }
        if pane_top_edge >= image_top_edge
            && pane_top_edge <= image_bottom_edge
            && pane_intersects_with_chunk_vertically
        {
            let top_image_chunk = Self {
                cell_x: self.cell_x,
                cell_y: self.cell_y,
                source_px_x: self.source_px_x,
                source_px_y: self.source_px_y,
                source_px_width: rounded_width,
                source_px_height: pane_top_edge - image_top_edge,
            };
            uncovered_chunks.push(to_chunk(top_image_chunk));
        }
        if pane_bottom_edge <= image_bottom_edge
            && pane_bottom_edge >= image_top_edge
            && pane_intersects_with_chunk_vertically
        {
            let bottom_image_chunk = Self {
                cell_x: self.cell_x,
                cell_y: (pane_bottom_edge / character_cell_size.height) + 1,
                source_px_x: self.source_px_x,
                source_px_y: self.source_px_y
                    + (pane_bottom_edge - image_top_edge)
                    + character_cell_size.height,
                source_px_width: rounded_width,
                source_px_height: (rounded_height - (pane_bottom_edge - image_top_edge))
                    .saturating_sub(character_cell_size.height),
            };
            uncovered_chunks.push(to_chunk(bottom_image_chunk));
        }
        // Side strips span the rows the pane shares with the image; the top strip, if any, has
        // already taken the rows above the pane. Built only after the horizontal intersection
        // check, which keeps the height subtraction from underflowing.
        let covered_above = pane_top_edge.saturating_sub(image_top_edge);
        let side_strip = |cell_x: usize, source_px_x: usize, source_px_width: usize| Self {
            cell_x,
            cell_y: std::cmp::max(self.cell_y, pane_top_edge / character_cell_size.height),
            source_px_x,
            source_px_y: self.source_px_y + covered_above,
            source_px_width,
            source_px_height: std::cmp::min(
                pane_bottom_edge - std::cmp::max(pane_top_edge, image_top_edge)
                    + character_cell_size.height,
                rounded_height.saturating_sub(covered_above),
            ),
        };
        if pane_left_edge >= image_left_edge
            && pane_left_edge <= image_right_edge
            && pane_intersects_with_chunk_horizontally
        {
            uncovered_chunks.push(to_chunk(side_strip(
                self.cell_x,
                self.source_px_x,
                rounded_width.saturating_sub(image_right_edge.saturating_sub(pane_left_edge)),
            )));
        }
        if pane_right_edge <= image_right_edge
            && pane_right_edge >= image_left_edge
            && pane_intersects_with_chunk_horizontally
        {
            uncovered_chunks.push(to_chunk(side_strip(
                (pane_right_edge / character_cell_size.width) + 1,
                self.source_px_x + (pane_right_edge - image_left_edge) + character_cell_size.width,
                (rounded_width.saturating_sub(pane_right_edge - image_left_edge))
                    .saturating_sub(character_cell_size.width),
            )));
        }
        if uncovered_chunks.is_empty() {
            uncovered_chunks.push(to_chunk(*self));
        }
        uncovered_chunks
    }
}

impl FloatingPanesStack {
    pub fn visible_character_chunks(
        &self,
        mut character_chunks: Vec<CharacterChunk>,
        z_index: Option<usize>,
    ) -> Result<Vec<CharacterChunk>> {
        let err_context = || {
            format!(
                "failed to determine visible character chunks at z-index {:?}",
                z_index
            )
        };

        let z_index = z_index.unwrap_or(0);
        let mut chunks_to_check: Vec<CharacterChunk> = std::mem::take(&mut character_chunks);
        let mut visible_chunks = vec![];
        'chunk_loop: loop {
            match chunks_to_check.pop() {
                Some(mut c_chunk) => {
                    let panes_to_check = self.layers.iter().skip(z_index);
                    for pane_geom in panes_to_check {
                        let new_chunk_to_check = self
                            .remove_covered_parts(pane_geom, &mut c_chunk)
                            .with_context(err_context)?;
                        if let Some(new_chunk_to_check) = new_chunk_to_check {
                            // this happens when the pane covers the middle of the chunk, and so we
                            // end up with an extra chunk we need to check (eg. against panes above
                            // this one)
                            chunks_to_check.push(new_chunk_to_check);
                        }
                        if c_chunk.terminal_characters.is_empty() {
                            continue 'chunk_loop;
                        }
                    }
                    visible_chunks.push(c_chunk);
                },
                None => {
                    break 'chunk_loop;
                },
            }
        }
        Ok(visible_chunks)
    }
    pub fn visible_sixel_image_chunks(
        &self,
        mut sixel_image_chunks: Vec<SixelImageChunk>,
        z_index: Option<usize>,
        character_cell_size: &SizeInPixels,
    ) -> Vec<SixelImageChunk> {
        let z_index = z_index.unwrap_or(0);
        let mut chunks_to_check: Vec<SixelImageChunk> = std::mem::take(&mut sixel_image_chunks);
        let panes_to_check = self.layers.iter().skip(z_index);
        for pane_geom in panes_to_check {
            let chunks_to_check_against_this_pane: Vec<SixelImageChunk> =
                std::mem::take(&mut chunks_to_check);
            for s_chunk in chunks_to_check_against_this_pane {
                let mut uncovered_chunks =
                    self.remove_covered_sixel_parts(pane_geom, &s_chunk, character_cell_size);
                chunks_to_check.append(&mut uncovered_chunks);
            }
        }
        chunks_to_check
    }
    pub fn visible_kitty_image_chunks(
        &self,
        mut kitty_image_chunks: Vec<KittyImageChunk>,
        z_index: Option<usize>,
        character_cell_size: &SizeInPixels,
    ) -> Vec<KittyImageChunk> {
        let z_index = z_index.unwrap_or(0);
        let mut chunks_to_check: Vec<KittyImageChunk> = std::mem::take(&mut kitty_image_chunks);
        let panes_to_check = self.layers.iter().skip(z_index);
        for pane_geom in panes_to_check {
            let chunks_to_check_against_this_pane: Vec<KittyImageChunk> =
                std::mem::take(&mut chunks_to_check);
            for k_chunk in chunks_to_check_against_this_pane {
                let mut uncovered_chunks =
                    self.remove_covered_kitty_parts(pane_geom, &k_chunk, character_cell_size);
                chunks_to_check.append(&mut uncovered_chunks);
            }
        }
        chunks_to_check
    }
    fn remove_covered_kitty_parts(
        &self,
        pane_geom: &PaneGeom,
        k_chunk: &KittyImageChunk,
        character_cell_size: &SizeInPixels,
    ) -> Vec<KittyImageChunk> {
        ImageChunkGeometry {
            cell_x: k_chunk.cell_x,
            cell_y: k_chunk.cell_y,
            source_px_x: k_chunk.source_px_x,
            source_px_y: k_chunk.source_px_y,
            source_px_width: k_chunk.source_px_width,
            source_px_height: k_chunk.source_px_height,
        }
        .remove_covered_parts(pane_geom, character_cell_size, |geometry| KittyImageChunk {
            cell_x: geometry.cell_x,
            cell_y: geometry.cell_y,
            source_px_x: geometry.source_px_x,
            source_px_y: geometry.source_px_y,
            source_px_width: geometry.source_px_width,
            source_px_height: geometry.source_px_height,
            ..*k_chunk
        })
        .into_iter()
        .filter(|chunk| chunk.source_px_width > 0 && chunk.source_px_height > 0)
        .collect()
    }
    fn remove_covered_parts(
        &self,
        pane_geom: &PaneGeom,
        c_chunk: &mut CharacterChunk,
    ) -> Result<Option<CharacterChunk>> {
        let err_context = || {
            format!(
                "failed to remove covered parts from floating panes: {:#?}",
                self
            )
        };

        let pane_top_edge = pane_geom.y;
        let pane_left_edge = pane_geom.x;
        let pane_bottom_edge = pane_geom.y + pane_geom.rows.as_usize().saturating_sub(1);
        let pane_right_edge = pane_geom.x + pane_geom.cols.as_usize().saturating_sub(1);
        let c_chunk_left_side = c_chunk.x;
        let c_chunk_right_side = c_chunk.x + (c_chunk.width()).saturating_sub(1);
        if pane_top_edge <= c_chunk.y && pane_bottom_edge >= c_chunk.y {
            if pane_left_edge <= c_chunk_left_side && pane_right_edge >= c_chunk_right_side {
                // pane covers chunk completely
                drop(c_chunk.terminal_characters.drain(..));
                return Ok(None);
            } else if pane_right_edge >= c_chunk_left_side
                && pane_right_edge < c_chunk_right_side
                && pane_left_edge <= c_chunk_left_side
            {
                // pane covers chunk partially to the left
                let covered_part = c_chunk.drain_by_width(pane_right_edge + 1 - c_chunk_left_side);
                drop(covered_part);
                c_chunk.x = pane_right_edge + 1;
                return Ok(None);
            } else if pane_left_edge >= c_chunk_left_side
                && pane_left_edge <= c_chunk_right_side
                && pane_right_edge >= c_chunk_right_side
            {
                // pane covers chunk partially to the right
                c_chunk.retain_by_width(pane_left_edge - c_chunk_left_side);
                return Ok(None);
            } else if pane_left_edge >= c_chunk_left_side && pane_right_edge <= c_chunk_right_side {
                // pane covers chunk middle
                let (left_chunk_characters, right_chunk_characters) = c_chunk
                    .cut_middle_out(
                        pane_left_edge - c_chunk_left_side,
                        (pane_right_edge + 1) - c_chunk_left_side,
                    )
                    .with_context(err_context)?;
                let left_chunk_x = c_chunk_left_side;
                let right_chunk_x = pane_right_edge + 1;
                let mut left_chunk =
                    CharacterChunk::new(left_chunk_characters, left_chunk_x, c_chunk.y);
                left_chunk.pane_default_fg = c_chunk.pane_default_fg;
                left_chunk.pane_default_bg = c_chunk.pane_default_bg;
                left_chunk.theme_background = c_chunk.theme_background;
                left_chunk.changed_colors = c_chunk.changed_colors;
                left_chunk.agent_theme = c_chunk.agent_theme.clone();
                if !c_chunk.selection_and_colors.is_empty() {
                    left_chunk.selection_and_colors = c_chunk.selection_and_colors.clone();
                }

                c_chunk.x = right_chunk_x;
                c_chunk.terminal_characters = right_chunk_characters;
                return Ok(Some(left_chunk));
            }
        };
        Ok(None)
    }
    fn remove_covered_sixel_parts(
        &self,
        pane_geom: &PaneGeom,
        s_chunk: &SixelImageChunk,
        character_cell_size: &SizeInPixels,
    ) -> Vec<SixelImageChunk> {
        ImageChunkGeometry {
            cell_x: s_chunk.cell_x,
            cell_y: s_chunk.cell_y,
            source_px_x: s_chunk.sixel_image_pixel_x,
            source_px_y: s_chunk.sixel_image_pixel_y,
            source_px_width: s_chunk.sixel_image_pixel_width,
            source_px_height: s_chunk.sixel_image_pixel_height,
        }
        .remove_covered_parts(pane_geom, character_cell_size, |geometry| SixelImageChunk {
            cell_x: geometry.cell_x,
            cell_y: geometry.cell_y,
            sixel_image_pixel_x: geometry.source_px_x,
            sixel_image_pixel_y: geometry.source_px_y,
            sixel_image_pixel_width: geometry.source_px_width,
            sixel_image_pixel_height: geometry.source_px_height,
            ..*s_chunk
        })
    }
    pub fn cursor_is_visible(
        &self,
        cursor_x: usize,
        cursor_y: usize,
        z_index: Option<usize>,
    ) -> bool {
        let z_index = z_index.map(|z| z + 1).unwrap_or(0); // +1 because we only check panes above the active pane
        let panes_to_check = self.layers.iter().skip(z_index);
        for pane_geom in panes_to_check {
            let pane_top_edge = pane_geom.y;
            let pane_left_edge = pane_geom.x;
            let pane_bottom_edge = pane_geom.y + pane_geom.rows.as_usize().saturating_sub(1);
            let pane_right_edge = pane_geom.x + pane_geom.cols.as_usize().saturating_sub(1);
            if pane_top_edge <= cursor_y
                && pane_bottom_edge >= cursor_y
                && pane_left_edge <= cursor_x
                && pane_right_edge >= cursor_x
            {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct CharacterChunk {
    pub terminal_characters: Vec<TerminalCharacter>,
    pub x: usize,
    pub y: usize,
    pub changed_colors: Option<[Option<AnsiCode>; 256]>,
    pub pane_default_fg: Option<AnsiCode>,
    pub pane_default_bg: Option<AnsiCode>,
    /// Theme backgrounds remain eligible for the pane opacity setting.
    pub theme_background: bool,
    pub agent_theme: Option<Rc<crate::agent_theme::SourcePalette>>,
    selection_and_colors: Vec<HighlightSelection>,
}

pub use crate::panes::kitty_graphics::grid_state::KittyImageChunk;
pub use crate::panes::sixel::SixelImageChunk;

impl CharacterChunk {
    pub fn application_styles(&self, styles: CharacterStyles) -> CharacterStyles {
        self.agent_theme
            .as_ref()
            .map_or(styles, |mapping| mapping.apply(styles))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(terminal_characters: Vec<TerminalCharacter>, x: usize, y: usize) -> Self {
        CharacterChunk {
            terminal_characters,
            x,
            y,
            ..Default::default()
        }
    }
    pub(crate) fn offset(&mut self, x: usize, y: usize) {
        self.x += x;
        self.y += y;
        for highlight in &mut self.selection_and_colors {
            highlight.selection = highlight.selection.offset(x, y);
        }
    }
    pub fn add_selection_and_colors(
        &mut self,
        highlight: HighlightSelection,
        offset_x: usize,
        offset_y: usize,
    ) {
        self.selection_and_colors.push(HighlightSelection {
            selection: highlight.selection.offset(offset_x, offset_y),
            ..highlight
        });
    }
    pub fn selection_and_colors(&self) -> &[HighlightSelection] {
        &self.selection_and_colors
    }
    pub fn add_changed_colors(&mut self, changed_colors: Option<[Option<AnsiCode>; 256]>) {
        self.changed_colors = changed_colors;
    }
    pub fn add_pane_defaults(&mut self, fg: Option<AnsiCode>, bg: Option<AnsiCode>) {
        self.pane_default_fg = fg;
        self.pane_default_bg = bg;
    }
    pub fn changed_colors(&self) -> Option<[Option<AnsiCode>; 256]> {
        self.changed_colors
    }
    pub fn width(&self) -> usize {
        let mut width = 0;
        for t_character in &self.terminal_characters {
            width += t_character.width()
        }
        width
    }
    pub fn drain_by_width(&mut self, x: usize) -> impl Iterator<Item = TerminalCharacter> + use<> {
        let mut remaining_width = x;
        let end = self
            .terminal_characters
            .iter()
            .take_while(|character| {
                if character.width() <= remaining_width {
                    remaining_width -= character.width();
                    true
                } else {
                    false
                }
            })
            .count();
        // Move the suffix once, rather than once per drained character.
        let mut drained_part: Vec<_> = self.terminal_characters.drain(..end).collect();
        if remaining_width > 0
            && let Some(character) = self.terminal_characters.first()
        {
            let retained_width = character.width() - remaining_width;
            self.terminal_characters.splice(
                ..1,
                std::iter::repeat_n(EMPTY_TERMINAL_CHARACTER, retained_width),
            );
            drained_part.extend(std::iter::repeat_n(
                EMPTY_TERMINAL_CHARACTER,
                remaining_width,
            ));
        }
        drained_part.into_iter()
    }
    pub fn retain_by_width(&mut self, x: usize) {
        let part_to_retain = self.drain_by_width(x);
        self.terminal_characters = part_to_retain.collect();
    }
    pub fn cut_middle_out(
        &mut self,
        middle_start: usize,
        middle_end: usize,
    ) -> Result<(Vec<TerminalCharacter>, Vec<TerminalCharacter>)> {
        let err_context = || "failed to cut middle out of character chunk".to_string();

        let (
            absolute_middle_start_index,
            absolute_middle_end_index,
            pad_left_end_by,
            pad_right_start_by,
        ) = adjust_middle_segment_for_wide_chars(
            middle_start,
            middle_end,
            &self.terminal_characters,
        )
        .with_context(err_context)?;
        let mut terminal_characters: Vec<TerminalCharacter> =
            std::mem::take(&mut self.terminal_characters);
        let mut characters_on_the_right: Vec<TerminalCharacter> = terminal_characters
            .drain(absolute_middle_end_index..)
            .collect();
        let mut characters_on_the_left: Vec<TerminalCharacter> = terminal_characters
            .drain(..absolute_middle_start_index)
            .collect();
        characters_on_the_left.extend(std::iter::repeat_n(
            EMPTY_TERMINAL_CHARACTER,
            pad_left_end_by,
        ));
        if pad_right_start_by > 0 {
            for _ in 0..pad_right_start_by {
                characters_on_the_right.insert(0, EMPTY_TERMINAL_CHARACTER);
            }
        }
        Ok((characters_on_the_left, characters_on_the_right))
    }
}

/// The set of viewport line indices that need re-rendering.
///
/// This is marked once per printed character, so it sits directly on the pty ingest path. The
/// indices are dense and small (`0..viewport_height`), which makes a bitset a much better fit than
/// a `HashSet` - hashing a `usize` with SipHash cost more than the render it was tracking.
/// Iteration is ascending by construction, so callers that need ordered lines no longer sort.
#[derive(Clone, Debug, Default)]
pub struct ChangedLines {
    words: Vec<u64>,
}

impl ChangedLines {
    #[inline]
    pub fn insert(&mut self, line_index: usize) {
        let word = line_index / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1 << (line_index % 64);
    }
    #[inline]
    pub fn contains(&self, line_index: usize) -> bool {
        self.words
            .get(line_index / 64)
            .is_some_and(|word| word & (1 << (line_index % 64)) != 0)
    }
    pub fn clear(&mut self) {
        // keep the allocation - the same pane dirties lines again next frame
        self.words.clear();
    }
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }
    pub fn union_with(&mut self, other: &ChangedLines) {
        if other.words.len() > self.words.len() {
            self.words.resize(other.words.len(), 0);
        }
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }
    /// Ascending line indices.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(word_index, word)| {
                let mut remaining = *word;
                std::iter::from_fn(move || {
                    if remaining == 0 {
                        return None;
                    }
                    let bit = remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    Some(word_index * 64 + bit)
                })
            })
    }
}

#[derive(Clone, Debug)]
pub struct OutputBuffer {
    pub changed_lines: ChangedLines, // line index
    pub should_update_all_lines: bool,
    /// What the last render read, kept so watchers can be sent the same lines.
    last_read: ChangedLines,
    last_read_all: bool,
    styled_underlines: bool,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        OutputBuffer {
            changed_lines: ChangedLines::default(),
            should_update_all_lines: true, // first time we should do a full render
            last_read: ChangedLines::default(),
            last_read_all: false,
            styled_underlines: true,
        }
    }
}

impl OutputBuffer {
    pub fn update_line(&mut self, line_index: usize) {
        if !self.should_update_all_lines {
            self.changed_lines.insert(line_index);
        }
    }
    pub fn update_lines(&mut self, start: usize, end: usize) {
        if !self.should_update_all_lines {
            for idx in start..=end {
                self.changed_lines.insert(idx);
            }
        }
    }
    pub fn update_all_lines(&mut self) {
        self.clear();
        self.should_update_all_lines = true;
    }
    pub fn clear(&mut self) {
        self.changed_lines.clear();
        self.should_update_all_lines = false;
    }
    /// Clear the changed lines once a render has read them, remembering them for
    /// [`Self::replay_last_read`]. Swapping keeps both allocations.
    pub fn consume(&mut self) {
        std::mem::swap(&mut self.changed_lines, &mut self.last_read);
        self.last_read_all = self.should_update_all_lines;
        self.clear();
    }
    /// Mark the lines the last render read as changed again, so a second render
    /// before any new output sends exactly what the first one did.
    pub fn replay_last_read(&mut self) {
        if self.last_read_all {
            self.update_all_lines();
        } else if !self.should_update_all_lines {
            self.changed_lines.union_with(&self.last_read);
        }
    }
    pub fn serialize(
        &self,
        viewport: &[Row],
        osc8_hyperlinks: bool,
        max_size: Option<Size>,
    ) -> Result<String> {
        let mut chunks = Vec::new();
        for (line_index, line) in viewport.iter().enumerate() {
            let terminal_characters =
                self.extract_line_from_viewport(line_index, viewport, line.width());

            let x = 0;
            let y = line_index;
            chunks.push((
                CharacterChunk::new(terminal_characters, x, y),
                line.is_canonical,
                None,
            ));
        }
        serialize_chunks_with_newlines(
            chunks,
            None,
            self.styled_underlines,
            osc8_hyperlinks,
            max_size,
            None,
        )
    }
    pub fn serialize_handoff(
        &self,
        rows: &[Row],
        width: usize,
        osc8_hyperlinks: bool,
        handoff: &mut yinz_types::agent::theme::AgentThemeHandoff,
    ) -> Result<String> {
        let mut chunks = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            // Include implicit background cells: these also have palette ownership.
            let characters = self.extract_characters_from_row(row, row.width().max(width));
            let parts = row.split_palette_chunks(CharacterChunk::new(characters, 0, y));
            for (index, part) in parts.into_iter().enumerate() {
                chunks.push((part, index == 0 && row.is_canonical, Some(row.width())));
            }
        }
        serialize_chunks_with_newlines(
            chunks,
            None,
            self.styled_underlines,
            osc8_hyperlinks,
            None,
            Some(handoff),
        )
    }
    pub fn changed_chunks_in_viewport(
        &self,
        viewport: &[Row],
        viewport_width: usize,
        viewport_height: usize,
        x_offset: usize,
        y_offset: usize,
    ) -> Vec<CharacterChunk> {
        if self.should_update_all_lines {
            let mut changed_chunks = Vec::new();
            for line_index in 0..viewport_height {
                let terminal_characters =
                    self.extract_line_from_viewport(line_index, viewport, viewport_width);

                let x = x_offset; // right now we only buffer full lines as this doesn't seem to have a huge impact on performance, but the infra is here if we want to change this
                let y = line_index + y_offset;
                changed_chunks.push(CharacterChunk::new(terminal_characters, x, y));
            }
            changed_chunks
        } else {
            // `ChangedLines::iter` is already ascending, so this no longer collects and sorts
            let mut changed_chunks = Vec::new();
            for line_index in self.changed_lines.iter().filter(|i| *i < viewport_height) {
                let terminal_characters =
                    self.extract_line_from_viewport(line_index, viewport, viewport_width);
                let x = x_offset;
                let y = line_index + y_offset;
                changed_chunks.push(CharacterChunk::new(terminal_characters, x, y));
            }
            changed_chunks
        }
    }
    fn extract_characters_from_row(
        &self,
        row: &Row,
        viewport_width: usize,
    ) -> Vec<TerminalCharacter> {
        let mut terminal_characters: Vec<TerminalCharacter> = row.columns.iter().cloned().collect();
        // pad row
        let row_width = row.width();
        if row_width < viewport_width {
            let mut pad_character = EMPTY_TERMINAL_CHARACTER;
            if let Some(bg_color) = row.bg_color {
                pad_character
                    .styles
                    .update(|styles| styles.background = Some(bg_color));
            }
            let mut padding = vec![pad_character; viewport_width - row_width];
            terminal_characters.append(&mut padding);
        } else if row_width > viewport_width {
            // A clipped wide character still occupies cells that must erase the old frame.
            let mut chunk = CharacterChunk::new(terminal_characters, 0, 0);
            chunk.retain_by_width(viewport_width);
            terminal_characters = chunk.terminal_characters;
        }
        terminal_characters
    }
    fn extract_line_from_viewport(
        &self,
        line_index: usize,
        viewport: &[Row],
        viewport_width: usize,
    ) -> Vec<TerminalCharacter> {
        match viewport.get(line_index) {
            Some(row) => self.extract_characters_from_row(row, viewport_width),
            None => {
                vec![EMPTY_TERMINAL_CHARACTER; viewport_width]
            },
        }
    }
    pub fn changed_rects_in_viewport(&self, viewport_height: usize) -> HashMap<usize, usize> {
        // group the changed lines into "changed_rects", which indicate where the line starts (the
        // hashmap key) and how many lines are in there (its value)
        let mut changed_rects: HashMap<usize, usize> = HashMap::new(); // <start_line_index, line_count>
        let mut last_changed_line_index: Option<usize> = None;
        let mut changed_line_count = 0;
        let mut add_changed_line = |line_index| match last_changed_line_index.as_mut() {
            Some(changed_line_index) => {
                if *changed_line_index + changed_line_count == line_index {
                    changed_line_count += 1
                } else {
                    changed_rects.insert(*changed_line_index, changed_line_count);
                    last_changed_line_index = Some(line_index);
                    changed_line_count = 1;
                }
            },
            None => {
                last_changed_line_index = Some(line_index);
                changed_line_count = 1;
            },
        };

        if self.should_update_all_lines {
            // for line_index in 0..self.viewport.len() {
            for line_index in 0..viewport_height {
                add_changed_line(line_index);
            }
        } else {
            // ascending, which `add_changed_line` requires to group contiguous lines into one
            // rect - the previous `HashSet` iteration order was arbitrary, so runs were split and
            // `changed_rects.insert` could clobber an earlier rect at the same start index
            for line_index in self.changed_lines.iter() {
                add_changed_line(line_index);
            }
        }
        if let Some(changed_line_index) = last_changed_line_index {
            changed_rects.insert(changed_line_index, changed_line_count);
        }
        changed_rects
    }
}

#[cfg(test)]
mod unit;

#[cfg(test)]
#[path = "unit/translucency_tests.rs"]
mod translucency_tests;
