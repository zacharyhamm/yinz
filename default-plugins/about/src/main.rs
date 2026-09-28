mod active_component;
mod keybindings;
mod pages;
use yinz_tile::prelude::*;

use pages::Page;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::keybindings::{apply_missing_binds, ApplyStatus, Feature, KeybindingState};
use crate::pages::PageKind;

const UI_ROWS: usize = 20;
const UI_COLUMNS: usize = 90;

#[derive(Debug)]
struct App {
    active_page: Page,
    yinz_version: Rc<RefCell<String>>,
    keybinding_state: Rc<RefCell<KeybindingState>>,
    tab_rows: usize,
    tab_columns: usize,
    own_plugin_id: Option<yinz_tile::prelude::PluginId>,
    is_release_notes: bool,
}

impl Default for App {
    fn default() -> Self {
        let yinz_version = Rc::new(RefCell::new("".to_owned()));
        let keybinding_state = Rc::new(RefCell::new(Default::default()));
        App {
            active_page: Page::new_main_screen("".to_owned(), false, keybinding_state.clone()),
            yinz_version,
            keybinding_state,
            tab_rows: 0,
            tab_columns: 0,
            own_plugin_id: None,
            is_release_notes: false,
        }
    }
}

register_plugin!(App);

impl YinzPlugin for App {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.is_release_notes = configuration
            .get("is_release_notes")
            .map(|v| v == "true")
            .unwrap_or(false);
        subscribe(&[
            EventType::Key,
            EventType::Mouse,
            EventType::ModeUpdate,
            EventType::TabUpdate,
            EventType::FailedToWriteConfigToDisk,
            EventType::ConfigWasWrittenToDisk,
        ]);
        let own_plugin_id = get_plugin_ids().plugin_id;
        self.own_plugin_id = Some(own_plugin_id);
        *self.yinz_version.borrow_mut() = get_yinz_version();
        self.change_own_title();
        self.active_page = self.main_screen();
    }
    fn update(&mut self, event: Event) -> bool {
        let mut should_render = false;
        match event {
            Event::FailedToWriteConfigToDisk { file_path, reason } => {
                if self.applying_keybindings() {
                    self.keybinding_state
                        .borrow_mut()
                        .set_status(ApplyStatus::Failed { file_path, reason });
                    self.refresh_active_page();
                    should_render = true;
                }
            },
            Event::ConfigWasWrittenToDisk => {
                if self.applying_keybindings() {
                    self.keybinding_state
                        .borrow_mut()
                        .set_status(ApplyStatus::Applied);
                    self.refresh_active_page();
                    should_render = true;
                }
            },
            Event::TabUpdate(tab_info) => {
                self.center_own_pane(tab_info);
            },
            Event::Mouse(mouse_event) => {
                should_render = self.handle_mouse_event(mouse_event);
            },
            Event::ModeUpdate(mode_info) => {
                let base_mode_changed = self
                    .keybinding_state
                    .borrow_mut()
                    .set_base_mode(mode_info.base_mode.unwrap_or(InputMode::Normal));
                let missing_keybindings_changed = self
                    .keybinding_state
                    .borrow_mut()
                    .update_from_keybinds(&mode_info.keybinds);
                if base_mode_changed || missing_keybindings_changed {
                    self.refresh_active_page();
                    should_render = true;
                }
            },
            Event::Key(key) => {
                should_render = self.handle_key(key);
            },
            _ => {},
        }
        should_render
    }
    fn render(&mut self, rows: usize, cols: usize) {
        self.active_page.render(rows, cols);
    }
}

impl App {
    fn main_screen(&self) -> Page {
        Page::new_main_screen(
            self.yinz_version.borrow().clone(),
            self.is_release_notes,
            self.keybinding_state.clone(),
        )
    }
    fn main_screen_builder(&self) -> Rc<dyn Fn() -> Page> {
        let yinz_version = self.yinz_version.clone();
        let keybinding_state = self.keybinding_state.clone();
        let is_release_notes = self.is_release_notes;
        Rc::new(move || {
            Page::new_main_screen(
                yinz_version.borrow().clone(),
                is_release_notes,
                keybinding_state.clone(),
            )
        })
    }
    fn update_keybindings_screen(&self) -> Page {
        Page::new_update_keybindings(self.keybinding_state.clone(), self.main_screen_builder())
    }
    fn applying_keybindings(&self) -> bool {
        self.keybinding_state.borrow().status() == &ApplyStatus::Applying
    }
    fn awaiting_keybinding_confirmation(&self) -> bool {
        let is_confirmation_page = match self.active_page.kind {
            PageKind::UpdateKeybindings => true,
            PageKind::NestedSessions => self
                .keybinding_state
                .borrow()
                .has_missing_binds_for(Feature::NestedSessions),
            PageKind::PaneFocus => self
                .keybinding_state
                .borrow()
                .has_missing_binds_for(Feature::PaneFocus),
            PageKind::ScrollByCommand => self
                .keybinding_state
                .borrow()
                .has_missing_binds_for(Feature::ScrollByCommand),
            _ => false,
        };
        is_confirmation_page && self.keybinding_state.borrow().status() == &ApplyStatus::NotApplied
    }
    fn confirm_keybindings(&mut self) {
        let needs_conflict_confirmation = self.active_page.kind != PageKind::UpdateKeybindings
            && self.keybinding_state.borrow().has_conflicts();
        if needs_conflict_confirmation {
            self.active_page = self.update_keybindings_screen();
        } else {
            apply_missing_binds(&self.keybinding_state);
            self.refresh_active_page();
        }
    }
    fn refresh_active_page(&mut self) {
        match self.active_page.kind {
            PageKind::MainScreen => {
                self.active_page = self.main_screen();
            },
            PageKind::NestedSessions => {
                self.active_page = Page::new_nested_sessions(
                    self.keybinding_state.clone(),
                    self.main_screen_builder(),
                );
            },
            PageKind::PaneFocus => {
                self.active_page =
                    Page::new_pane_focus(self.keybinding_state.clone(), self.main_screen_builder());
            },
            PageKind::ScrollByCommand => {
                self.active_page = Page::new_scroll_by_command(
                    self.keybinding_state.clone(),
                    self.main_screen_builder(),
                );
            },
            PageKind::UpdateKeybindings => {
                self.active_page = self.update_keybindings_screen();
            },
            PageKind::Other => {},
        }
    }
    pub fn change_own_title(&mut self) {
        if let Some(own_plugin_id) = self.own_plugin_id {
            if self.is_release_notes {
                rename_plugin_pane(
                    own_plugin_id,
                    format!("Release Notes {}", self.yinz_version.borrow()),
                );
            } else {
                rename_plugin_pane(own_plugin_id, "About Yinz");
            }
        }
    }
    pub fn handle_mouse_event(&mut self, mouse_event: Mouse) -> bool {
        let mut should_render = false;
        match mouse_event {
            Mouse::LeftClick(line, column) => {
                if let Some(new_page) = self
                    .active_page
                    .handle_mouse_left_click(column, line as usize)
                {
                    self.active_page = new_page;
                    should_render = true;
                }
            },
            Mouse::Hover(line, column) => {
                should_render = self.active_page.handle_mouse_hover(column, line as usize);
            },
            _ => {},
        }
        should_render
    }
    pub fn handle_key(&mut self, key: KeyWithModifier) -> bool {
        let mut should_render = false;
        if key.bare_key == BareKey::Enter && key.has_no_modifiers() {
            if let Some(new_page) = self.active_page.handle_selection() {
                self.active_page = new_page;
                should_render = true;
            }
        } else if key.bare_key == BareKey::Char('u')
            && key.has_no_modifiers()
            && self.keybinding_state.borrow().has_missing_binds()
        {
            self.active_page = self.update_keybindings_screen();
            should_render = true;
        } else if key.bare_key == BareKey::Char('y')
            && key.has_no_modifiers()
            && self.awaiting_keybinding_confirmation()
        {
            self.confirm_keybindings();
            should_render = true;
        } else if key.bare_key == BareKey::Char('n')
            && key.has_no_modifiers()
            && self.awaiting_keybinding_confirmation()
        {
            self.active_page = self.main_screen();
            should_render = true;
        } else if key.bare_key == BareKey::Esc && key.has_no_modifiers() {
            if self.active_page.is_main_screen {
                close_self();
            } else {
                self.active_page = self.main_screen();
                should_render = true;
            }
        } else {
            should_render = self.active_page.handle_key(key);
        }
        should_render
    }
    fn center_own_pane(&mut self, tab_info: Vec<TabInfo>) {
        // we only take the size of the first tab because at the time of writing this is
        // identical to all tabs, but this might not always be the case...
        if let Some(first_tab) = tab_info.first() {
            let prev_tab_columns = self.tab_columns;
            let prev_tab_rows = self.tab_rows;
            self.tab_columns = (first_tab.display_area_columns).get();
            self.tab_rows = (first_tab.display_area_rows).get();
            if self.tab_columns != prev_tab_columns || self.tab_rows != prev_tab_rows {
                let desired_x_coords = self.tab_columns.saturating_sub(UI_COLUMNS) / 2;
                let desired_y_coords = self.tab_rows.saturating_sub(UI_ROWS) / 2;
                change_floating_panes_coordinates(vec![(
                    PaneId::Plugin(self.own_plugin_id.unwrap()),
                    FloatingPaneCoordinates::new(
                        Some(desired_x_coords.to_string()),
                        Some(desired_y_coords.to_string()),
                        Some(UI_COLUMNS.to_string()),
                        Some(UI_ROWS.to_string()),
                        None,
                        Some(false),
                    )
                    .unwrap(),
                )]);
            }
        }
    }
}

#[cfg(test)]
mod unit;
