//! The Todos panel owns its store watches, drafts and revision-guarded edits.
mod todos;
#[cfg(test)]
mod unit;
use agent_proto::panel_text::*;
use agent_proto::{agents, markdown, tabs, wire};
use std::collections::BTreeMap;
use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::*;

#[derive(Default)]
struct State {
    todos: todos::View,
    own_plugin_id: PluginId,
    own_client_id: ClientId,
    pane_tabs: Option<PaneTabsId>,
    bar_focused: Vec<ClientId>,
    tabs_focused: bool,
    pane_shown: bool,
    has_keyboard: Option<bool>,
    key_sheet_scroll: Option<usize>,
    last_sheet_rows: usize,
    last_sheet_max_scroll: usize,
    last_rows: usize,
    last_cols: usize,
    last_footer_row: Option<usize>,
    mouse_position: Option<(isize, usize)>,
    #[cfg(test)]
    panel_hides: usize,
    #[cfg(test)]
    ring_yields: Vec<&'static str>,
}
register_plugin!(State);
impl YinzPlugin for State {
    fn load(&mut self, _: BTreeMap<String, String>) {
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::PastedText,
            EventType::PaneUpdate,
            EventType::PaneTabsUpdate,
            EventType::Visible,
            EventType::PluginDataChanged,
        ]);
        let ids = get_plugin_ids();
        self.own_plugin_id = ids.plugin_id;
        self.own_client_id = ids.client_id;
        set_selectable(true);
        set_self_mouse_selection_support(true);
        rename_plugin_pane(self.own_plugin_id, "Todos");
        self.open_todos(ids.initial_cwd.to_string_lossy().into_owned());
    }
    fn update(&mut self, event: Event) -> bool {
        let input = matches!(
            event,
            Event::Key(_) | Event::Mouse(_) | Event::PastedText(_)
        );
        if input {
            #[cfg(not(test))]
            {
                self.own_client_id = get_plugin_ids().client_id;
            }
            self.tabs_focused = self.bar_focused.contains(&self.own_client_id);
        }
        let had_bar = self.tabs_focused;
        let rendered = match event {
            Event::Key(key) => self.on_key(key),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            Event::PastedText(text) => self.on_paste(&text),
            Event::PaneTabsUpdate(tabs) => {
                let own = tabs.iter().find(|tabs| {
                    tabs.tabs
                        .iter()
                        .any(|slot| slot.plugin_id == Some(self.own_plugin_id))
                });
                self.pane_tabs = own.map(|tabs| tabs.id);
                self.bar_focused = own.map(|tabs| tabs.bar_focused.clone()).unwrap_or_default();
                self.tabs_focused = self.bar_focused.contains(&self.own_client_id);
                true
            },
            Event::PaneUpdate(manifest) => {
                let changed = own_pane(&manifest, self.own_plugin_id).is_some_and(|pane| {
                    self.own_pane_changed(!pane.is_suppressed, pane.is_focused)
                });
                changed
            },
            Event::Visible(false) => {
                self.end_visit();
                true
            },
            Event::PluginDataChanged { watch_id, rows, .. } => {
                self.todo_rows_changed(watch_id, rows)
            },
            _ => false,
        };
        if input {
            self.bar_focused
                .retain(|client| *client != self.own_client_id);
            if self.tabs_focused {
                self.bar_focused.push(self.own_client_id);
            }
            if self.tabs_focused && !had_bar {
                if let Some(id) = self.pane_tabs {
                    let _ = focus_pane_tabs(id, true);
                }
            }
        }
        rendered
    }
    fn pipe(&mut self, message: PipeMessage) -> bool {
        #[cfg(not(test))]
        {
            self.own_client_id = get_plugin_ids().client_id;
        }
        if message.name == wire::AGENT_STRIP_TAKE {
            self.take_ring_stop(message.args.get(wire::PANEL_STOP).map(String::as_str))
        } else {
            false
        }
    }
    fn render(&mut self, rows: usize, cols: usize) {
        #[cfg(not(test))]
        let client = get_plugin_ids().client_id;
        #[cfg(test)]
        let client = self.own_client_id;
        self.tabs_focused = self.bar_focused.contains(&client);
        self.last_rows = rows;
        self.last_cols = cols;
        self.last_footer_row = None;
        let footer = self.footer_hint();
        let body = rows.saturating_sub(usize::from(footer.is_some()));
        if rows == 0 || cols == 0 {
            self.todos.clear_dimensions();
            return;
        }
        show_cursor(
            if self.has_keyboard == Some(false) || self.key_sheet_scroll.is_some() {
                None
            } else {
                self.todo_input_cursor(body, cols)
                    .map(|(x, y)| (x, y.saturating_sub(1)))
            },
        );
        if let Some(footer) = footer {
            self.last_footer_row = Some(rows - 1);
            paint(
                Text::new(footer).dim_range(..),
                0,
                rows - 1,
                Some(cols),
                Some(1),
            );
        }
        if self.key_sheet_scroll.is_some() {
            self.render_key_sheet(body, cols);
        } else {
            self.render_todos(body, cols);
        }
    }
}
impl State {
    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if self.tabs_focused {
            return self.dispatch_key(key);
        }
        // Cycle the whole panel one corner clockwise.
        // Taken before the content handlers, because where the keys are inside the panel changes
        // nothing about what it means: not a box being typed in, not the
        // reader, not an armed question - which stays armed and on screen, since moving the
        // panel answers nothing it asked - and not the key sheet, which stays up. agent owns
        // both overlays' geometry, so this asks rather than moves. Nothing to redraw: the move
        // is the server's to paint.
        if key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
            return false;
        }
        let sheet_was_up = self.key_sheet_scroll.is_some();
        if let Some(answer) = self.sheet_key(&key) {
            return answer;
        }
        // A key the sheet let through has shut it, unless it is one that works through it, and
        // the sheet coming down is a redraw whatever the key goes on to do.
        let shut = sheet_was_up && self.key_sheet_scroll.is_none();
        self.dispatch_key(key) || shut
    }
    fn sheet_key(&mut self, key: &KeyWithModifier) -> Option<bool> {
        let plain = key.has_no_modifiers();
        // A terminal that reports the Shift most layouts spend on `?` is still typing one.
        let question = key.bare_key == BareKey::Char('?')
            && (plain || key.has_modifiers(&[KeyModifier::Shift]));
        if self.key_sheet_scroll.is_none() {
            if question && !self.typing() {
                self.key_sheet_scroll = Some(0);
                return Some(true);
            }
            return None;
        }
        let shuts =
            question || (plain && matches!(key.bare_key, BareKey::Esc | BareKey::Char('q')));
        if shuts {
            self.key_sheet_scroll = None;
            return Some(true);
        }
        let page = self.last_sheet_rows.saturating_sub(1).max(1) as isize;
        match key.bare_key {
            BareKey::Down => Some(self.scroll_key_sheet(1)),
            BareKey::Up => Some(self.scroll_key_sheet(-1)),
            BareKey::Char('j') if plain => Some(self.scroll_key_sheet(1)),
            BareKey::Char('k') if plain => Some(self.scroll_key_sheet(-1)),
            BareKey::PageDown => Some(self.scroll_key_sheet(page)),
            BareKey::PageUp => Some(self.scroll_key_sheet(-page)),
            // The ring works through the sheet, which then says what the stop the keys went to
            // answers - or stays up over a pane the keys have left, to be read from there.
            BareKey::Tab => None,
            _ => {
                self.key_sheet_scroll = None;
                None
            },
        }
    }
    fn scroll_key_sheet(&mut self, delta: isize) -> bool {
        let Some(scroll) = self.key_sheet_scroll else {
            return false;
        };
        let target = scroll
            .saturating_add_signed(delta)
            .min(self.last_sheet_max_scroll);
        self.key_sheet_scroll = Some(target);
        target != scroll
    }
    fn render_key_sheet(&mut self, rows: usize, cols: usize) {
        let lines = self.key_sheet_display(cols);
        self.last_sheet_rows = rows;
        let max_scroll = lines.len().saturating_sub(rows);
        self.last_sheet_max_scroll = max_scroll;
        let scroll = self.key_sheet_scroll.unwrap_or(0).min(max_scroll);
        self.key_sheet_scroll = Some(scroll);
        if lines.is_empty() {
            paint(
                Text::new(" no keys listed for this tab").dim_range(..),
                0,
                0,
                Some(cols),
                None,
            );
            return;
        }
        for (index, (line, key_chars)) in lines.into_iter().skip(scroll).take(rows).enumerate() {
            paint(
                Text::new(line).color_range(2, ..key_chars),
                0,
                index,
                Some(cols),
                None,
            );
        }
    }
    fn key_sheet_display(&self, cols: usize) -> Vec<(String, usize)> {
        let sheet = self.key_sheet();
        // As wide as the widest keys, up to half the row: past that the descriptions would have
        // nothing left to be read in.
        let key_width = sheet
            .iter()
            .map(|(keys, _)| keys.width())
            .max()
            .unwrap_or(0)
            .min(cols.saturating_sub(1) / 2);
        sheet
            .iter()
            .flat_map(|(keys, what)| key_sheet_lines(keys, what, key_width, cols))
            .collect()
    }
    fn own_pane_changed(&mut self, visible: bool, focused: bool) -> bool {
        let was_shown = std::mem::replace(&mut self.pane_shown, visible);
        if was_shown && !visible {
            self.end_visit();
        }
        let focused = visible && focused;
        let was = self.has_keyboard.replace(focused);
        visible && was.is_some_and(|was| was != focused)
    }

    fn bar_has_keys(&self) -> bool {
        self.has_keyboard != Some(false) && self.tabs_focused
    }
    fn focus_panel_bar(&mut self, stop: &'static str) {
        #[cfg(test)]
        self.ring_yields.push(stop);
        self.take_ring_stop(Some(if stop == wire::STOP_HISTORY {
            wire::STOP_HISTORY
        } else {
            agent_proto::tabs::STOP_TABS
        }));
    }
    fn key_sheet(&self) -> Vec<(String, String)> {
        if self.bar_has_keys() {
            return sheet(tabs::BAR_KEYS);
        }
        let mut rows = self.todo_key_sheet();
        rows.extend(sheet(PANEL_KEYS));
        rows
    }
    fn footer_hint(&self) -> Option<String> {
        if self.key_sheet_scroll.is_none() || self.last_rows < 2 || self.last_cols == 0 {
            return None;
        }
        if self.bar_has_keys() {
            let segments: Vec<&str> = tabs::BAR_HINT.split(" · ").collect();
            return Some(agents::fit_keys(&segments, self.last_cols));
        }
        Some(agents::fit_keys(
            &["Esc/? close", "j/k scroll"],
            self.last_cols,
        ))
    }
    fn dispatch_key(&mut self, key: KeyWithModifier) -> bool {
        if self.tabs_focused {
            if key.bare_key == BareKey::Tab && key.has_modifiers(&[KeyModifier::Shift]) {
                self.keys_to_the_list();
                if let Some(id) = self.pane_tabs {
                    let _ = focus_pane_tabs(id, false);
                }
                self.focus_panel_bar(wire::STOP_HISTORY);
            } else {
                if key.has_no_modifiers()
                    && matches!(key.bare_key, BareKey::Enter | BareKey::Tab | BareKey::Esc)
                {
                    self.keys_to_the_list();
                }
                tabs::forward_bar_key(key);
            }
            return true;
        }
        self.todo_key(key)
    }
    fn keys_to_the_list(&mut self) {
        self.tabs_focused = false;
        self.bar_focused
            .retain(|client| *client != self.own_client_id);
    }
    fn take_ring_stop(&mut self, stop: Option<&str>) -> bool {
        self.tabs_focused = stop == Some(agent_proto::tabs::STOP_TABS);
        self.bar_focused
            .retain(|client| *client != self.own_client_id);
        if self.tabs_focused {
            self.bar_focused.push(self.own_client_id);
        }
        if let Some(id) = self.pane_tabs {
            let _ = focus_pane_tabs(id, self.tabs_focused);
        }
        true
    }
    fn end_visit(&mut self) {
        self.tabs_focused = false;
        self.bar_focused.clear();
        self.key_sheet_scroll = None;
        self.cancel_todo_question();
        self.todos_end_visit();
    }
    fn hide_the_panel(&mut self) {
        self.end_visit();
        self.pane_shown = false;
        #[cfg(test)]
        {
            self.panel_hides += 1;
        }
        hide_self();
    }
    fn typing(&self) -> bool {
        matches!(self.todos.mode, todos::Mode::Input { .. })
    }
    fn on_paste(&mut self, text: &str) -> bool {
        if self.tabs_focused {
            return false;
        }
        self.todo_paste(text)
    }
    fn on_mouse(&mut self, mouse: Mouse) -> bool {
        match mouse {
            Mouse::Hover(y, x)
            | Mouse::LeftClick(y, x)
            | Mouse::RightClick(y, x)
            | Mouse::Hold(y, x)
            | Mouse::Release(y, x) => self.mouse_position = Some((y, x)),
            _ => {},
        }
        if self.last_rows == 0
            || self.last_cols == 0
            || self.mouse_position.is_some_and(|(y, x)| {
                y < 0
                    || y as usize >= self.last_rows
                    || x >= self.last_cols
                    || self.last_footer_row == Some(y as usize)
            })
        {
            return false;
        }
        if self.key_sheet_scroll.is_some() {
            return match mouse {
                Mouse::LeftClick(..) | Mouse::RightClick(..) => {
                    self.key_sheet_scroll = None;
                    true
                },
                Mouse::ScrollUp(n) => self.scroll_key_sheet(-(n.max(1) as isize)),
                Mouse::ScrollDown(n) => self.scroll_key_sheet(n.max(1) as isize),
                _ => false,
            };
        }

        let left_bar =
            matches!(mouse, Mouse::LeftClick(..)) && std::mem::take(&mut self.tabs_focused);
        if left_bar {
            self.bar_focused
                .retain(|client| *client != self.own_client_id);
            if let Some(id) = self.pane_tabs {
                let _ = focus_pane_tabs(id, false);
            }
        }
        self.todo_mouse(mouse)
    }
}
fn own_pane(manifest: &PaneManifest, own_plugin_id: PluginId) -> Option<&PaneInfo> {
    manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == PaneId::Plugin(own_plugin_id))
}
