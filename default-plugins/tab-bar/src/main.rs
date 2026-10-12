mod line;
mod tab;

use std::cmp::{max, min};
use std::collections::BTreeMap;
use yinz_tile::prelude::TabNumber;
use yinz_tile::prelude::TabPosition;

use tab::get_tab_to_focus;
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::*;

use crate::line::{tab_line, tab_line_prefix_width, tab_separator};
use crate::tab::tab_style;

/// The widest a tab name is ever allowed to be, however much room the bar has.
///
/// A lone pane lends the tab its own title (see `Tab::lent_pane_title` on the server), and
/// those run long - hostname, checkout prefix, then whatever the program set. Without a ceiling one
/// such tab would take the whole bar on a wide terminal and push every other tab behind a "+N".
const MAX_TAB_NAME_WIDTH: usize = 40;
/// A floor for the fitted budget. On a terminal too narrow to honour it the bar overflows anyway,
/// and a few characters of name read better there than a lone ellipsis.
const MIN_TAB_NAME_WIDTH: usize = 8;

#[derive(Debug, Default)]
pub struct LinePart {
    part: String,
    len: usize,
    tab_index: Option<TabPosition>,
}

impl LinePart {
    pub fn append(&mut self, to_append: &LinePart) {
        self.part.push_str(&to_append.part);
        self.len += to_append.len;
    }
}

#[derive(Default, Debug)]
struct State {
    tabs: Vec<TabInfo>,
    active_tab_idx: TabNumber,
    mode_info: ModeInfo,
    tab_line: Vec<LinePart>,
    hide_swap_layout_indication: bool,
    cached_keybinds: KeybindsVec,
    active_pane_scroll: Option<(usize, usize)>,
    new_tab_button_range: Option<(usize, usize)>,
    breadcrumb_range: Option<(usize, usize)>,
    hovered_tab_idx: Option<TabNumber>,
    hovered_new_tab_button: bool,
    hint_text: Option<BTreeMap<usize, StyledText>>,
    outstanding_hint_timeouts: usize,
    hostname: String,
}

static ARROW_SEPARATOR: &str = "";

register_plugin!(State);

impl State {
    /// How wide a single tab's name may be, given what else has to share the row.
    ///
    /// The active tab is rendered unconditionally (see `populate_tabs_in_tab_line`), so it is the
    /// one that would overflow the bar; sizing every name against what is left beside the prefix
    /// and the new-tab button is what keeps that from happening. Tabs other than the active one
    /// are dropped when they do not fit, so they need no budget of their own - but giving them the
    /// same one keeps the bar's widths even.
    fn max_tab_name_width(&self, bar_cols: usize, breadcrumb_ancestry: &[String]) -> usize {
        let session_name = if self.mode_info.style.hide_session_name {
            None
        } else {
            self.mode_info
                .session_name
                .as_ref()
                .map(|name| name.as_str())
        };
        let prefix_width = tab_line_prefix_width(
            &self.hostname,
            session_name,
            self.mode_info.style.colors,
            bar_cols,
            breadcrumb_ancestry,
        );
        let separator_width = tab_separator(self.mode_info.capabilities).width();
        let tab_chrome = separator_width * 2 + 2; // separators, and a space either side of the name
        let new_tab_button_width = separator_width * 2 + 3; // " + " between separators
        bar_cols
            .saturating_sub(prefix_width + tab_chrome + new_tab_button_width)
            .clamp(MIN_TAB_NAME_WIDTH, MAX_TAB_NAME_WIDTH)
    }
}

impl YinzPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.hostname = get_plugin_ids().hostname;
        self.hide_swap_layout_indication = configuration
            .get("hide_swap_layout_indication")
            .map(|s| s == "true")
            .unwrap_or(false);
        set_selectable(false);
        subscribe(&[
            EventType::TabUpdate,
            EventType::ModeUpdate,
            EventType::Mouse,
            EventType::InitialKeybinds,
            EventType::ActivePaneScroll,
            EventType::HintText,
            EventType::Timer,
            EventType::InputReceived,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::InitialKeybinds(keybinds) => {
                self.cached_keybinds = keybinds;
                if !self.cached_keybinds.is_empty() {
                    self.mode_info.keybinds = self.cached_keybinds.clone();
                }
                should_render = true;
            },
            Event::ModeUpdate(mut mode_info) => {
                if mode_info.keybinds.is_empty() && !self.cached_keybinds.is_empty() {
                    mode_info.keybinds = self.cached_keybinds.clone();
                } else if !mode_info.keybinds.is_empty() {
                    self.cached_keybinds = mode_info.keybinds.clone();
                }
                if self.mode_info != mode_info {
                    should_render = true;
                }
                self.mode_info = mode_info;
            },
            Event::TabUpdate(tabs) => {
                if let Some(active_tab_index) = tabs.iter().position(|t| t.active) {
                    // tabs are indexed starting from 1 so we need to add 1
                    let Ok(active_tab_idx) =
                        TabNumber::try_from(TabPosition::new(active_tab_index))
                    else {
                        return false;
                    };

                    if self.active_tab_idx != active_tab_idx || self.tabs != tabs {
                        should_render = true;
                    }
                    self.active_tab_idx = active_tab_idx;
                    self.tabs = tabs;
                } else {
                    eprintln!("Could not find active tab.");
                }
            },
            Event::ActivePaneScroll(scroll) => {
                if self.active_pane_scroll != scroll {
                    should_render = true;
                }
                self.active_pane_scroll = scroll;
            },
            Event::HintText(hint_variants) => {
                if hint_variants.is_empty() {
                    if self.hint_text.is_some() {
                        self.hint_text = None;
                        should_render = true;
                    }
                } else {
                    self.hint_text = Some(hint_variants);
                    self.outstanding_hint_timeouts += 1;
                    set_timeout(yinz_tile::prelude::FractionalSeconds::new(5.0));
                    should_render = true;
                }
            },
            Event::Timer(_) => {
                self.outstanding_hint_timeouts = self.outstanding_hint_timeouts.saturating_sub(1);
                if self.outstanding_hint_timeouts == 0 && self.hint_text.is_some() {
                    self.hint_text = None;
                    should_render = true;
                }
            },
            Event::InputReceived => {
                if self.hint_text.is_some() {
                    self.hint_text = None;
                    should_render = true;
                }
            },
            Event::Mouse(me) => match me {
                Mouse::LeftClick(_, col) => {
                    if let Some((start, end)) = self.breadcrumb_range {
                        if col >= start && col < end {
                            focus_host_session();
                            return should_render;
                        }
                    }
                    if let Some((start, end)) = self.new_tab_button_range {
                        if col >= start && col < end {
                            new_tab::<&str>(None, None);
                            return should_render;
                        }
                    }
                    let tab_to_focus = get_tab_to_focus(&self.tab_line, self.active_tab_idx, col);
                    if let Some(idx) = tab_to_focus {
                        switch_tab_to(idx);
                    }
                },
                Mouse::Hover(_, col) => {
                    let simplified_ui = self.mode_info.capabilities.arrow_fonts;
                    let mut new_hovered_new_tab_button = false;
                    let mut new_hovered_tab_idx = None;
                    if !simplified_ui {
                        if let Some((start, end)) = self.new_tab_button_range {
                            if col >= start && col < end {
                                new_hovered_new_tab_button = true;
                            }
                        }
                        if !new_hovered_new_tab_button {
                            new_hovered_tab_idx =
                                get_tab_to_focus(&self.tab_line, self.active_tab_idx, col);
                        }
                    }
                    if self.hovered_new_tab_button != new_hovered_new_tab_button
                        || self.hovered_tab_idx != new_hovered_tab_idx
                    {
                        self.hovered_new_tab_button = new_hovered_new_tab_button;
                        self.hovered_tab_idx = new_hovered_tab_idx;
                        should_render = true;
                    }
                },
                Mouse::ScrollUp(_) => {
                    if let Ok(next) = TabNumber::try_from(min(
                        (self.active_tab_idx.get() as usize).saturating_add(1),
                        self.tabs.len(),
                    )) {
                        switch_tab_to(next);
                    }
                },
                Mouse::ScrollDown(_) => {
                    switch_tab_to(TabNumber::new(max(
                        self.active_tab_idx.get().saturating_sub(1),
                        1,
                    )));
                },
                _ => {},
            },
            _ => {
                eprintln!("Got unrecognized event: {:?}", event);
            },
        }
        if self.tabs.is_empty() {
            // no need to render if we have no tabs, this can sometimes happen on startup before we
            // get the tab update and then we definitely don't want to render
            should_render = false;
        }
        should_render
    }

    fn render(&mut self, _rows: usize, cols: usize) {
        if self.tabs.is_empty() {
            return;
        }
        let dimmed = self.mode_info.session_ascended == Some(true)
            || self.mode_info.session_dimmed == Some(true);
        let breadcrumb_ancestry: Vec<String> = if self.mode_info.host_fullscreen == Some(true) {
            self.mode_info
                .session_ancestry
                .iter()
                .map(ToString::to_string)
                .collect()
        } else {
            vec![]
        };
        let bar_cols = cols.saturating_sub(1);
        let max_name_width = self.max_tab_name_width(bar_cols, &breadcrumb_ancestry);
        let mut all_tabs: Vec<LinePart> = vec![];
        let mut active_tab_index = 0;
        let mut is_alternate_tab = false;
        for t in &mut self.tabs {
            let mut tabname = t.name.clone();
            if t.active && self.mode_info.mode == InputMode::RenameTab {
                if tabname.is_empty() {
                    tabname = String::from("Enter name...");
                }
                active_tab_index = t.position.get();
            } else if t.active {
                active_tab_index = t.position.get();
            }
            let is_hovered = self.hovered_tab_idx.map(TabNumber::position) == Some(t.position);
            let tab = tab_style(
                tabname,
                t,
                is_alternate_tab,
                is_hovered,
                self.mode_info.style.colors,
                self.mode_info.capabilities,
                dimmed,
                max_name_width,
            );
            is_alternate_tab = !is_alternate_tab;
            all_tabs.push(tab);
        }

        let background = self.mode_info.style.colors.text_unselected.background;

        let full_pane_frames = self.mode_info.pane_frame_style == Some(PaneFrameStyle::Full);
        let hint_text = if full_pane_frames {
            None
        } else {
            self.hint_text.as_ref()
        };
        let (line, new_tab_button_range, breadcrumb_range) = tab_line(
            &self.hostname,
            self.mode_info
                .session_name
                .as_ref()
                .map(|name| name.as_str()),
            all_tabs,
            active_tab_index,
            bar_cols,
            self.mode_info.style.colors,
            self.mode_info.capabilities,
            self.mode_info.style.hide_session_name,
            self.tabs.iter().find(|t| t.active),
            &self.mode_info,
            self.hide_swap_layout_indication,
            &background,
            self.active_pane_scroll,
            hint_text,
            is_alternate_tab,
            self.hovered_new_tab_button,
            dimmed,
            &breadcrumb_ancestry,
        );
        self.tab_line = line;
        self.new_tab_button_range = new_tab_button_range;
        self.breadcrumb_range = breadcrumb_range;

        let output = self
            .tab_line
            .iter()
            .fold(String::new(), |output, part| output + &part.part);

        match background {
            PaletteColor::Rgb((r, g, b)) => {
                print!("{}\u{1b}[48;2;{};{};{}m\u{1b}[0K", output, r, g, b);
            },
            PaletteColor::EightBit(color) => {
                print!("{}\u{1b}[48;5;{}m\u{1b}[0K", output, color);
            },
        }
    }
}
