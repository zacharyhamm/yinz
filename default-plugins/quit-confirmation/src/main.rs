use std::collections::BTreeMap;

use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{Button, Dialog, Form, FormEvent, Rect};
use zellij_tile::ui_components::Text;

const CANCEL: &str = "cancel";
const QUIT: &str = "quit";

struct State {
    dialog: Dialog,
}

impl Default for State {
    fn default() -> Self {
        Self {
            dialog: confirmation_dialog(),
        }
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        subscribe(&[EventType::Key]);
        set_selectable(true);

        let plugin_id = get_plugin_ids().plugin_id;
        rename_plugin_pane(plugin_id, "quit confirmation");
        if let Some(coordinates) = FloatingPaneCoordinates::new(
            Some("30%".to_owned()),
            Some("30%".to_owned()),
            Some("40%".to_owned()),
            Some("8".to_owned()),
            None,
            None,
            None,
        )
        .map(FloatingPaneCoordinates::as_overlay)
        {
            change_floating_panes_coordinates(vec![(PaneId::Plugin(plugin_id), coordinates)]);
        }
    }

    fn update(&mut self, event: Event) -> bool {
        let Event::Key(key) = event else {
            return false;
        };
        match self.dialog.handle_key(key) {
            FormEvent::Cancel => {
                close_self();
                false
            },
            FormEvent::ButtonPressed(id) if id == CANCEL => {
                close_self();
                false
            },
            FormEvent::ButtonPressed(id) if id == QUIT => {
                close_self();
                quit_zellij();
                false
            },
            event => event.should_render(),
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        self.dialog.render(Rect::new(0, 0, cols, rows));
    }
}

fn confirmation_dialog() -> Dialog {
    Dialog::new(
        Text::new("Quit Zellij?").color_all(2),
        Form::new()
            .button(CANCEL, Button::new("Cancel"))
            .button(QUIT, Button::new("Quit")),
    )
    .message(Text::new("Are you sure you want to quit?"))
    .help(Text::new("<Tab> select  <Enter> confirm  <Esc> cancel"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_the_safe_default_and_quit_is_one_tab_away() {
        let mut dialog = confirmation_dialog();
        assert_eq!(
            dialog.handle_key(KeyWithModifier::new(BareKey::Enter)),
            FormEvent::ButtonPressed(CANCEL.to_owned())
        );
        assert_eq!(
            dialog.handle_key(KeyWithModifier::new(BareKey::Tab)),
            FormEvent::FocusMoved
        );
        assert_eq!(
            dialog.handle_key(KeyWithModifier::new(BareKey::Enter)),
            FormEvent::ButtonPressed(QUIT.to_owned())
        );
        assert_eq!(
            dialog.handle_key(KeyWithModifier::new(BareKey::Esc)),
            FormEvent::Cancel
        );
    }
}
