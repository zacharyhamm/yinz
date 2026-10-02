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

impl Composition {
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

    pub(super) fn add(&mut self, chunks: &[CharacterChunk], layer: usize) {
        let cells = self.layers.entry(layer).or_default();
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
                    let styles = adjust_styles_for_possible_selection(
                        chunk.selection_and_colors(),
                        *character.styles,
                        chunk.y,
                        x,
                    );
                    let default_background =
                        matches!(styles.background, None | Some(AnsiCode::Reset))
                            && styles.reverse != Some(AnsiCode::On)
                            && chunk.pane_default_bg.is_none();
                    let transparent =
                        character.character == ' ' && combining.is_empty() && default_background;
                    let mut styles = adjust_styles_for_custom_bg_fg(
                        styles,
                        chunk.pane_default_fg,
                        chunk.pane_default_bg,
                    );
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
                    let mut resolved = character.clone();
                    resolved.styles = styles.into();
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
    }

    pub(super) fn finish(self) -> Vec<CharacterChunk> {
        let mut composed = Cells::new();
        for (layer, cells) in self.layers {
            let opacity = self
                .overlays
                .iter()
                .find(|(z, _, _)| *z == layer)
                .map(|(_, _, opacity)| *opacity);
            for (y, cells) in cells {
                let row = composed.entry(y).or_default();
                row.resize_with(row.len().max(cells.len()), || None);
                for (index, cell) in cells.into_iter().enumerate() {
                    let Some(mut cell) = cell else { continue };
                    if let Some(opacity) = opacity.filter(|_| cell.transparent) {
                        cell = row[index].take().unwrap_or_else(|| Cell {
                            character: TerminalCharacter::new_styled(
                                ' ',
                                DEFAULT_STYLES
                                    .foreground(Some(self.fg.into()))
                                    .background(Some(self.bg.into()))
                                    .into(),
                            ),
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
                        let mut styles = *cell.character.styles;
                        styles.foreground = Some(
                            blend(
                                color(styles.foreground, self.fg, None, &self.host_colors),
                                self.tint,
                                f32::from(opacity) / 100.0,
                            )
                            .into(),
                        );
                        styles.background = Some(
                            blend(
                                color(styles.background, self.bg, None, &self.host_colors),
                                self.tint,
                                f32::from(opacity) / 100.0,
                            )
                            .into(),
                        );
                        if let Some(underline) =
                            styles.underline_color.filter(|c| *c != AnsiCode::Reset)
                        {
                            styles.underline_color = Some(
                                blend(
                                    color(Some(underline), self.fg, None, &self.host_colors),
                                    self.tint,
                                    f32::from(opacity) / 100.0,
                                )
                                .into(),
                            );
                        }
                        cell.character.styles = styles.into();
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
