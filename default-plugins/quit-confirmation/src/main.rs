use std::collections::BTreeMap;

use zellij_tile::prelude::*;
use zellij_tile::ui_components::forms::{Button, Dialog, Form, FormEvent, Rect};
use zellij_tile::ui_components::Text;

const CANCEL: &str = "cancel";
const QUIT: &str = "quit";

/// Why the confirmation went up. Absent from the configuration for a `Ctrl-q`; the server sets
/// it when the session's last pane closing is what raised this.
const LAST_PANE: &str = "last_pane";

struct State {
    dialog: Dialog,
}

impl Default for State {
    fn default() -> Self {
        Self {
            dialog: confirmation_dialog(None, false),
        }
    }
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // What the plugins holding something outstanding asked to have said, one per line, and
        // why we are asking. Both are put here by `Screen::confirm_quit`.
        let notice = configuration.get("notice").map(String::as_str);
        let on_last_pane = configuration.get("reason").map(String::as_str) == Some(LAST_PANE);
        self.dialog = confirmation_dialog(notice, on_last_pane);

        subscribe(&[EventType::Key, EventType::Mouse]);
        set_selectable(true);

        let plugin_id = get_plugin_ids().plugin_id;
        rename_plugin_pane(plugin_id, "quit confirmation");
        // The pane is already an overlay - `Action::Quit` opens it as a modal. This only sizes
        // it. Asked of the dialog rather than fixed, because a notice - and a notice is as many
        // lines as there are plugins with something to say - makes it taller.
        if let Some(coordinates) = FloatingPaneCoordinates::new(
            Some("30%".to_owned()),
            Some("30%".to_owned()),
            Some("40%".to_owned()),
            Some(self.dialog.height().to_string()),
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

fn confirmation_dialog(notice: Option<&str>, on_last_pane: bool) -> Dialog {
    // The last pane closing is a quit the user did not ask for in so many words, so the question
    // says which gesture it is answering rather than leaving them to work it out.
    let message = match on_last_pane {
        true => "That was the last pane. Quit Zellij?",
        false => "Are you sure you want to quit?",
    };
    let mut dialog = Dialog::new(
        Text::new("Quit Zellij?").color_all(2),
        Form::new()
            .button(CANCEL, Button::new("Cancel"))
            .button(QUIT, Button::new("Quit")),
    )
    .message(Text::new(message))
    .help(Text::new(
        "<Tab/Arrows> select  <Enter> confirm  <Esc> cancel",
    ));
    // Below the buttons rather than folded into the message: the message is the question, and
    // this is what the session has to say about answering it either way.
    if let Some(notice) = notice {
        dialog.set_status(Some(Text::new(notice).color_all(3)));
    }
    dialog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_the_safe_default_and_quit_is_one_tab_away() {
        let mut dialog = confirmation_dialog(None, false);
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

    /// The notice is what a plugin with something outstanding asked to have said, and it has to
    /// survive into the dialog the user actually reads - a guard the user is never told about is
    /// a session that refuses to close for no stated reason.
    #[test]
    fn a_notice_is_shown_and_makes_the_dialog_taller() {
        let plain = confirmation_dialog(None, false);
        let with_notice = confirmation_dialog(Some("hocus is still working"), false);
        assert!(
            with_notice.height() > plain.height(),
            "a notice needs a row of its own: {} vs {}",
            with_notice.height(),
            plain.height()
        );
    }

    /// Cancel stays the safe default however the confirmation was raised. The last-pane path is
    /// the one where a stray Enter costs the most - the user did not press anything to get here.
    #[test]
    fn cancel_is_still_the_default_on_the_last_pane_path() {
        let mut dialog = confirmation_dialog(Some("hocus is still working"), true);
        assert_eq!(
            dialog.handle_key(KeyWithModifier::new(BareKey::Enter)),
            FormEvent::ButtonPressed(CANCEL.to_owned())
        );
    }
}
