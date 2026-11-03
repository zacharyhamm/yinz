//! Versioned semantic terminal state. This is independent of the bounded ANSI warp preview.
use super::*;
use crate::agent_theme::SourcePalette;
use crate::panes::terminal_character::LinkAnchor;
use serde::{Deserialize, Serialize};
use yinz_types::agent::theme::AgentThemeSnapshot;

const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Snapshot {
    format: u32,
    lines_above: Vec<RowState>,
    viewport: Vec<RowState>,
    lines_below: Vec<RowState>,
    alternate: Option<AlternateState>,
    palettes: Vec<Option<AgentThemeSnapshot>>,
    active_palette: usize,
    links: LinkHandler,
    changed_colors: Option<Vec<Option<AnsiCode>>>,
    horizontal_tabstops: BTreeSet<usize>,
    cursor: Cursor,
    cursor_is_hidden: bool,
    saved_cursor_position: Option<Cursor>,
    scroll_region: (usize, usize),
    active_charset: CharsetIndex,
    preceding_char: Option<TerminalCharacter>,
    title_stack: Vec<String>,
    cursor_key_mode: bool,
    bracketed_paste_mode: bool,
    erasure_mode: bool,
    sixel_scrolling: bool,
    insert_mode: bool,
    disable_linewrap: bool,
    new_line_mode: bool,
    width: usize,
    height: usize,
    selection: Selection,
    agent_launch: Option<yinz_types::agent::theme::AgentThemeLaunch>,
    detected_theme_interval: Option<String>,
    copied_text_selection: Option<Selection>,
    text_selection_active: bool,
    title: Option<String>,
    is_scrolled: bool,
    ring_bell: bool,
    scrollback_buffer_lines: usize,
    mouse_mode: MouseMode,
    mouse_tracking: MouseTracking,
    focus_event_tracking: bool,
    color_palette_notification_enabled: bool,
    search_results: SearchResult,
    notification_tracker: NotificationTracker,
    style: Style,
    debug: bool,
    arrow_fonts: bool,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    supports_kitty_keyboard_protocol: bool,
    explicitly_disable_kitty_keyboard_protocol: bool,
    hyperlink_tracker: HyperlinkTracker,
    pane_default_fg: Option<(u8, u8, u8)>,
    pane_default_bg: Option<(u8, u8, u8)>,
    application_fg: Option<(u8, u8, u8)>,
    application_bg: Option<(u8, u8, u8)>,
    osc133_markers_seen: bool,
    osc133_command_selection: bool,
    word_separators: String,
    osc7_payload: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct AlternateState {
    lines_above: Vec<RowState>,
    viewport: Vec<RowState>,
    cursor: Cursor,
    supports_kitty_keyboard_protocol: bool,
}
type PlaceholderState = (usize, Option<u16>, Option<u16>, Option<u16>);

#[derive(Serialize, Deserialize)]
struct RowState {
    columns: VecDeque<TerminalCharacter>,
    canonical: bool,
    bg_color: Option<AnsiCode>,
    markers: Vec<row::Osc133Marker>,
    placeholders: Vec<PlaceholderState>,
    palettes: Vec<(std::ops::Range<usize>, usize)>,
    write_palette: Option<usize>,
}
fn palette_id(palette: &Rc<SourcePalette>, palettes: &mut Vec<Rc<SourcePalette>>) -> usize {
    if let Some(index) = palettes
        .iter()
        .position(|existing| Rc::ptr_eq(existing, palette))
    {
        return index;
    }
    palettes.push(palette.clone());
    palettes.len() - 1
}
impl RowState {
    fn capture(row: &Row, palettes: &mut Vec<Rc<SourcePalette>>) -> Self {
        Self {
            columns: row.columns.clone(),
            canonical: row.is_canonical,
            bg_color: row.bg_color,
            markers: row.osc133_markers.clone(),
            placeholders: row
                .placeholder_marks
                .iter()
                .map(|mark| {
                    (
                        mark.column,
                        mark.row_in_image,
                        mark.column_in_image,
                        mark.id_most_significant_byte,
                    )
                })
                .collect(),
            palettes: row
                .palettes
                .migration_spans()
                .map(|(range, palette)| (range, palette_id(palette, palettes)))
                .collect(),
            write_palette: row
                .write_palette
                .as_ref()
                .map(|palette| palette_id(palette, palettes)),
        }
    }
    fn restore(mut self, palettes: &[Rc<SourcePalette>], links: &[u32]) -> Result<Row> {
        for character in &mut self.columns {
            remap_styles(&mut character.styles, links)?;
        }
        let mut row = Row::new();
        row.columns = self.columns;
        row.is_canonical = self.canonical;
        row.bg_color = self.bg_color;
        row.osc133_markers = self.markers;
        row.placeholder_marks = self
            .placeholders
            .into_iter()
            .map(
                |(column, row_in_image, column_in_image, id_most_significant_byte)| {
                    super::super::kitty_graphics::PlaceholderMark {
                        column,
                        row_in_image,
                        column_in_image,
                        id_most_significant_byte,
                    }
                },
            )
            .collect();
        for (range, index) in self.palettes {
            row.palettes.set(
                range,
                Some(
                    palettes
                        .get(index)
                        .context("unknown terminal source palette")?,
                ),
            );
        }
        row.write_palette = self
            .write_palette
            .map(|index| {
                palettes
                    .get(index)
                    .cloned()
                    .context("unknown terminal write palette")
            })
            .transpose()?;
        Ok(row)
    }
}
fn remap_styles(styles: &mut RcCharacterStyles, links: &[u32]) -> Result<()> {
    if let Some(LinkAnchor::Start(index)) = styles.link_anchor {
        let destination = *links
            .get(index as usize)
            .context("unknown terminal hyperlink")?;
        styles.update(|styles| styles.link_anchor = Some(LinkAnchor::Start(destination)));
    }
    Ok(())
}
fn rows(source: &VecDeque<Row>, palettes: &mut Vec<Rc<SourcePalette>>) -> Vec<RowState> {
    source
        .iter()
        .map(|row| RowState::capture(row, palettes))
        .collect()
}
fn restore_rows(
    rows: Vec<RowState>,
    palettes: &[Rc<SourcePalette>],
    links: &[u32],
) -> Result<VecDeque<Row>> {
    rows.into_iter()
        .map(|row| row.restore(palettes, links))
        .collect()
}
fn no_graphics(sixel: &SixelGrid, kitty: &KittyGrid) -> bool {
    !sixel.is_parsing() && sixel.image_coordinates().next().is_none() && kitty.is_migration_empty()
}
impl Grid {
    pub fn migration_state(&self) -> Result<serde_json::Value> {
        anyhow::ensure!(
            no_graphics(&self.sixel_grid, &self.kitty_grid)
                && self
                    .alternate_screen_state
                    .as_ref()
                    .is_none_or(|alternate| no_graphics(
                        &alternate.sixel_grid,
                        &alternate.kitty_grid
                    ))
                && !self.kitty_parser.has_pending(),
            "terminal has graphics that this migration format cannot transfer"
        );
        anyhow::ensure!(!self.lock_renders && self.ui_component_bytes.is_none() && self.nested_frame_bytes.is_none() && self.xtgettcap_bytes.is_none(),
            "terminal has an incomplete frame or device-control sequence; retry when output settles");
        anyhow::ensure!(
            self.pending_forwarded_queries.is_empty()
                && self.agent_color_queries.is_empty()
                && self.pending_messages_to_pty.is_empty()
                && self.pending_nested_session_messages.is_empty()
                && self.pending_clipboard_updates.is_empty()
                && self.pending_desktop_notifications.is_empty()
                && self.pending_agent_theme_updates.is_empty()
                && self.pending_osc7_cwd.is_none()
                && self.history_replay.is_none(),
            "terminal has pending effects or host replies; retry after they are delivered"
        );
        let mut palettes = vec![];
        let active_palette = palette_id(&self.agent_theme, &mut palettes);
        let lines_above = rows(&self.lines_above, &mut palettes);
        let viewport = rows(&self.viewport, &mut palettes);
        let lines_below = rows(&self.lines_below, &mut palettes);
        let alternate = self
            .alternate_screen_state
            .as_ref()
            .map(|alternate| AlternateState {
                lines_above: rows(&alternate.lines_above, &mut palettes),
                viewport: rows(&alternate.viewport, &mut palettes),
                cursor: alternate.cursor.clone(),
                supports_kitty_keyboard_protocol: alternate.supports_kitty_keyboard_protocol,
            });
        let snapshot = Snapshot {
            format: FORMAT,
            lines_above,
            viewport,
            lines_below,
            alternate,
            palettes: palettes
                .iter()
                .map(|palette| palette.snapshot().map(|snapshot| snapshot.clone()))
                .collect(),
            active_palette,
            links: self.link_handler.borrow().clone(),
            changed_colors: self.changed_colors.map(|colors| colors.to_vec()),
            horizontal_tabstops: self.horizontal_tabstops.clone(),
            cursor: self.cursor.clone(),
            cursor_is_hidden: self.cursor_is_hidden,
            saved_cursor_position: self.saved_cursor_position.clone(),
            scroll_region: self.scroll_region,
            active_charset: self.active_charset,
            preceding_char: self.preceding_char.clone(),
            title_stack: self.title_stack.clone(),
            cursor_key_mode: self.cursor_key_mode,
            bracketed_paste_mode: self.bracketed_paste_mode,
            erasure_mode: self.erasure_mode,
            sixel_scrolling: self.sixel_scrolling,
            insert_mode: self.insert_mode,
            disable_linewrap: self.disable_linewrap,
            new_line_mode: self.new_line_mode,
            width: self.width,
            height: self.height,
            selection: self.selection,
            agent_launch: self.agent_launch.clone(),
            detected_theme_interval: self.detected_theme_interval.clone(),
            copied_text_selection: self.copied_text_selection,
            text_selection_active: self.text_selection_active,
            title: self.title.clone(),
            is_scrolled: self.is_scrolled,
            ring_bell: self.ring_bell,
            scrollback_buffer_lines: self.scrollback_buffer_lines,
            mouse_mode: self.mouse_mode.clone(),
            mouse_tracking: self.mouse_tracking.clone(),
            focus_event_tracking: self.focus_event_tracking,
            color_palette_notification_enabled: self.color_palette_notification_enabled,
            search_results: self.search_results.clone(),
            notification_tracker: self.notification_tracker.clone(),
            style: self.style,
            debug: self.debug,
            arrow_fonts: self.arrow_fonts,
            styled_underlines: self.styled_underlines,
            osc8_hyperlinks: self.osc8_hyperlinks,
            supports_kitty_keyboard_protocol: self.supports_kitty_keyboard_protocol,
            explicitly_disable_kitty_keyboard_protocol: self
                .explicitly_disable_kitty_keyboard_protocol,
            hyperlink_tracker: self.hyperlink_tracker.clone(),
            pane_default_fg: self.pane_default_fg,
            pane_default_bg: self.pane_default_bg,
            application_fg: self.application_fg,
            application_bg: self.application_bg,
            osc133_markers_seen: self.osc133_markers_seen,
            osc133_command_selection: self.osc133_command_selection,
            word_separators: self.word_separators.clone(),
            osc7_payload: self.osc7_payload.clone(),
        };
        serde_json::to_value(snapshot).context("encode semantic terminal snapshot")
    }
    pub fn restore_migration_state(&mut self, value: serde_json::Value) -> Result<()> {
        let mut state: Snapshot =
            serde_json::from_value(value).context("decode semantic terminal snapshot")?;
        anyhow::ensure!(
            state.format == FORMAT,
            "unsupported terminal migration format"
        );
        anyhow::ensure!(
            state.width > 0
                && state.height > 0
                && state.cursor.x <= state.width
                && state.cursor.y < state.height
                && state.scroll_region.0 <= state.scroll_region.1
                && state.scroll_region.1 < state.height,
            "invalid terminal snapshot dimensions or cursor"
        );
        let palettes: Vec<_> = state
            .palettes
            .into_iter()
            .map(|snapshot| Rc::new(SourcePalette::new(snapshot, state.style)))
            .collect();
        let active = palettes
            .get(state.active_palette)
            .cloned()
            .context("unknown active terminal palette")?;
        let mut merged_links = self.link_handler.borrow().clone();
        let links = merged_links
            .merge_migration(&state.links)
            .map_err(anyhow::Error::msg)?;
        remap_styles(&mut state.cursor.pending_styles, &links)?;
        if let Some(cursor) = &mut state.saved_cursor_position {
            remap_styles(&mut cursor.pending_styles, &links)?;
        }
        if let Some(character) = &mut state.preceding_char {
            remap_styles(&mut character.styles, &links)?;
        }
        let lines_above = restore_rows(state.lines_above, &palettes, &links)?;
        let viewport = restore_rows(state.viewport, &palettes, &links)?;
        let lines_below = restore_rows(state.lines_below, &palettes, &links)?;
        let alternate = state
            .alternate
            .map(|mut alternate| -> Result<AlternateScreenState> {
                remap_styles(&mut alternate.cursor.pending_styles, &links)?;
                Ok(AlternateScreenState::new(
                    restore_rows(alternate.lines_above, &palettes, &links)?,
                    restore_rows(alternate.viewport, &palettes, &links)?,
                    alternate.cursor,
                    SixelGrid::new(
                        self.character_cell_size.clone(),
                        self.sixel_grid.sixel_image_store.clone(),
                    ),
                    KittyGrid::new(
                        self.character_cell_size.clone(),
                        self.kitty_grid.kitty_image_store.clone(),
                    ),
                    alternate.supports_kitty_keyboard_protocol,
                ))
            })
            .transpose()?;
        let changed_colors = state
            .changed_colors
            .map(|colors| {
                colors
                    .try_into()
                    .map_err(|_| anyhow!("invalid terminal palette length"))
            })
            .transpose()?;
        // All validation and shared-link allocation above precede mutation of the live grid.
        *self.link_handler.borrow_mut() = merged_links;
        self.lines_above = lines_above;
        self.viewport = viewport;
        self.lines_below = lines_below;
        self.alternate_screen_state = alternate;
        self.changed_colors = changed_colors;
        self.agent_theme = active;
        self.agent_palettes = palettes.iter().map(Rc::downgrade).collect();
        self.horizontal_tabstops = state.horizontal_tabstops;
        self.cursor = state.cursor;
        self.cursor_is_hidden = state.cursor_is_hidden;
        self.saved_cursor_position = state.saved_cursor_position;
        self.scroll_region = state.scroll_region;
        self.active_charset = state.active_charset;
        self.preceding_char = state.preceding_char;
        self.title_stack = state.title_stack;
        self.cursor_key_mode = state.cursor_key_mode;
        self.bracketed_paste_mode = state.bracketed_paste_mode;
        self.erasure_mode = state.erasure_mode;
        self.sixel_scrolling = state.sixel_scrolling;
        self.insert_mode = state.insert_mode;
        self.disable_linewrap = state.disable_linewrap;
        self.new_line_mode = state.new_line_mode;
        self.width = state.width;
        self.height = state.height;
        self.selection = state.selection;
        self.agent_launch = state.agent_launch;
        self.detected_theme_interval = state.detected_theme_interval;
        self.copied_text_selection = state.copied_text_selection;
        self.text_selection_active = state.text_selection_active;
        self.title = state.title;
        self.is_scrolled = state.is_scrolled;
        self.ring_bell = state.ring_bell;
        self.scrollback_buffer_lines = state.scrollback_buffer_lines;
        self.mouse_mode = state.mouse_mode;
        self.mouse_tracking = state.mouse_tracking;
        self.focus_event_tracking = state.focus_event_tracking;
        self.color_palette_notification_enabled = state.color_palette_notification_enabled;
        self.search_results = state.search_results;
        self.notification_tracker = state.notification_tracker;
        self.style = state.style;
        self.debug = state.debug;
        self.arrow_fonts = state.arrow_fonts;
        self.styled_underlines = state.styled_underlines;
        self.osc8_hyperlinks = state.osc8_hyperlinks;
        self.supports_kitty_keyboard_protocol = state.supports_kitty_keyboard_protocol;
        self.explicitly_disable_kitty_keyboard_protocol =
            state.explicitly_disable_kitty_keyboard_protocol;
        self.hyperlink_tracker = state.hyperlink_tracker;
        self.pane_default_fg = state.pane_default_fg;
        self.pane_default_bg = state.pane_default_bg;
        self.application_fg = state.application_fg;
        self.application_bg = state.application_bg;
        self.osc133_markers_seen = state.osc133_markers_seen;
        self.osc133_command_selection = state.osc133_command_selection;
        // The source's short-lived feedback timer is not transferred. Keeping its
        // highlight would otherwise leave a permanent flash in the replacement.
        self.command_output_flash = None;
        self.word_separators = state.word_separators;
        self.osc7_payload = state.osc7_payload;
        self.output_buffer = Default::default();
        self.completed_text.clear();
        self.completed_sixel.clear();
        self.completed_kitty.clear();
        self.clear_viewport_before_rendering = true;
        self.should_render = true;
        self.render_full_viewport();
        Ok(())
    }
}

#[cfg(test)]
#[path = "../unit/grid_migration.rs"]
mod tests;
