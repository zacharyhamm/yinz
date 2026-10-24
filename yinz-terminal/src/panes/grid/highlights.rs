//! Plugin regex matching, hover tooltips, and highlight styles.

use super::{Grid, MouseTracking, Row};
use crate::output::HighlightSelection;
use crate::panes::terminal_character::AnsiCode;
use crate::panes::Selection;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::str;
use unicode_width::UnicodeWidthChar;
use yinz_types::data::HighlightLayer;
use yinz_types::data::HighlightStyle;
use yinz_types::data::PaletteColor;
use yinz_types::data::RegexHighlight;
use yinz_types::internal_ids::PluginId;
use yinz_types::position::Position;
use yinz_types::style::Style;

/// A logical line rebuilt from wrapped rows: `(canonical row index, row count, text, per-row
/// byte-offset boundaries)`.
type LogicalLine = (usize, usize, String, Vec<(usize, usize)>);

/// Find the canonical root row for a logical line group containing `row_idx`,
/// collect the non-canonical tail rows, and build the concatenated text with
/// a byte-offset boundary table.
///
/// Returns `(canonical_idx, group_len, text, boundaries)` where `group_len`
/// is the total number of viewport rows in the group (1 + tail count).
/// Returns `None` if `row_idx` is out of bounds.
fn collect_and_build_logical_line(viewport: &VecDeque<Row>, row_idx: usize) -> Option<LogicalLine> {
    if row_idx >= viewport.len() {
        return None;
    }
    // Walk backward to find the canonical root.
    let mut canonical = row_idx;
    while canonical > 0 {
        match viewport.get(canonical) {
            Some(r) if !r.is_canonical => canonical -= 1,
            _ => break,
        }
    }
    let canonical_row = viewport.get(canonical)?;
    // Collect non-canonical tail rows.
    let mut tail_count = 0;
    loop {
        let tail_idx = canonical + tail_count + 1;
        match viewport.get(tail_idx) {
            Some(r) if !r.is_canonical => tail_count += 1,
            _ => break,
        }
    }
    let group_len = 1 + tail_count;
    // Build concatenated text and boundary table.
    let mut text = String::new();
    let mut boundaries: Vec<(usize, usize)> = Vec::with_capacity(group_len);
    boundaries.push((canonical, 0));
    for ch in &canonical_row.columns {
        text.push(ch.character);
    }
    for i in 0..tail_count {
        let idx = canonical + 1 + i;
        boundaries.push((idx, text.len()));
        if let Some(row) = viewport.get(idx) {
            for ch in &row.columns {
                text.push(ch.character);
            }
        }
    }
    Some((canonical, group_len, text, boundaries))
}

/// Map a byte offset in the concatenated logical-line string back to
/// (viewport_row_idx, display_column).
///
/// display_column is the sum of character.width() for all characters
/// before the target character in that row — NOT a char count or byte count.
/// This correctly handles wide characters (CJK, emoji).
///
/// `boundaries` is the table produced by `collect_and_build_logical_line`.
/// `viewport` is `&self.viewport`.
fn byte_offset_to_display_col(
    byte_offset: usize,
    boundaries: &[(usize, usize)],
    viewport: &VecDeque<Row>,
) -> Option<(usize, usize)> {
    if boundaries.is_empty() {
        return None;
    }
    // Find which row the byte offset falls in (last boundary whose byte_start <= offset).
    let boundary_idx = boundaries
        .partition_point(|&(_, byte_start)| byte_start <= byte_offset)
        .saturating_sub(1);
    let &(row_idx, row_byte_start) = boundaries.get(boundary_idx)?;
    let intra_byte_offset = byte_offset - row_byte_start;

    // Count display columns up to (but not including) the character at intra_byte_offset.
    let row = viewport.get(row_idx)?;
    let mut display_col = 0usize;
    let mut bytes_seen = 0usize;
    for ch in &row.columns {
        if bytes_seen >= intra_byte_offset {
            break;
        }
        bytes_seen += ch.character.len_utf8();
        display_col += ch.character.width().unwrap_or(1);
    }
    Some((row_idx, display_col))
}

/// Convert a regex match into a `Selection` spanning the matched display region.
/// Returns `None` if the boundary table or viewport lookup fails.
fn match_to_selection(
    mat: &regex::Match,
    boundaries: &[(usize, usize)],
    viewport: &VecDeque<Row>,
) -> Option<(Selection, usize, usize, usize, usize)> {
    let (start_row, start_col) = byte_offset_to_display_col(mat.start(), boundaries, viewport)?;
    let (end_row, end_col) = byte_offset_to_display_col(mat.end(), boundaries, viewport)?;
    let mut sel = Selection::default();
    sel.set_start_and_end_positions(
        Position::new(start_row as i32, start_col as u16),
        Position::new(end_row as i32, end_col as u16),
    );
    Some((sel, start_row, start_col, end_row, end_col))
}

/// Extract the effective match from a set of captures.
/// If capture group 1 exists, it is used (allowing patterns to include
/// context such as surrounding whitespace in the full match while
/// highlighting only the content in group 1). Otherwise the full match
/// (group 0) is returned.
fn highlight_match<'t>(captures: &regex::Captures<'t>) -> Option<regex::Match<'t>> {
    captures.get(1).or_else(|| captures.get(0))
}

pub(super) fn highlight_matches<'r, 't>(
    regex: &'r regex::Regex,
    text: &'t str,
) -> impl Iterator<Item = regex::Match<'t>> + 'r
where
    't: 'r,
{
    let mut pos = 0;
    std::iter::from_fn(move || {
        while pos <= text.len() {
            let captures = regex.captures_at(text, pos)?;
            let mat = highlight_match(&captures);
            let end = match (mat, captures.get(0)) {
                (Some(m), _) => m.end(),
                (None, Some(whole)) => whole.end(),
                (None, None) => return None,
            };
            pos = if end > pos && mat.is_none_or(|m| !m.is_empty()) {
                end
            } else {
                text[end..]
                    .chars()
                    .next()
                    .map(|c| end + c.len_utf8())
                    .unwrap_or(text.len() + 1)
            };
            if mat.is_some() {
                return mat;
            }
        }
        None
    })
}

/// Check whether a (row, col) position falls within a display span.
/// The span is inclusive at start and exclusive at end.
fn position_in_span(
    row: usize,
    col: usize,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) -> bool {
    let after_start = row > start_row || (row == start_row && col >= start_col);
    let before_end = row < end_row || (row == end_row && col < end_col);
    after_start && before_end
}

/// A compiled highlight entry for one plugin/pattern combination.
#[derive(Clone)]
pub struct CompiledHighlight {
    pub regex: regex::Regex,
    pub fg: Option<AnsiCode>,
    pub bg: Option<AnsiCode>,
    pub context: BTreeMap<String, String>,
    pub on_hover: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub tooltip_text: Option<String>,
    pub layer: HighlightLayer,
}

impl CompiledHighlight {
    /// Whether this highlight would produce any visible styling change.
    pub fn has_visual_effect(&self) -> bool {
        self.bg.is_some() || self.fg.is_some() || self.bold || self.italic || self.underline
    }
}

pub(super) fn resolve_highlight_colors(
    style_decl: &HighlightStyle,
    style: &Style,
) -> (Option<AnsiCode>, Option<AnsiCode>) {
    let palette_to_ansi = |c: PaletteColor| -> AnsiCode {
        match c {
            PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
            PaletteColor::EightBit(i) => AnsiCode::ColorIndex(i),
        }
    };
    let tu = &style.colors.text_unselected;
    let emphasis = [tu.emphasis_0, tu.emphasis_1, tu.emphasis_2, tu.emphasis_3];
    match style_decl {
        HighlightStyle::None => (None, None),
        HighlightStyle::Emphasis0 => (Some(palette_to_ansi(emphasis[0])), None),
        HighlightStyle::Emphasis1 => (Some(palette_to_ansi(emphasis[1])), None),
        HighlightStyle::Emphasis2 => (Some(palette_to_ansi(emphasis[2])), None),
        HighlightStyle::Emphasis3 => (Some(palette_to_ansi(emphasis[3])), None),
        HighlightStyle::BackgroundEmphasis0 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[0])),
        ),
        HighlightStyle::BackgroundEmphasis1 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[1])),
        ),
        HighlightStyle::BackgroundEmphasis2 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[2])),
        ),
        HighlightStyle::BackgroundEmphasis3 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[3])),
        ),
        HighlightStyle::CustomRgb { fg, bg } => {
            (fg.map(AnsiCode::RgbCode), bg.map(AnsiCode::RgbCode))
        },
        HighlightStyle::CustomIndex { fg, bg } => {
            (fg.map(AnsiCode::ColorIndex), bg.map(AnsiCode::ColorIndex))
        },
    }
}

/// The plugin highlight found under a position so far, ranked by its layer:
/// `(layer, plugin, pattern, matched text, context)`.
type RankedHighlight = (
    HighlightLayer,
    PluginId,
    String,
    String,
    BTreeMap<String, String>,
);

impl Grid {
    /// Called by the server-side handler for SetPaneRegexHighlights.
    /// Upserts highlights keyed by pattern string for the given plugin.
    pub fn set_plugin_regex_highlights(
        &mut self,
        plugin_id: PluginId,
        highlights: Vec<RegexHighlight>,
        style: &Style,
    ) {
        let slot = self.plugin_highlights.entry(plugin_id).or_default();
        for h in highlights {
            let (fg, bg) = resolve_highlight_colors(&h.style, style);
            match regex::Regex::new(&h.pattern) {
                Ok(regex) => {
                    // Upsert: replace existing entry with same pattern and on_hover flag, or push new
                    let on_hover = h.on_hover;
                    if let Some(existing) = slot
                        .iter_mut()
                        .find(|(p, c)| p == &h.pattern && c.on_hover == on_hover)
                    {
                        existing.1 = CompiledHighlight {
                            regex,
                            fg,
                            bg,
                            context: h.context,
                            on_hover: h.on_hover,
                            bold: h.bold,
                            italic: h.italic,
                            underline: h.underline,
                            tooltip_text: h.tooltip_text.clone(),
                            layer: h.layer,
                        };
                    } else {
                        slot.push((
                            h.pattern,
                            CompiledHighlight {
                                regex,
                                fg,
                                bg,
                                context: h.context,
                                on_hover: h.on_hover,
                                bold: h.bold,
                                italic: h.italic,
                                underline: h.underline,
                                tooltip_text: h.tooltip_text,
                                layer: h.layer,
                            },
                        ));
                    }
                },
                _ => {
                    log::warn!(
                        "Plugin {} supplied invalid regex: {:?}",
                        plugin_id,
                        h.pattern
                    );
                },
            }
        }
        self.output_buffer.update_all_lines();
        self.recompute_hover_tooltip();
    }

    /// Called by the server-side handler for ClearPaneHighlights.
    pub fn clear_plugin_highlights(&mut self, plugin_id: PluginId) {
        if self.plugin_highlights.remove(&plugin_id).is_some() {
            self.output_buffer.update_all_lines();
            self.recompute_hover_tooltip();
        }
    }

    /// Returns (plugin_id, pattern, matched_string, context) if the given
    /// pane-relative position falls inside any plugin highlight match, or None.
    /// Uses logical-line grouping so multi-line (wrapped) matches are detected.
    pub fn plugin_highlight_at(
        &self,
        position: &Position,
    ) -> Option<(PluginId, String, String, BTreeMap<String, String>)> {
        let click_row = position.line.get() as usize;
        let click_col = position.column.get();

        let (_canonical, _group_len, logical_text, boundaries) =
            collect_and_build_logical_line(&self.viewport, click_row)?;

        let mut best: Option<RankedHighlight> = None;
        for (plugin_id, pattern_map) in &self.plugin_highlights {
            for (pattern, compiled) in pattern_map {
                for mat in highlight_matches(&compiled.regex, &logical_text) {
                    if let Some((_sel, start_row, start_col, end_row, end_col)) =
                        match_to_selection(&mat, &boundaries, &self.viewport)
                        && position_in_span(
                            click_row, click_col, start_row, start_col, end_row, end_col,
                        )
                    {
                        let dominated = match &best {
                            Some((best_layer, ..)) => compiled.layer > *best_layer,
                            None => true,
                        };
                        if dominated {
                            best = Some((
                                compiled.layer,
                                *plugin_id,
                                pattern.clone(),
                                mat.as_str().to_string(),
                                compiled.context.clone(),
                            ));
                        }
                    }
                }
            }
        }
        best.map(|(_layer, plugin_id, pattern, matched, ctx)| (plugin_id, pattern, matched, ctx))
    }

    pub fn set_hover_position(&mut self, new_pos: Option<Position>) -> bool {
        if self.hover_position == new_pos {
            return false;
        }

        let has_hover_consumer = self
            .plugin_highlights
            .values()
            .any(|highlights| highlights.iter().any(|(_, c)| c.on_hover));

        if !has_hover_consumer {
            self.hover_position = new_pos;
            return false;
        }

        // Mark the canonical group containing the old hover row dirty.
        if let Some(old_pos) = self.hover_position {
            self.mark_logical_line_dirty(old_pos.line.get() as usize);
        }
        // Mark the canonical group containing the new hover row dirty.
        if let Some(new_pos_inner) = new_pos {
            self.mark_logical_line_dirty(new_pos_inner.line.get() as usize);
        }

        self.hover_position = new_pos;
        self.recompute_hover_tooltip();
        true
    }

    /// Mark all physical rows belonging to the logical line group that contains
    /// `row_idx` as dirty in the output buffer.
    pub(super) fn mark_logical_line_dirty(&mut self, row_idx: usize) {
        if let Some((canonical, group_len, _, _)) =
            collect_and_build_logical_line(&self.viewport, row_idx)
        {
            for r in canonical..canonical + group_len {
                self.output_buffer.update_line(r);
            }
        }
    }

    /// Recompute the cached hover tooltip from the current hover position and
    /// plugin highlights.  Called whenever the hover position, highlight set, or
    /// highlight clearing changes.
    fn recompute_hover_tooltip(&mut self) {
        self.cached_hover_tooltip = None;
        let hover_pos = match self.hover_position {
            Some(p) => p,
            None => return,
        };
        if self.mouse_tracking != MouseTracking::Off {
            return;
        }
        if self.plugin_highlights.is_empty() {
            return;
        }
        let hover_row = hover_pos.line.get() as usize;
        let hover_col = hover_pos.column.get();
        let (_canonical, _group_len, logical_text, boundaries) =
            match collect_and_build_logical_line(&self.viewport, hover_row) {
                Some(v) => v,
                None => return,
            };
        let mut best_layer: Option<HighlightLayer> = None;
        let mut best_tooltip: Option<String> = None;
        for pattern_map in self.plugin_highlights.values() {
            for (_pattern, compiled) in pattern_map {
                if !compiled.on_hover || compiled.tooltip_text.is_none() {
                    continue;
                }
                for mat in highlight_matches(&compiled.regex, &logical_text) {
                    if let Some((_sel, start_row, start_col, end_row, end_col)) =
                        match_to_selection(&mat, &boundaries, &self.viewport)
                        && position_in_span(
                            hover_row, hover_col, start_row, start_col, end_row, end_col,
                        )
                    {
                        let dominated = match best_layer {
                            Some(bl) => compiled.layer > bl,
                            None => true,
                        };
                        if dominated {
                            best_layer = Some(compiled.layer);
                            best_tooltip = compiled.tooltip_text.clone();
                        }
                    }
                }
            }
        }
        self.cached_hover_tooltip = best_tooltip;
    }

    /// Pre-compute plugin highlight selections across all logical line groups in
    /// the viewport.  Hover highlights are emitted first so they take priority
    /// when the cursor overlaps a match; non-hover highlights follow.
    pub(super) fn compute_plugin_highlight_selections(&self) -> Vec<HighlightSelection> {
        if self.plugin_highlights.is_empty() {
            return vec![];
        }
        let mut selections = Vec::new();
        let viewport_len = self.viewport.len();
        let mut ridx = 0;
        while ridx < viewport_len {
            let (_canonical, group_len, logical_text, boundaries) =
                match collect_and_build_logical_line(&self.viewport, ridx) {
                    Some(v) => v,
                    None => break,
                };

            // Hover highlights are pushed first so that `.find()` in
            // `adjust_styles_for_possible_selection` returns the hover
            // style when the cursor overlaps the match.  They are suppressed
            // when mouse tracking is active (events pass through to the app)
            // and on unfocused panes (hover_position is not set for those).
            if self.mouse_tracking == MouseTracking::Off
                && let Some(hover_pos) = self.hover_position
            {
                let hover_row = hover_pos.line.get() as usize;
                let group_end_row = ridx + group_len - 1;
                if hover_row >= ridx && hover_row <= group_end_row {
                    let hover_col = hover_pos.column.get();
                    for pattern_map in self.plugin_highlights.values() {
                        for (_pattern, compiled) in pattern_map {
                            if !compiled.on_hover || !compiled.has_visual_effect() {
                                continue;
                            }
                            for mat in highlight_matches(&compiled.regex, &logical_text) {
                                if let Some((sel, start_row, start_col, end_row, end_col)) =
                                    match_to_selection(&mat, &boundaries, &self.viewport)
                                    && position_in_span(
                                        hover_row, hover_col, start_row, start_col, end_row,
                                        end_col,
                                    )
                                {
                                    selections.push(HighlightSelection {
                                        selection: sel,
                                        bg: compiled.bg,
                                        fg: compiled.fg,
                                        bold: compiled.bold,
                                        italic: compiled.italic,
                                        underline: compiled.underline,
                                        layer: compiled.layer,
                                    });
                                    break; // only one match per pattern per hover
                                }
                            }
                        }
                    }
                }
            }

            // Non-hover highlights are pushed after hover so that hover
            // takes precedence for overlapping regions.
            for pattern_map in self.plugin_highlights.values() {
                for (_pattern, compiled) in pattern_map {
                    if compiled.on_hover || !compiled.has_visual_effect() {
                        continue;
                    }
                    for mat in highlight_matches(&compiled.regex, &logical_text) {
                        if let Some((sel, _, _, _, _)) =
                            match_to_selection(&mat, &boundaries, &self.viewport)
                        {
                            selections.push(HighlightSelection {
                                selection: sel,
                                bg: compiled.bg,
                                fg: compiled.fg,
                                bold: compiled.bold,
                                italic: compiled.italic,
                                underline: compiled.underline,
                                layer: compiled.layer,
                            });
                        }
                    }
                }
            }

            ridx += group_len; // advance past this logical line group
        }
        // Sort by layer priority (highest first) and within the same layer,
        // hover highlights before non-hover (hover entries were pushed first,
        // so a stable sort preserves their relative order).
        selections.sort_by(|a, b| {
            use std::cmp::Reverse;
            Reverse(a.layer).cmp(&Reverse(b.layer))
        });
        selections
    }
}
