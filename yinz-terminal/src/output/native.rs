//! Structured counterpart of the ANSI serializer. Both consume the same composed chunks.
use super::*;
use crate::panes::terminal_character::{AnsiStyledUnderline, NamedColor};
use yinz_types::{data::PaletteColor, structured_render::*};

#[derive(Default)]
pub struct NativeRenderState {
    pub seq: u64,
    pub acknowledged: u64,
    media: Vec<(u64, tempfile::NamedTempFile)>,
    cursor: CursorState,
    links: Vec<String>,
    size: Option<Size>,
    presentation: Option<Presentation>,
}

impl NativeRenderState {
    pub fn acknowledge(&mut self, seq: u64) {
        if seq <= self.seq {
            self.acknowledged = self.acknowledged.max(seq);
            self.media.retain(|(frame, _)| *frame > self.acknowledged);
        }
    }
}

impl Output {
    pub fn set_native_geometry(&mut self, client: ClientId, geometry: GeometryRecord) {
        self.native_geometry.insert(client, geometry);
    }

    /// Drain just native clients, leaving the ordinary ANSI clients for `serialize`.
    pub fn serialize_native(
        &mut self,
        clients: &mut HashMap<ClientId, NativeRenderState>,
        sizes: &HashMap<ClientId, Size>,
        presentations: &HashMap<ClientId, Presentation>,
    ) -> HashMap<ClientId, Vec<u8>> {
        let mut frames = HashMap::new();
        for (client, state) in clients {
            let Some(chunks) = self.client_character_chunks.remove(client) else {
                continue;
            };
            let size = sizes.get(client).copied().unwrap_or(Size {
                cols: 120,
                rows: 40,
            });
            let presentation = presentations.get(client).copied();
            let (dx, dy) = presentation.map_or((0, 0), Presentation::offset);
            state.seq += 1;
            let mut builder = FrameBuilder::new(
                size.cols.min(u16::MAX as usize) as u16,
                size.rows.min(u16::MAX as usize) as u16,
                state.seq,
            );
            let mut cleared = self.clients_with_cleared_host_display.remove(client)
                || state.size != Some(size)
                || state.presentation != presentation;
            if cleared {
                // Full repaints reset the receiver's cells and hyperlink table together.
                state.links.clear();
            }
            builder.set_full_repaint(cleared);
            builder.set_clear(cleared);
            state.size = Some(size);
            state.presentation = presentation;
            for instructions in [
                self.pre_vte_instructions.remove(client),
                self.post_vte_instructions.get(client).cloned(),
            ] {
                for instruction in instructions.into_iter().flatten() {
                    let mut parser = vte::Parser::new();
                    let mut control = CursorControl {
                        cursor: &mut state.cursor,
                        clear: false,
                    };
                    parser.advance(&mut control, instruction.as_bytes());
                    if control.clear {
                        cleared = true;
                        builder.set_clear(true);
                    }
                    builder
                        .sideband_mut()
                        .extend_from_slice(instruction.as_bytes());
                }
            }
            if cleared && let Some(presentation) = presentation {
                paint_native_margins(&mut builder, presentation);
            }
            let handler = self
                .client_link_handlers
                .get(client)
                .map(|handler| handler.borrow());
            for chunk in chunks {
                if chunk.x + dx >= size.cols || chunk.y + dy >= size.rows {
                    continue;
                }
                let mut cells = Vec::new();
                let mut col = chunk.x;
                for character in &chunk.terminal_characters {
                    let width = character.width();
                    if width == 0 {
                        continue;
                    }
                    if col + width + dx > size.cols {
                        break;
                    }
                    let styles = adjust_styles_for_custom_bg_fg(
                        adjust_styles_for_possible_selection(
                            chunk.selection_and_colors(),
                            chunk.application_styles(*character.styles),
                            chunk.y,
                            col,
                        ),
                        chunk.pane_default_fg,
                        chunk.pane_default_bg,
                    );
                    let mut cell =
                        wire_cell(character, styles, chunk.changed_colors.as_ref(), true);
                    if let Some(uri) = handler
                        .as_ref()
                        .and_then(|handler| handler.uri(character.styles.link_anchor))
                    {
                        let index = state.links.iter().position(|link| link == uri).or_else(|| {
                            if state.links.len() >= MAX_LINK_ID as usize {
                                return None;
                            }
                            state.links.push(uri.to_owned());
                            Some(state.links.len() - 1)
                        });
                        cell.link = index.map_or(0, |index| (index + 1) as u8);
                    }
                    cells.push(cell);
                    cells.extend((1..width).map(|_| WireCell {
                        ch: ' ' as u32,
                        width: 1,
                        ..cell
                    }));
                    col += width;
                }
                builder.push_row((chunk.y + dy) as u16, (chunk.x + dx) as u16, &cells);
            }
            builder.push_links(&LinkRecord {
                entries: state
                    .links
                    .iter()
                    .enumerate()
                    .map(|(index, uri)| LinkEntry {
                        id: (index + 1) as u8,
                        uri: uri.clone(),
                    })
                    .collect(),
            });
            if let Some(mut geometry) = self.native_geometry.remove(client) {
                for pane in &mut geometry.panes {
                    pane.x = pane.x.saturating_add(dx.min(u16::MAX as usize) as u16);
                    pane.y = pane.y.saturating_add(dy.min(u16::MAX as usize) as u16);
                }
                builder.push_geometry(&geometry);
            }
            let mut cursor = state.cursor;
            cursor.x = cursor.x.saturating_add(dx.min(u16::MAX as usize) as u16);
            cursor.y = cursor.y.saturating_add(dy.min(u16::MAX as usize) as u16);
            builder.set_cursor(cursor);
            self.post_vte_instructions.remove(client);
            // Sixel already has cropped, occluded slices at this stage.
            if let Some(chunks) = self.sixel_chunks.remove(client) {
                for chunk in chunks {
                    if let Some(payload) = self.sixel_image_store.borrow_mut().serialize_image(
                        chunk.sixel_image_id,
                        chunk.sixel_image_pixel_x,
                        chunk.sixel_image_pixel_y,
                        chunk.sixel_image_pixel_width,
                        chunk.sixel_image_pixel_height,
                    ) {
                        builder.extend_graphics(&[GraphicsRecord::SixelChunk {
                            cell_x: (chunk.cell_x + dx) as u32,
                            cell_y: (chunk.cell_y + dy) as u32,
                            pixel_x: 0,
                            pixel_y: 0,
                            pixel_width: chunk.sixel_image_pixel_width as u32,
                            pixel_height: chunk.sixel_image_pixel_height as u32,
                            payload: payload.into_bytes(),
                        }]);
                    }
                }
            }
            let chunks_by_pane = self.client_kitty_chunks.remove(client).unwrap_or_default();
            let rendered_panes = self
                .client_rendered_kitty_panes
                .remove(client)
                .unwrap_or_default();
            let updated_placements = self
                .client_updated_kitty_placements
                .remove(client)
                .unwrap_or_default();
            let visible = self.client_kitty_visible_panes.remove(client);
            let mut records = Vec::new();
            flatten_kitty_frame(
                KittyFrameInput {
                    chunks_by_pane,
                    rendered_panes,
                    updated_placements,
                    visible_panes: visible.as_ref(),
                    kitty_image_store: &mut self.kitty_image_store.borrow_mut(),
                    host_state: self
                        .kitty_host_state
                        .borrow_mut()
                        .entry(*client)
                        .or_default(),
                    host_display_cleared: cleared,
                },
                &mut records,
                state.seq,
                &mut state.media,
            )
            .context("flatten native graphics")
            .non_fatal();
            for record in &mut records {
                if let GraphicsRecord::Placement { cell_x, cell_y, .. } = record {
                    *cell_x = cell_x.saturating_add(dx as u32);
                    *cell_y = cell_y.saturating_add(dy as u32);
                }
            }
            builder.extend_graphics(&records);
            frames.insert(*client, builder.finish());
        }
        frames
    }
}

// Gutters use explicit cells because native clients do not interpret ANSI erases.
fn paint_native_margins(builder: &mut FrameBuilder, presentation: Presentation) {
    let (dx, dy) = presentation.offset();
    let bg = match presentation.background {
        PaletteColor::Rgb((r, g, b)) => WireColor::Rgb(r, g, b),
        PaletteColor::EightBit(n) => WireColor::Indexed(n),
    }
    .pack();
    let cell = WireCell {
        ch: ' ' as u32,
        bg,
        ..WireCell::default()
    };
    for y in 0..presentation.terminal.rows {
        let spans = if y < dy || y >= dy + presentation.content.rows {
            [(0, presentation.terminal.cols), (0, 0)]
        } else {
            [
                (0, dx),
                (
                    dx + presentation.content.cols,
                    presentation
                        .terminal
                        .cols
                        .saturating_sub(dx + presentation.content.cols),
                ),
            ]
        };
        for (x, width) in spans {
            if width > 0 {
                builder.push_row(y as u16, x as u16, &vec![cell; width]);
            }
        }
    }
}

struct CursorControl<'a> {
    cursor: &'a mut CursorState,
    clear: bool,
}
impl vte::Perform for CursorControl<'_> {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let args: Vec<u16> = params.iter().map(|p| p[0]).collect();
        match (action, intermediates) {
            ('H' | 'f', []) => {
                self.cursor.y = args.first().copied().unwrap_or(1).max(1) - 1;
                self.cursor.x = args.get(1).copied().unwrap_or(1).max(1) - 1;
            },
            ('h', [b'?']) if args.contains(&25) => self.cursor.visible = true,
            ('l', [b'?']) if args.contains(&25) => self.cursor.visible = false,
            ('J', []) if args.first() == Some(&2) => self.clear = true,
            ('q', [b' ']) => {
                let style = args.first().copied().unwrap_or(0);
                self.cursor.shape = match style {
                    0 => CURSOR_SHAPE_DEFAULT,
                    3 | 4 => CURSOR_SHAPE_UNDERLINE,
                    5 | 6 => CURSOR_SHAPE_BEAM,
                    _ => CURSOR_SHAPE_BLOCK,
                } | if style % 2 == 1 { CURSOR_BLINKING } else { 0 };
            },
            _ => {},
        }
    }
}

fn wire_color(code: Option<AnsiCode>) -> u32 {
    match code {
        Some(AnsiCode::NamedColor(named)) => WireColor::Named(named_color_index(named)).pack(),
        Some(AnsiCode::ColorIndex(index)) => WireColor::Indexed(index).pack(),
        Some(AnsiCode::RgbCode((r, g, b))) => WireColor::Rgb(r, g, b).pack(),
        _ => WireColor::Default.pack(),
    }
}

fn named_color_index(named: NamedColor) -> u8 {
    match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
    }
}

fn substitute_changed_color(
    code: Option<AnsiCode>,
    changed_colors: Option<&[Option<AnsiCode>; 256]>,
) -> Option<AnsiCode> {
    match (code, changed_colors) {
        (Some(AnsiCode::ColorIndex(index)), Some(changed_colors)) => {
            changed_colors[index as usize].or(code)
        },
        _ => code,
    }
}

fn wire_attrs(styles: &CharacterStyles, styled_underlines: bool) -> u16 {
    let mut attrs = 0u16;
    for (code, bit) in [
        (styles.bold, ATTR_BOLD),
        (styles.dim, ATTR_DIM),
        (styles.italic, ATTR_ITALIC),
        (styles.reverse, ATTR_REVERSE),
        (styles.hidden, ATTR_HIDDEN),
        (styles.strike, ATTR_STRIKE),
        (styles.slow_blink, ATTR_SLOW_BLINK),
        (styles.fast_blink, ATTR_FAST_BLINK),
    ] {
        if matches!(code, Some(AnsiCode::On)) {
            attrs |= bit;
        }
    }
    let underline = match styles.underline {
        Some(AnsiCode::Underline(None)) => UNDERLINE_STRAIGHT,
        Some(AnsiCode::Underline(Some(styled))) if styled_underlines => match styled {
            AnsiStyledUnderline::Double => UNDERLINE_DOUBLE,
            AnsiStyledUnderline::Undercurl => UNDERLINE_CURLY,
            AnsiStyledUnderline::Underdotted => UNDERLINE_DOTTED,
            AnsiStyledUnderline::Underdashed => UNDERLINE_DASHED,
        },
        Some(AnsiCode::Underline(Some(_))) => UNDERLINE_STRAIGHT,
        _ => UNDERLINE_NONE,
    };
    attrs |= underline << UNDERLINE_SHIFT;
    attrs
}

#[inline]
fn wire_cell(
    t_character: &TerminalCharacter,
    styles: CharacterStyles,
    chunk_changed_colors: Option<&[Option<AnsiCode>; 256]>,
    styled_underlines: bool,
) -> WireCell {
    WireCell {
        ch: t_character.character as u32,
        fg: wire_color(substitute_changed_color(
            styles.foreground,
            chunk_changed_colors,
        )),
        bg: wire_color(substitute_changed_color(
            styles.background,
            chunk_changed_colors,
        )),
        underline_color: if styled_underlines {
            wire_color(styles.underline_color)
        } else {
            WireColor::Default.pack()
        },
        attrs: wire_attrs(&styles, styled_underlines),
        width: t_character.width().min(u8::MAX as usize) as u8,
        link: LINK_NONE,
    }
}

fn flatten_kitty_frame(
    kitty_input: KittyFrameInput,
    out: &mut Vec<GraphicsRecord>,
    seq: u64,
    media: &mut Vec<(u64, tempfile::NamedTempFile)>,
) -> Result<()> {
    let KittyFrameInput {
        mut chunks_by_pane,
        rendered_panes,
        updated_placements,
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
        out.push(GraphicsRecord::DeleteImage { id: host_id as u64 });
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
            out.push(GraphicsRecord::DeletePlacement {
                image_id: host_image_id as u64,
                placement_id: host_placement_id as u64,
            });
            host_state.live_placements.remove(&key);
        }
    }
    // Overlay placements update independently of their owning pane, even when fully covered.
    let mut rendered: Vec<PaneId> = rendered_panes
        .iter()
        .copied()
        .chain(chunks_by_pane.keys().copied())
        .chain(updated_placements.iter().map(|(pane, _)| *pane))
        .collect();
    rendered.sort_unstable();
    rendered.dedup();
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
            .filter(|(key, _)| {
                key.pane_id == pane_id
                    && (rendered_panes.contains(&pane_id)
                        || updated_placements.contains(&(pane_id, key.placement_uid)))
                    && !current.contains_key(key)
            })
            .map(|(key, record)| (key.clone(), record.host_image_id, record.host_placement_id))
            .collect();
        stale.sort_by_key(|(key, _, _)| (key.placement_uid, key.sub_index));
        for (key, host_image_id, host_placement_id) in stale {
            out.push(GraphicsRecord::DeletePlacement {
                image_id: host_image_id as u64,
                placement_id: host_placement_id as u64,
            });
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
                    if let Some(variant) = variant {
                        kitty_image_store.ensure_scaled_variant(chunk.internal_image_id, variant);
                    }
                    let host_image_id = host_state.next_host_image_id;
                    host_state.next_host_image_id += 1;
                    let Some(image) = kitty_image_store.get(chunk.internal_image_id) else {
                        continue;
                    };
                    let (width, height, rgba) = if let Some(variant) = variant {
                        let Some(rgba) =
                            kitty_image_store.scaled_variant(chunk.internal_image_id, variant)
                        else {
                            continue;
                        };
                        (variant.size.0 as u32, variant.size.1 as u32, rgba)
                    } else {
                        (image.width, image.height, image.displayed_rgba())
                    };
                    // Keep large uploads out of the bounded IPC frame. The file stays alive
                    // until this client acknowledges applying the frame that references it.
                    let shared = if rgba.len() >= 256 * 1024 && media.len() < 32 {
                        write_shared_media(rgba).map(|file| {
                            let path = file.path().to_string_lossy().as_bytes().to_vec();
                            media.push((seq, file));
                            path
                        })
                    } else {
                        None
                    };
                    let (medium, data) = match shared {
                        Some(path) => (GraphicsMedium::SharedFile, path),
                        None => (GraphicsMedium::Inline, rgba.to_vec()),
                    };
                    out.push(GraphicsRecord::Residency {
                        id: host_image_id as u64,
                        width,
                        height,
                        format: GRAPHICS_FORMAT_RGBA8,
                        medium,
                        byte_len: rgba.len() as u32,
                        data,
                    });
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
                        out.push(GraphicsRecord::DeletePlacement {
                            image_id: existing_host_image_id as u64,
                            placement_id: host_placement_id as u64,
                        });
                    }
                    host_placement_id
                },
                None => {
                    let host_placement_id = host_state.next_host_placement_id;
                    host_state.next_host_placement_id += 1;
                    host_placement_id
                },
            };
            out.push(GraphicsRecord::Placement {
                image_id: host_image_id as u64,
                placement_id: host_placement_id as u64,
                cell_x: chunk.cell_x as u32,
                cell_y: chunk.cell_y as u32,
                offset_x: chunk.cell_offset_x,
                offset_y: chunk.cell_offset_y,
                source_x: chunk.source_px_x as u32,
                source_y: chunk.source_px_y as u32,
                source_width: chunk.source_px_width as u32,
                source_height: chunk.source_px_height as u32,
                z: chunk.z_index,
            });
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

fn write_shared_media(bytes: &[u8]) -> Option<tempfile::NamedTempFile> {
    use std::io::Write;
    let directory = if cfg!(target_os = "linux") && std::path::Path::new("/dev/shm").is_dir() {
        std::path::PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    let result = (|| -> Result<tempfile::NamedTempFile> {
        let mut file = tempfile::Builder::new()
            .prefix("yinz-native-image-")
            .tempfile_in(directory)
            .context("create native shared image")?;
        file.write_all(bytes).context("write native shared image")?;
        Ok(file)
    })();
    match result {
        Ok(file) => Some(file),
        Err(error) => {
            log::warn!("{error:#}; sending native image inline");
            None
        },
    }
}

#[cfg(test)]
#[path = "unit/native_media_tests.rs"]
mod tests;
