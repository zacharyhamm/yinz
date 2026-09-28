use crate::{line::tab_separator, LinePart};
use ansi_term::{ANSIString, ANSIStrings};
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::*;
use yinz_tile_utils::{ellipsize, style};

fn cursors<'a>(
    focused_clients: &'a [ClientId],
    multiplayer_colors: MultiplayerColors,
) -> (Vec<ANSIString<'a>>, usize) {
    // cursor section, text length
    let mut len = 0;
    let mut cursors = vec![];
    for client_id in focused_clients.iter() {
        if let Some(color) = client_id_to_colors(*client_id, multiplayer_colors) {
            cursors.push(style!(color.1, color.0).paint(" "));
            len += 1;
        }
    }
    (cursors, len)
}

pub fn render_tab(
    text: String,
    tab: &TabInfo,
    is_alternate_tab: bool,
    is_hovered: bool,
    palette: Styling,
    separator: &str,
    dimmed: bool,
) -> LinePart {
    let focused_clients = tab.other_focused_clients.as_slice();
    let separator_width = separator.width();

    let alternate_tab_color = if is_alternate_tab {
        palette.ribbon_unselected.emphasis_1
    } else {
        palette.ribbon_unselected.background
    };
    let background_color = if tab.active {
        palette.ribbon_selected.background
    } else if is_hovered {
        palette.ribbon_unselected.emphasis_1
    } else if is_alternate_tab {
        alternate_tab_color
    } else {
        palette.ribbon_unselected.background
    };
    let foreground_color = if tab.is_flashing_bell {
        if tab.active {
            palette.ribbon_selected.emphasis_3
        } else {
            palette.ribbon_unselected.emphasis_3
        }
    } else if tab.active {
        palette.ribbon_selected.base
    } else {
        palette.ribbon_unselected.base
    };

    let separator_fill_color = palette.text_unselected.background;
    let background_color = if dimmed {
        palette.ribbon_unselected.background
    } else {
        background_color
    };
    let text_style = if dimmed {
        style!(palette.ribbon_unselected.base, background_color).italic()
    } else {
        style!(foreground_color, background_color).bold()
    };
    let left_separator = style!(separator_fill_color, background_color).paint(separator);
    let right_separator_style = style!(background_color, separator_fill_color);
    let mut tab_text_len = text.width() + (separator_width * 2) + 2; // +2 for padding
    let tab_styled_text = text_style.paint(format!(" {} ", text));

    let right_separator = right_separator_style.paint(separator);
    let tab_styled_text = if !focused_clients.is_empty() {
        let (cursor_section, extra_length) =
            cursors(focused_clients, palette.multiplayer_user_colors);
        tab_text_len += extra_length + 2; // 2 for cursor_beginning and cursor_end
        let mut s = String::new();
        let cursor_beginning = text_style.paint("[").to_string();
        let cursor_section = ANSIStrings(&cursor_section).to_string();
        let cursor_end = text_style.paint("]").to_string();
        s.push_str(&left_separator.to_string());
        s.push_str(&tab_styled_text.to_string());
        s.push_str(&cursor_beginning);
        s.push_str(&cursor_section);
        s.push_str(&cursor_end);
        s.push_str(&right_separator.to_string());
        s
    } else {
        ANSIStrings(&[left_separator, tab_styled_text, right_separator]).to_string()
    };

    LinePart {
        part: tab_styled_text,
        len: tab_text_len,
        tab_index: Some(tab.position),
    }
}

/// `tabname` decorated with the tab's state and cut to fit `max_name_width` columns.
///
/// The state markers are worked out first and charged to the budget, so that a tab whose title is
/// already at the cap does not grow past it by acquiring a "(FULLSCREEN)" - the marker is the part
/// the reader cannot reconstruct, so it is the name that gives way.
#[allow(clippy::too_many_arguments)]
pub fn tab_style(
    tabname: String,
    tab: &TabInfo,
    mut is_alternate_tab: bool,
    is_hovered: bool,
    palette: Styling,
    capabilities: PluginCapabilities,
    dimmed: bool,
    max_name_width: usize,
) -> LinePart {
    let separator = tab_separator(capabilities);

    let mut suffix = String::new();
    if tab.is_fullscreen_active {
        suffix.push_str(" (FULLSCREEN)");
    } else if tab.is_sync_panes_active {
        suffix.push_str(" (SYNC)");
    }
    if tab.has_bell_notification || tab.is_flashing_bell {
        suffix.push_str(" [!]");
    }
    let tabname = format!(
        "{}{}",
        ellipsize(&tabname, max_name_width.saturating_sub(suffix.width())),
        suffix
    );
    // we only color alternate tabs differently if we can't use the arrow fonts to separate them
    if !capabilities.arrow_fonts {
        is_alternate_tab = false;
    }

    render_tab(
        tabname,
        tab,
        is_alternate_tab,
        is_hovered,
        palette,
        separator,
        dimmed,
    )
}

pub(crate) fn get_tab_to_focus(
    tab_line: &[LinePart],
    active_tab_idx: TabNumber,
    mouse_click_col: usize,
) -> Option<TabNumber> {
    let clicked_line_part = get_clicked_line_part(tab_line, mouse_click_col)?;
    let clicked_tab_idx = clicked_line_part.tab_index?;
    // tabs are indexed starting from 1 so we need to add 1
    let clicked_tab_idx = TabNumber::try_from(clicked_tab_idx).ok()?;
    if clicked_tab_idx != active_tab_idx {
        return Some(clicked_tab_idx);
    }
    None
}

pub(crate) fn get_clicked_line_part(
    tab_line: &[LinePart],
    mouse_click_col: usize,
) -> Option<&LinePart> {
    let mut len = 0;
    for tab_line_part in tab_line {
        if mouse_click_col >= len && mouse_click_col < len + tab_line_part.len {
            return Some(tab_line_part);
        }
        len += tab_line_part.len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capabilities that turn the arrow separators off, so a `LinePart`'s `len` is just the
    /// name plus its two padding spaces and the test can reason about widths directly.
    fn no_arrow_fonts() -> PluginCapabilities {
        PluginCapabilities {
            arrow_fonts: true, // the flag reads "arrow fonts are unavailable"
            ..Default::default()
        }
    }

    fn rendered(tabname: &str, tab: &TabInfo, max_name_width: usize) -> LinePart {
        tab_style(
            tabname.to_string(),
            tab,
            false,
            false,
            Styling::default(),
            no_arrow_fonts(),
            false,
            max_name_width,
        )
    }

    #[test]
    fn a_name_that_fits_is_left_alone() {
        let part = rendered("short", &TabInfo::default(), 40);
        assert!(part.part.contains("short"));
        assert_eq!(part.len, "short".width() + 2);
    }

    #[test]
    fn a_long_name_is_cut_to_the_budget() {
        let long = "a".repeat(120);
        let part = rendered(&long, &TabInfo::default(), 40);
        assert!(part.part.contains('…'));
        assert_eq!(part.len, 40 + 2);
    }

    #[test]
    fn the_state_marker_survives_a_cut_name() {
        let long = "a".repeat(120);
        let tab = TabInfo {
            is_fullscreen_active: true,
            ..Default::default()
        };
        let part = rendered(&long, &tab, 40);
        assert!(part.part.contains("(FULLSCREEN)"));
        // the marker is charged to the budget rather than added on top of it
        assert_eq!(part.len, 40 + 2);
    }

    #[test]
    fn a_bell_marker_survives_too() {
        let long = "a".repeat(120);
        let tab = TabInfo {
            has_bell_notification: true,
            ..Default::default()
        };
        let part = rendered(&long, &tab, 20);
        assert!(part.part.contains("[!]"));
        assert_eq!(part.len, 20 + 2);
    }

    #[test]
    fn a_wide_name_is_measured_in_columns_not_characters() {
        // 19 two-column characters plus the ellipsis, not 39 characters: a wide character cannot
        // straddle the last column, so the budget is met from below rather than exactly
        let part = rendered(&"日".repeat(60), &TabInfo::default(), 40);
        assert_eq!(part.len, 39 + 2);
    }
}
