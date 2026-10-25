use super::*;
use crate::ui_components::forms::{Button, Dialog, Form, Rect, TextInput};
use yinz_types::ui_components::UI_COMPONENT_DCS_PREFIX;

#[test]
fn native_buffers_leave_host_cursor_delivery_to_the_caller() {
    let mut form = Form::new().field("name", "Name", TextInput::new("日".into()));
    form.handle_key(crate::prelude::KeyWithModifier::new(
        crate::prelude::BareKey::Enter,
    ));
    let buffer = form.render_buffer_in_viewport(2, 3, 20, 1);
    assert_eq!(buffer.cursor, Some(Some((10, 3))));
    assert!(buffer
        .output
        .starts_with(&format!("{UI_COMPONENT_DCS_PREFIX}text;")));
    let mut dialog = Dialog::new(Text::new("Name"), form);
    assert!(!dialog
        .render_buffer(Rect::new(0, 0, 20, 5))
        .output
        .is_empty());
    let empty = dialog.render_buffer(Rect::new(0, 0, 0, 0));
    assert_eq!(empty.cursor, Some(None));
    assert!(empty.output.is_empty());
    assert_eq!(
        dialog.handle_mouse(crate::prelude::Mouse::LeftClick(2, 2)),
        crate::ui_components::forms::FormEvent::Ignored
    );
    let mut button = Button::new("Save");
    assert!(button.serialize(0, 0, Some(1)).contains("0/0/1/1;83"));
    assert_eq!(
        button.serialize(0, 0, Some(40)),
        crate::ui_components::serialize_ribbon_with_coordinates(
            &Text::new("Save"),
            0,
            0,
            None,
            None
        )
    );
    assert!(button.serialize(0, 0, Some(0)).is_empty());
}
