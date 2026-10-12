//! Per-frame text composition, restricted to translucent overlay rectangles.
use super::*;
use yinz_types::{
    animation::blend,
    colors::{eightbit_to_rgb, PaletteColor},
};

#[derive(Clone, Debug)]
struct Cell {
    character: TerminalCharacter,
    combining: Vec<TerminalCharacter>,
    start: usize,
    layer: usize,
    transparent: bool,
}

// Rows stay ordered; columns use direct indexing instead of a tree lookup for every cell.
type Cells = BTreeMap<usize, Vec<Option<Cell>>>;

/// Untinted backing cells for a screen overlay, bounded by its current rectangle.
#[derive(Clone, Debug, Default)]
pub struct TranslucentOverlayBacking {
    composition: Option<Composition>,
}

impl TranslucentOverlayBacking {
    /// Forget exposed overlay cells before underlying panes repaint their damage.
    pub fn invalidate(&mut self, damage: &[CellRect]) {
        let Some(composition) = &mut self.composition else {
            return;
        };
        for rect in damage {
            for cells in composition.layers.values_mut() {
                for (_, row) in cells.range_mut(rect.y..rect.y + rect.rows) {
                    let start = rect.x.saturating_sub(composition.left);
                    let end = (rect.x + rect.cols).saturating_sub(composition.left);
                    for cell in row.iter_mut().take(end).skip(start) {
                        *cell = None;
                    }
                }
            }
        }
    }

    /// Returns whether the caller must repaint the underlying panes before composing.
    pub fn prepare(
        &mut self,
        rect: CellRect,
        opacity: u8,
        palette: yinz_types::style::Palette,
        tint: PaletteColor,
        terminal_emulator_color_codes: &HashMap<usize, String>,
    ) -> bool {
        let next = Composition::new(
            vec![(1, rect, opacity)],
            palette.fg,
            palette.bg,
            tint,
            host_colors(terminal_emulator_color_codes),
        );
        if let Some(current) = &self.composition {
            if current.overlays == next.overlays
                && current.fg == next.fg
                && current.bg == next.bg
                && current.tint == next.tint
                && current.host_colors == next.host_colors
            {
                return current.layers.is_empty();
            }
        }
        self.composition = Some(next);
        true
    }

    pub(super) fn update(
        &mut self,
        chunks: &[CharacterChunk],
        display_cleared: bool,
    ) -> Result<Composition> {
        let composition = self
            .composition
            .as_mut()
            .context("overlay backing was not prepared")?;
        if display_cleared {
            composition.layers.clear();
        }
        composition.add(chunks, 0);
        // Only the copy receives the overlay, so its tint and lettering never enter the cache.
        Ok(composition.clone())
    }
}

pub(super) fn host_colors(codes: &HashMap<usize, String>) -> [Option<AnsiCode>; 256] {
    let mut colors = [None; 256];
    for (&index, value) in codes {
        if let Some(color) = colors.get_mut(index) {
            *color = crate::panes::alacritty_functions::xparse_color(value.as_bytes());
        }
    }
    colors
}

#[derive(Clone, Debug)]
pub(super) struct Composition {
    overlays: Vec<(usize, CellRect, u8)>,
    layers: BTreeMap<usize, Cells>,
    left: usize,
    fg: PaletteColor,
    bg: PaletteColor,
    tint: PaletteColor,
    host_colors: [Option<AnsiCode>; 256],
}

fn color(
    code: Option<AnsiCode>,
    defaults: PaletteColor,
    changed: Option<&[Option<AnsiCode>; 256]>,
    host_colors: &[Option<AnsiCode>; 256],
) -> PaletteColor {
    let index = match code {
        Some(AnsiCode::RgbCode(rgb)) => return PaletteColor::Rgb(rgb),
        Some(AnsiCode::ColorIndex(index)) => index,
        Some(AnsiCode::NamedColor(named)) => named as u8,
        _ => return defaults,
    };
    match changed
        .and_then(|colors| colors[index as usize])
        .or(host_colors[index as usize])
    {
        Some(AnsiCode::RgbCode(rgb)) => PaletteColor::Rgb(rgb),
        Some(code) => color(Some(code), defaults, None, host_colors),
        _ => PaletteColor::Rgb(eightbit_to_rgb(index)),
    }
}

/// Whether two cells' styles are equal, checking the shared-allocation case first.
fn same_styles(a: &RcCharacterStyles, b: &RcCharacterStyles) -> bool {
    a.ptr_eq(b) || **a == **b
}

impl Composition {
    pub(super) fn protect_cursor_cell(&mut self, x: usize, y: usize, layer: usize) {
        if !self.contains_layer(layer) {
            return;
        }
        let Some(index) = x.checked_sub(self.left) else {
            return;
        };
        if let Some(cell) = self
            .layers
            .get_mut(&layer)
            .and_then(|cells| cells.get_mut(&y))
            .and_then(|row| row.get_mut(index))
            .and_then(Option::as_mut)
        {
            cell.transparent = false;
        }
    }

    pub(super) fn contains_layer(&self, layer: usize) -> bool {
        self.overlays.iter().any(|(z, _, _)| *z == layer)
    }

    pub(super) fn new(
        overlays: Vec<(usize, CellRect, u8)>,
        fg: PaletteColor,
        bg: PaletteColor,
        tint: PaletteColor,
        host_colors: [Option<AnsiCode>; 256],
    ) -> Self {
        Self {
            left: overlays
                .iter()
                .map(|(_, rect, _)| rect.x)
                .min()
                .unwrap_or(0),
            overlays,
            layers: BTreeMap::new(),
            fg: color(Some(fg.into()), fg, None, &host_colors),
            bg: color(Some(bg.into()), bg, None, &host_colors),
            tint: color(Some(tint.into()), tint, None, &host_colors),
            host_colors,
        }
    }

    pub(super) fn outside_chunks(
        &self,
        chunks: Vec<CharacterChunk>,
        layer: usize,
    ) -> Result<Vec<CharacterChunk>> {
        let clip = FloatingPanesStack {
            layers: self
                .overlays
                .iter()
                .filter(|(z, _, _)| *z >= layer)
                .map(|(_, rect, _)| PaneGeom {
                    x: rect.x,
                    y: rect.y,
                    cols: yinz_types::pane_size::Dimension::fixed(rect.cols),
                    rows: yinz_types::pane_size::Dimension::fixed(rect.rows),
                    ..Default::default()
                })
                .collect(),
        };
        clip.visible_character_chunks(chunks, None)
            .context("failed to clip composed rectangles")
    }

    /// A covered cell's styles with every color made concrete, and whether its background
    /// is the default one the panel tints.
    fn resolve_styles(
        &self,
        chunk: &CharacterChunk,
        character: &TerminalCharacter,
        x: usize,
        layer: usize,
    ) -> (bool, RcCharacterStyles) {
        let styles = adjust_styles_for_possible_selection(
            chunk.selection_and_colors(),
            chunk.application_styles(*character.styles),
            chunk.y,
            x,
        );
        let selected_background = chunk
            .selection_and_colors()
            .iter()
            .find(|highlight| highlight.selection.contains(chunk.y, x))
            .is_some_and(|highlight| highlight.bg.is_some());
        let mapped_theme_background = !selected_background
            && chunk
                .agent_theme
                .as_ref()
                .is_some_and(|palette| palette.theme_background(*character.styles));
        let default_background = (matches!(styles.background, None | Some(AnsiCode::Reset))
            || mapped_theme_background)
            && styles.reverse != Some(AnsiCode::On)
            && (chunk.pane_default_bg.is_none() || chunk.theme_background);
        let mut styles =
            adjust_styles_for_custom_bg_fg(styles, chunk.pane_default_fg, chunk.pane_default_bg);
        let mut fg = color(
            styles.foreground,
            self.fg,
            chunk.changed_colors.as_ref(),
            &self.host_colors,
        );
        let mut bg = color(
            styles.background,
            self.bg,
            chunk.changed_colors.as_ref(),
            &self.host_colors,
        );
        if styles.reverse == Some(AnsiCode::On) {
            std::mem::swap(&mut fg, &mut bg);
        }
        styles.reverse = Some(AnsiCode::Reset);
        styles.foreground = Some(fg.into());
        // Default panel backgrounds (including under its lettering) are the tint.
        if default_background && self.overlays.iter().any(|(z, _, _)| *z == layer) {
            bg = self.tint;
        }
        styles.background = Some(bg.into());
        if let Some(underline) = styles.underline_color {
            if !matches!(underline, AnsiCode::Reset) {
                styles.underline_color = Some(
                    color(
                        Some(underline),
                        fg,
                        chunk.changed_colors.as_ref(),
                        &self.host_colors,
                    )
                    .into(),
                );
            }
        }
        (default_background, styles.into())
    }

    pub(super) fn add(&mut self, chunks: &[CharacterChunk], layer: usize) {
        // Out of `layers` while it is filled, so cells can be resolved through `self`.
        let mut cells = self.layers.remove(&layer).unwrap_or_default();
        for chunk in chunks {
            if !self.overlays.iter().any(|(_, rect, _)| {
                chunk.y >= rect.y
                    && chunk.y < rect.y + rect.rows
                    && chunk.x < rect.x + rect.cols
                    && chunk.x + chunk.width() > rect.x
            }) {
                continue;
            }
            let row = cells.entry(chunk.y).or_default();
            // Without selections a cell's resolved styles depend only on its own styles and on
            // this chunk, so a run printed with one pen resolves once and shares the result.
            let has_selections = !chunk.selection_and_colors().is_empty();
            let mut resolved_run: Option<(RcCharacterStyles, bool, RcCharacterStyles)> = None;
            let mut x = chunk.x;
            let mut characters = chunk.terminal_characters.iter().peekable();
            while let Some(character) = characters.next() {
                let width = character.width();
                if width == 0 {
                    continue;
                }
                let mut combining = vec![];
                while characters.peek().is_some_and(|c| c.width() == 0) {
                    combining.extend(characters.next().cloned());
                }
                let mut covered = (x..x + width)
                    .filter(|x| {
                        self.overlays
                            .iter()
                            .any(|(_, rect, _)| rect.contains(*x, chunk.y))
                    })
                    .peekable();
                if covered.peek().is_some() {
                    let reused = resolved_run
                        .as_ref()
                        .filter(|(source, _, _)| {
                            !has_selections && same_styles(source, &character.styles)
                        })
                        .map(|(_, default_background, resolved)| {
                            (*default_background, resolved.clone())
                        });
                    let (default_background, resolved_styles) = match reused {
                        Some(reused) => reused,
                        None => {
                            let (default_background, resolved_styles) =
                                self.resolve_styles(chunk, character, x, layer);
                            resolved_run = Some((
                                character.styles.clone(),
                                default_background,
                                resolved_styles.clone(),
                            ));
                            (default_background, resolved_styles)
                        },
                    };
                    let transparent =
                        character.character == ' ' && combining.is_empty() && default_background;
                    let mut resolved = character.clone();
                    resolved.styles = resolved_styles;
                    for column in covered {
                        let index = column - self.left;
                        if row.len() <= index {
                            row.resize_with(index + 1, || None);
                        }
                        row[index] = Some(Cell {
                            character: resolved.clone(),
                            combining: combining.clone(),
                            start: x,
                            layer,
                            transparent,
                        });
                    }
                }
                x += width;
            }
        }
        self.layers.insert(layer, cells);
    }

    /// Styles seen through a panel of `opacity`: every color pulled toward the tint.
    fn blend_styles(&self, mut styles: CharacterStyles, opacity: u8) -> RcCharacterStyles {
        let amount = f32::from(opacity) / 100.0;
        styles.foreground = Some(
            blend(
                color(styles.foreground, self.fg, None, &self.host_colors),
                self.tint,
                amount,
            )
            .into(),
        );
        styles.background = Some(
            blend(
                color(styles.background, self.bg, None, &self.host_colors),
                self.tint,
                amount,
            )
            .into(),
        );
        if let Some(underline) = styles.underline_color.filter(|c| *c != AnsiCode::Reset) {
            styles.underline_color = Some(
                blend(
                    color(Some(underline), self.fg, None, &self.host_colors),
                    self.tint,
                    amount,
                )
                .into(),
            );
        }
        styles.into()
    }

    pub(super) fn finish(mut self) -> Vec<CharacterChunk> {
        let mut composed = Cells::new();
        // Shared by every bare cell, so the blend below can reuse one result for all of them.
        let bare_styles: RcCharacterStyles = DEFAULT_STYLES
            .foreground(Some(self.fg.into()))
            .background(Some(self.bg.into()))
            .into();
        for (layer, cells) in std::mem::take(&mut self.layers) {
            let opacity = self
                .overlays
                .iter()
                .find(|(z, _, _)| *z == layer)
                .map(|(_, _, opacity)| *opacity);
            // The blend depends only on the styles beneath and this layer's opacity, and
            // neighbouring cells usually share their styles.
            let mut blended_run: Option<(RcCharacterStyles, RcCharacterStyles)> = None;
            for (y, cells) in cells {
                let row = composed.entry(y).or_default();
                row.resize_with(row.len().max(cells.len()), || None);
                for (index, cell) in cells.into_iter().enumerate() {
                    let Some(mut cell) = cell else { continue };
                    if let Some(opacity) = opacity.filter(|_| cell.transparent) {
                        cell = row[index].take().unwrap_or_else(|| Cell {
                            character: TerminalCharacter::new_styled(' ', bare_styles.clone()),
                            combining: vec![],
                            start: index + self.left,
                            layer: 0,
                            transparent: false,
                        });
                        // Kitty images remain rectangularly clipped, including their placeholder text.
                        if cell.character.character == PLACEHOLDER {
                            cell.character = EMPTY_TERMINAL_CHARACTER;
                            cell.combining.clear();
                        }
                        let blended = match &blended_run {
                            Some((source, blended))
                                if same_styles(source, &cell.character.styles) =>
                            {
                                blended.clone()
                            },
                            _ => {
                                let blended = self.blend_styles(*cell.character.styles, opacity);
                                blended_run =
                                    Some((cell.character.styles.clone(), blended.clone()));
                                blended
                            },
                        };
                        cell.character.styles = blended;
                    }
                    row[index] = Some(cell);
                }
            }
        }
        let mut chunks: Vec<CharacterChunk> = vec![];
        let mut end = None;
        for (&y, row) in &composed {
            for (index, cell) in row.iter().enumerate() {
                let Some(cell) = cell else { continue };
                let x = index + self.left;
                if end.is_some_and(|(row, column)| row == y && x < column) {
                    continue;
                }
                // Upper opaque cells mask lower composition, but are already emitted whole.
                if !self
                    .overlays
                    .iter()
                    .any(|(z, rect, _)| *z >= cell.layer && rect.contains(x, y))
                {
                    continue;
                }
                let width = cell.character.width();
                // A wide glyph needs its whole span and identical tint in every cell. Otherwise each
                // surviving cell becomes a space; a covered lead must never leave an orphan tail.
                let intact = x == cell.start
                    && (width == 1
                        || (index..index + width).all(|column| {
                            row.get(column)
                                .and_then(Option::as_ref)
                                .is_some_and(|other| {
                                    other.start == x
                                        && other.layer == cell.layer
                                        && other.character == cell.character
                                })
                        }));
                let character = if intact {
                    cell.character.clone()
                } else {
                    TerminalCharacter::new_styled(' ', cell.character.styles.clone())
                };
                if end != Some((y, x)) {
                    chunks.push(CharacterChunk::new(vec![], x, y));
                }
                end = Some((y, x + character.width()));
                let Some(chunk) = chunks.last_mut() else {
                    continue;
                };
                chunk.terminal_characters.push(character.clone());
                if intact {
                    chunk
                        .terminal_characters
                        .extend(cell.combining.iter().map(|c| {
                            TerminalCharacter::new_styled(c.character, character.styles.clone())
                        }));
                }
            }
        }
        chunks
    }
}
