//! The Todos panel owns its store watches, drafts and revision-guarded edits.
mod todos;
#[cfg(test)]
mod unit;
use hocus_proto::panel_text::{divider_line, scroll_readout, wrap_words};
use hocus_proto::{agents, markdown, tabs, wire};
use std::collections::BTreeMap;
use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    todos: todos::View,
    own_plugin_id: PluginId,
    own_client_id: ClientId,
    container: Option<PaneTabsId>,
    tabs_focused: bool,
    has_keyboard: Option<bool>,
    #[cfg(test)]
    panel_hides: usize,
    key_sheet_scroll: Option<usize>,
    #[cfg(test)]
    ring_yields: Vec<&'static str>,
    #[cfg(test)]
    surface: Surface,
}
#[cfg(test)]
#[derive(Default, PartialEq)]
enum Surface {
    Agents,
    #[default]
    Todos,
}
register_plugin!(State);
impl ZellijPlugin for State {
    fn load(&mut self, _: BTreeMap<String, String>) {
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::PastedText,
            EventType::PluginDataChanged,
            EventType::Visible,
            EventType::PaneUpdate,
            EventType::PaneTabsUpdate,
        ]);
        let ids = get_plugin_ids();
        self.own_plugin_id = ids.plugin_id;
        self.own_client_id = ids.client_id;
        set_selectable(true);
        set_self_mouse_selection_support(true);
        rename_plugin_pane(ids.plugin_id, "Todos");
        self.open_todos(ids.initial_cwd.to_string_lossy().into_owned());
    }
    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Mouse(mouse) => self.todo_mouse(mouse),
            Event::PastedText(text) => self.on_paste(&text),
            Event::Visible(false) => {
                self.cancel_todo_question();
                self.todos_end_visit();
                true
            },
            Event::PluginDataChanged { watch_id, rows, .. } => {
                self.todo_rows_changed(watch_id, rows)
            },
            Event::PaneTabsUpdate(tabs) => {
                if let Some(tabs) = tabs.iter().find(|tabs| {
                    tabs.tabs
                        .iter()
                        .any(|slot| slot.plugin_id == Some(self.own_plugin_id))
                }) {
                    self.container = Some(tabs.id);
                    self.tabs_focused = tabs.bar_focused.contains(&self.own_client_id);
                }
                true
            },
            Event::PaneUpdate(manifest) => {
                self.has_keyboard = manifest
                    .panes
                    .values()
                    .flatten()
                    .find(|pane| pane.id == PaneId::Plugin(self.own_plugin_id))
                    .map(|pane| pane.is_focused);
                self.container = manifest
                    .panes
                    .values()
                    .flatten()
                    .find(|pane| pane.id == PaneId::Plugin(self.own_plugin_id))
                    .and_then(|pane| pane.pane_tabs.as_ref().map(|tabs| tabs.id));
                false
            },
            _ => false,
        }
    }
    fn pipe(&mut self, message: PipeMessage) -> bool {
        if message.name == wire::AGENT_STRIP_TAKE {
            if message
                .args
                .get(wire::PANEL_STOP)
                .is_some_and(|stop| stop == wire::STOP_TABS)
            {
                if let Some(id) = self.container {
                    let _ = focus_pane_tabs(id, true);
                }
            }
            return true;
        }
        false
    }
    fn render(&mut self, rows: usize, cols: usize) {
        if let Some(scroll) = self.key_sheet_scroll {
            show_cursor(None);
            for (y, (key, description)) in
                self.key_sheet().iter().skip(scroll).take(rows).enumerate()
            {
                paint(
                    Text::new(agents::cut_to_width(format!("{key}  {description}"), cols)),
                    0,
                    y,
                    Some(cols),
                    Some(1),
                );
            }
        } else {
            show_cursor(
                self.todo_input_cursor(rows, cols)
                    .map(|(x, y)| (x, y.saturating_sub(1))),
            );
            self.render_todos(rows, cols);
        }
    }
}
impl State {
    fn on_key(&mut self, key: KeyWithModifier) -> bool {
        if key.bare_key == BareKey::Char('o') && key.has_modifiers(&[KeyModifier::Ctrl]) {
            pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_REPOSITION));
            return false;
        }
        if self.tabs_focused {
            // Keys already queued when the child yielded still belong to the host bar.
            if key.has_no_modifiers() && matches!(key.bare_key, BareKey::Left | BareKey::Right) {
                run_action(
                    if key.bare_key == BareKey::Left {
                        actions::Action::PreviousPaneTab
                    } else {
                        actions::Action::NextPaneTab
                    },
                    BTreeMap::new(),
                );
                return true;
            }
            self.tabs_focused = false;
            if key.bare_key == BareKey::Tab && key.has_modifiers(&[KeyModifier::Shift]) {
                self.yield_to_chat(wire::STOP_INPUT);
                return true;
            }
            if let Some(id) = self.container {
                let _ = focus_pane_tabs(id, false);
            }
            if matches!(
                key.bare_key,
                BareKey::Tab | BareKey::Esc | BareKey::Enter | BareKey::Down | BareKey::Char('j')
            ) {
                return true;
            }
        }
        if let Some(scroll) = &mut self.key_sheet_scroll {
            match key.bare_key {
                BareKey::Esc | BareKey::Char('?') | BareKey::Char('q') => {
                    self.key_sheet_scroll = None
                },
                BareKey::Down | BareKey::Char('j') => *scroll += 1,
                BareKey::Up | BareKey::Char('k') => *scroll = scroll.saturating_sub(1),
                _ => {},
            }
            return true;
        }
        if key.bare_key == BareKey::Char('?')
            && key.has_no_modifiers()
            && !matches!(self.todos.mode, todos::Mode::Input { .. })
        {
            self.key_sheet_scroll = Some(0);
            return true;
        }
        let changed = self.todo_key(key);
        if self.tabs_focused {
            if let Some(id) = self.container {
                let _ = focus_pane_tabs(id, true);
            }
        }
        changed
    }
    fn on_paste(&mut self, text: &str) -> bool {
        let changed = self.todo_paste(text);
        if changed {
            self.tabs_focused = false;
        }
        changed
    }
    fn key_sheet(&self) -> Vec<(String, String)> {
        self.todo_key_sheet()
    }
    fn bar_has_keys(&self) -> bool {
        self.has_keyboard != Some(false) && self.tabs_focused
    }
    fn box_ahead_of_the_bar(&self) -> bool {
        false
    }
    fn hide_the_panel(&mut self) {
        #[cfg(test)]
        {
            self.panel_hides += 1;
        }
        self.todos_end_visit();
        hide_self();
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_HIDE));
    }
    fn yield_to_chat(&mut self, stop: &'static str) {
        #[cfg(test)]
        self.ring_yields.push(stop);
        pipe_message_to_plugin(MessageToPlugin::new(wire::AGENT_STRIP_YIELD).with_args(
            BTreeMap::from([(wire::PANEL_STOP.to_owned(), stop.to_owned())]),
        ));
    }
    #[cfg(test)]
    fn switch_surface(&mut self, surface: Surface) {
        self.cancel_todo_question();
        self.surface = surface;
    }
}
fn paint(text: Text, x: usize, y: usize, width: Option<usize>, height: Option<usize>) {
    print_text_with_coordinates(text, x, y, width, height);
}
