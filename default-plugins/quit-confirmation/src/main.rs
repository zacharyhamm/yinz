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
        subscribe(&[EventType::Key, EventType::Mouse]);
        set_selectable(true);

        let plugin_id = get_plugin_ids().plugin_id;
        rename_plugin_pane(plugin_id, "quit confirmation");
        // The pane is already an overlay - `Action::Quit` opens it as a modal. This only sizes it.
        if let Some(coordinates) = FloatingPaneCoordinates::new(
            Some("30%".to_owned()),
            Some("30%".to_owned()),
            Some("40%".to_owned()),
            Some("8".to_owned()),
            None,
            None,
        ) {
            change_overlay_coordinates(vec![(PaneId::Plugin(plugin_id), coordinates)]);
        }
    }

    fn update(&mut self, event: Event) -> bool {
        let event = match event {
            Event::Key(key) => self.dialog.handle_key(key),
            // The dialog is two buttons, and a dialog whose buttons only answer
            // to the keyboard is a dialog that looks broken to whoever reached
            // for the mouse. Every variant goes through: the form acts on the
            // release, not on the press.
            Event::Mouse(mouse) => self.dialog.handle_mouse(mouse),
            _ => return false,
        };
        match event {
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
