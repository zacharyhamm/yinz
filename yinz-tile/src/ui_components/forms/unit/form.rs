use super::*;

#[test]
fn inactive_arrows_traverse_every_control_and_skip_disabled_buttons() {
    let ids = ["name", "layout", "frames", "mode", "model", "save"];
    for (arrow, forward) in [
        (BareKey::Down, true),
        (BareKey::Right, true),
        (BareKey::Up, false),
        (BareKey::Left, false),
    ] {
        for (index, id) in ids.iter().enumerate() {
            let mut form = a_strict_form()
                .combo("model", "Model", Combo::new(["one", "two"]))
                .button("disabled", Button::new("Disabled").disabled(true));
            form.focus_on(id);
            assert_eq!(form.handle_key(key(arrow)), FormEvent::FocusMoved);
            let next = if forward {
                (index + 1) % ids.len()
            } else {
                (index + ids.len() - 1) % ids.len()
            };
            assert_eq!(form.focused_id(), Some(ids[next]), "{id}, {arrow:?}");
            assert_eq!(form.text("mode"), "One");
            assert!(!form.checked("frames"));
        }
    }
}

#[test]
fn active_controls_keep_all_arrows_even_with_custom_traversal() {
    for id in ["name", "layout", "mode", "model"] {
        for arrow in [BareKey::Up, BareKey::Down, BareKey::Left, BareKey::Right] {
            let mut form = a_strict_form()
                .combo("model", "Model", Combo::empty())
                .with_traversal_keys(key(arrow), key(arrow).with_alt_modifier());
            form.focus_on(id);
            form.handle_key(key(BareKey::Enter));
            assert_ne!(
                form.handle_key(key(arrow)),
                FormEvent::Ignored,
                "{id}, {arrow:?}"
            );
            assert_eq!(form.focused_id(), Some(id));
            assert_ne!(
                form.handle_key(key(arrow).with_alt_modifier()),
                FormEvent::Ignored
            );
            assert_eq!(form.focused_id(), Some(id));
        }
    }
}

#[test]
fn traversal_ends_interaction_and_discards_dropdown_highlights() {
    for backward in [false, true] {
        for id in ["name", "layout", "mode", "model"] {
            let mut form = a_strict_form().combo("model", "Model", Combo::new(["one", "two"]));
            form.focus_on(id);
            form.handle_key(key(BareKey::Enter));
            if id == "name" {
                form.insert_pasted_text("keep me");
            }
            form.handle_key(key(BareKey::Down));
            let value = form.text(id).to_owned();
            let tab = if backward {
                key(BareKey::Tab).with_shift_modifier()
            } else {
                key(BareKey::Tab)
            };
            assert_eq!(form.handle_key(tab), FormEvent::FocusMoved);
            assert!(!form.is_editing());
            assert!(!form.is_radio_active());
            assert!(form.open_dropdown_index().is_none());
            assert_eq!(form.text(id), value);
        }
    }
}

#[test]
fn ctrl_c_cancels_every_control_in_every_state() {
    for active in [false, true] {
        for id in ["name", "layout", "frames", "mode", "model", "save"] {
            let mut form = a_strict_form().combo("model", "Model", Combo::new(["one"]));
            form.focus_on(id);
            if active {
                form.handle_key(key(BareKey::Enter));
            }
            assert_eq!(
                form.handle_key(key(BareKey::Char('c')).with_ctrl_modifier()),
                FormEvent::Cancel
            );
        }
    }
}

#[test]
fn dropdown_highlighting_and_unchanged_commits_only_report_interaction() {
    let mut form = a_strict_form();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Enter));
    assert_eq!(form.handle_key(key(BareKey::Down)), FormEvent::FocusMoved);
    assert_eq!(form.text("layout"), "default");
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
    assert_eq!(form.text("layout"), "default");
    form.handle_key(key(BareKey::Enter));
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    form.handle_key(key(BareKey::Enter));
    form.render(0, 0, 40);
    assert_eq!(click(&mut form, 2, 0), FormEvent::FocusMoved);
    assert_eq!(form.text("layout"), "default");
}

#[test]
fn empty_and_single_control_forms_keep_navigation_bounded() {
    for arrow in [BareKey::Up, BareKey::Down, BareKey::Left, BareKey::Right] {
        assert_eq!(Form::new().handle_key(key(arrow)), FormEvent::Ignored);
        let mut form = Form::new().select("only", "Only", Select::new(["one"]));
        assert_eq!(form.handle_key(key(arrow)), FormEvent::FocusMoved);
        assert_eq!(form.focused_id(), Some("only"));
        form.handle_key(key(BareKey::Enter));
        assert_eq!(form.handle_key(key(arrow)), FormEvent::FocusMoved);
        form.handle_key(key(BareKey::Tab));
        assert!(!form.select_by_id("only").unwrap().is_open());
    }
    let mut form = Form::new().radio("only", "Only", RadioGroup::new(["one"]));
    form.handle_key(key(BareKey::Char(' ')));
    assert!(form.is_radio_active());
    assert_eq!(form.handle_key(key(BareKey::Right)), FormEvent::FocusMoved);
    form.handle_key(key(BareKey::Esc));
    assert!(!form.is_radio_active());
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
}

#[test]
fn active_text_retains_modified_editing_and_only_value_changes_report_changed() {
    let mut form = a_form();
    form.insert_pasted_text("hello world");
    assert_eq!(
        form.handle_key(key(BareKey::Left).with_ctrl_modifier()),
        FormEvent::FocusMoved
    );
    assert_eq!(form.fields[0].input().unwrap().get_cursor_position(), 6);
    assert_eq!(
        form.handle_key(key(BareKey::Char('u')).with_ctrl_modifier()),
        FormEvent::FieldChanged("name".into())
    );
    assert_eq!(form.text("name"), "world");
    assert_eq!(form.focused_id(), Some("name"));
}

#[test]
fn history_belongs_to_active_text_and_restores_the_draft() {
    let mut form = a_form();
    form.fields[0]
        .input_mut()
        .unwrap()
        .set_history(vec!["old".into()]);
    form.insert_pasted_text("draft");
    assert_eq!(
        form.handle_key(key(BareKey::Up)),
        FormEvent::FieldChanged("name".into())
    );
    assert_eq!(form.text("name"), "old");
    assert_eq!(
        form.handle_key(key(BareKey::Down)),
        FormEvent::FieldChanged("name".into())
    );
    assert_eq!(form.text("name"), "draft");
    form.handle_key(key(BareKey::Enter));
    form.handle_key(key(BareKey::Down));
    assert_eq!(form.focused_id(), Some("path"));
}

#[test]
fn radio_focus_styles_distinguish_activation_and_small_viewports_keep_focus_visible() {
    let mut form = a_strict_form();
    form.focus_on("mode");
    let inactive = form.render_buffer_in_viewport(0, 0, 40, 1);
    assert_eq!(inactive.cursor, Some(None));
    assert_eq!(form.fields[3].last_rendered, Some(Rect::new(0, 0, 40, 1)));
    form.handle_key(key(BareKey::Enter));
    let active = form.render_buffer_in_viewport(0, 0, 40, 1);
    assert_ne!(inactive.output, active.output);
    assert_eq!(active.cursor, Some(None));
    form.handle_key(key(BareKey::Char(' ')));
    assert_eq!(
        form.render_buffer_in_viewport(0, 0, 40, 1).output,
        inactive.output
    );
    form.handle_key(key(BareKey::Enter));
    form.handle_key(key(BareKey::Right));
    form.render_buffer_in_viewport(0, 0, 12, 1);
    assert_eq!(form.fields[3].radio().unwrap().option_at(6, 0), Some(1));
    form.focus_on("name");
    form.handle_key(key(BareKey::Enter));
    assert!(form
        .render_buffer_in_viewport(0, 0, 8, 1)
        .cursor
        .flatten()
        .is_some());
    assert_eq!(
        form.render_buffer_in_viewport(0, 0, 0, 0).cursor,
        Some(None)
    );
}

fn a_form() -> Form {
    Form::new()
        .field("name", "Name", TextInput::empty())
        .field("path", "Path", TextInput::empty())
        .button("save", Button::new("Save"))
        .button("cancel", Button::new("Cancel"))
}

fn key(bare_key: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare_key)
}

/// A form of every widget type, with explicit activation.
fn a_strict_form() -> Form {
    Form::new()
        .field("name", "Name", TextInput::empty())
        .select("layout", "Layout", Select::new(["default", "compact"]))
        .checkbox("frames", "Frames", Checkbox::new(false))
        .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
        .button("save", Button::new("Save"))
}

#[test]
fn enter_toggles_input_mode_and_never_submits() {
    let mut form = a_strict_form();
    assert!(!form.is_editing());
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert!(form.is_editing());
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert!(!form.is_editing());
}

#[test]
fn typing_auto_enters_input_mode() {
    let mut form = a_strict_form();
    assert_eq!(
        form.handle_key(key(BareKey::Char('a'))),
        FormEvent::FieldChanged("name".to_string())
    );
    assert!(form.is_editing());
    assert_eq!(form.text("name"), "a");
}

#[test]
fn esc_leaves_input_mode_before_cancelling() {
    let mut form = a_strict_form();
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
    form.handle_key(key(BareKey::Enter));
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
    assert!(!form.is_editing());
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
}

#[test]
fn tab_leaves_input_mode_and_moves_focus() {
    let mut form = a_strict_form();
    form.handle_key(key(BareKey::Enter));
    assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
    assert!(!form.is_editing());
    assert_eq!(form.focused_id(), Some("layout"));
}

#[test]
fn moving_focus_leaves_input_mode() {
    let mut form = a_strict_form();
    form.handle_key(key(BareKey::Enter));
    // Even onto the field that already has it: that is a request to start
    // over on the field, not a no-op.
    form.focus_on("name");
    assert!(!form.is_editing());

    form.handle_key(key(BareKey::Enter));
    form.move_focus(true);
    assert!(!form.is_editing());
}

#[test]
fn the_cursor_is_only_shown_while_editing() {
    let mut form = a_strict_form();
    let rendered = form.render(2, 5, 40);
    assert_eq!(rendered.cursor, Some(None));

    form.handle_key(key(BareKey::Enter));
    let rendered = form.render(2, 5, 40);
    // The row the focused field was drawn on, which is the row the form was
    // asked to start at.
    assert_eq!(rendered.cursor, Some(Some((8, 5))));
}

#[test]
fn enter_opens_a_closed_dropdown_then_commits_it() {
    let mut form = a_strict_form();
    form.focus_on("layout");
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert!(form.select_by_id("layout").unwrap().is_open());

    form.handle_key(key(BareKey::Down));
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FieldChanged("layout".to_string())
    );
    assert_eq!(form.text("layout"), "compact");
    assert!(!form.select_by_id("layout").unwrap().is_open());
}

#[test]
fn enter_toggles_a_checkbox() {
    let mut form = a_strict_form();
    form.focus_on("frames");
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FieldChanged("frames".to_string())
    );
    assert!(form.checked("frames"));
    form.handle_key(key(BareKey::Enter));
    assert!(!form.checked("frames"));
}

#[test]
fn enter_activates_a_radio_without_changing_it() {
    let mut form = a_strict_form();
    form.focus_on("mode");
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert_eq!(form.text("mode"), "One");
    assert_eq!(
        form.handle_key(key(BareKey::Right)),
        FormEvent::FieldChanged("mode".into())
    );
    assert_eq!(form.text("mode"), "Two");
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert_eq!(form.text("mode"), "Two");
}

#[test]
fn enter_on_the_button_is_the_only_submit() {
    let mut form = a_strict_form();
    form.focus_on("save");
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::ButtonPressed("save".to_string())
    );
}

/// `TextInput`'s bare-key arm ignores modifiers, so an unguarded Alt-Enter
/// comes back as the `Submit` this form promises never to emit.
#[test]
fn a_modified_enter_is_left_for_the_plugin() {
    let alt_enter = KeyWithModifier::new(BareKey::Enter).with_alt_modifier();
    for id in ["name", "layout", "frames", "mode"] {
        let mut form = a_strict_form();
        form.focus_on(id);
        assert_eq!(
            form.handle_key(alt_enter.clone()),
            FormEvent::Ignored,
            "{id}"
        );
    }

    let mut editing = a_strict_form();
    editing.handle_key(key(BareKey::Enter));
    assert_eq!(editing.handle_key(alt_enter), FormEvent::Ignored);
    assert!(
        editing.is_editing(),
        "a key it did not take changes nothing"
    );
}

#[test]
fn clicking_a_text_field_starts_editing() {
    let mut form = a_strict_form();
    form.focus_on("layout");
    form.render(0, 0, 40);
    let row = form.field_by_id("name").unwrap().last_rendered.unwrap().y;
    assert_eq!(
        form.handle_mouse(Mouse::LeftClick(row as isize, 8)),
        FormEvent::FocusMoved
    );
    assert_eq!(form.focused_id(), Some("name"));
    assert!(form.is_editing());
}

#[test]
fn pasted_text_lands_in_the_focused_field_and_starts_editing() {
    let mut form = a_strict_form();
    assert_eq!(form.insert_pasted_text("branch"), Some("name".to_string()));
    assert_eq!(form.text("name"), "branch");
    assert!(form.is_editing());

    form.focus_on("frames");
    assert_eq!(form.insert_pasted_text("nope"), None);
}

#[test]
fn the_first_widget_added_takes_focus() {
    assert_eq!(a_form().focused_id(), Some("name"));
}

#[test]
fn typing_goes_to_the_focused_field() {
    let mut form = a_form();
    let event = form.handle_key(key(BareKey::Char('a')));
    assert_eq!(event, FormEvent::FieldChanged("name".to_string()));
    assert_eq!(form.text("name"), "a");
    assert_eq!(form.text("path"), "");
}

#[test]
fn tab_traverses_every_widget_and_wraps() {
    let mut form = Form::new()
        .field("name", "Name", TextInput::empty())
        .select("kind", "Kind", Select::new(["One", "Two"]))
        .checkbox("frames", "Frames", Checkbox::new(false))
        .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
        .button("save", Button::new("Save"))
        .button("cancel", Button::new("Cancel"));
    for expected in ["kind", "frames", "mode", "save", "cancel", "name"] {
        assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
        assert_eq!(form.focused_id(), Some(expected));
    }
}

#[test]
fn cursor_tracks_the_focused_field_row_after_traversal() {
    let mut form = Form::new()
        .select("kind", "Kind", Select::new(["One", "Two"]))
        .field("name", "Name", TextInput::empty())
        .field("path", "Path", TextInput::empty());

    form.handle_key(key(BareKey::Tab));
    form.handle_key(key(BareKey::Enter));
    let rendered = form.render(2, 5, 40);
    let name_row = form.field_by_id("name").unwrap().last_rendered.unwrap().y;
    assert_eq!(rendered.cursor, Some(Some((8, name_row))));

    form.handle_key(key(BareKey::Tab));
    form.handle_key(key(BareKey::Enter));
    let rendered = form.render_in_viewport(2, 5, 40, 1);
    let path_row = form.field_by_id("path").unwrap().last_rendered.unwrap().y;
    assert_eq!(rendered.cursor, Some(Some((8, path_row))));
}

#[test]
fn shift_tab_traverses_backwards() {
    let mut form = a_form();
    let back = KeyWithModifier::new(BareKey::Tab).with_shift_modifier();
    assert_eq!(form.handle_key(back), FormEvent::FocusMoved);
    assert_eq!(form.focused_id(), Some("cancel"));
}

/// Tab reaching the field as a completion request would mean traversal
/// silently stopped working, which is the failure this ordering prevents.
#[test]
fn traversal_beats_the_fields_own_tab_handling() {
    let mut form = Form::new().field("name", "Name", TextInput::empty());
    assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
}

#[test]
fn custom_traversal_keeps_tab_traversal() {
    let mut form = Form::new()
        .field("name", "Name", TextInput::empty())
        .with_traversal_keys(
            KeyWithModifier::new(BareKey::Down),
            KeyWithModifier::new(BareKey::Up),
        );
    assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::FocusMoved);
}

#[test]
fn unhandled_keys_come_back_as_ignored() {
    let mut form = a_form();
    let event = form.handle_key(KeyWithModifier::new(BareKey::F(5)));
    assert_eq!(event, FormEvent::Ignored);
    assert!(!event.should_render());
}

#[test]
fn enter_edits_and_esc_leaves_editing_before_cancelling() {
    let mut form = a_form();
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
}

#[test]
fn enter_on_a_button_presses_it() {
    let mut form = a_form();
    form.focus_on("save");
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::ButtonPressed("save".to_string())
    );
}

#[test]
fn traversal_skips_a_disabled_button() {
    let mut form = Form::new()
        .field("name", "Name", TextInput::empty())
        .button("save", Button::new("Save").disabled(true))
        .button("cancel", Button::new("Cancel"));
    form.handle_key(key(BareKey::Tab));
    assert_eq!(form.focused_id(), Some("cancel"));
}

#[test]
fn a_disabled_button_cannot_be_pressed_by_key() {
    let mut form = Form::new().button("save", Button::new("Save").disabled(true));
    form.focus_on("save");
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Ignored);
}

#[test]
fn an_empty_form_ignores_everything() {
    let mut form = Form::new();
    assert_eq!(form.handle_key(key(BareKey::Tab)), FormEvent::Ignored);
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::Ignored);
    assert_eq!(form.focused_id(), None);
}

#[test]
fn a_click_before_any_render_hits_nothing() {
    let mut form = a_form();
    assert_eq!(
        form.handle_mouse(Mouse::LeftClick(0, 0)),
        FormEvent::Ignored,
        "a form that never rendered has no rectangles to hit"
    );
}

#[test]
fn a_negative_click_line_is_ignored() {
    let mut form = a_form();
    assert_eq!(
        form.handle_mouse(Mouse::LeftClick(-1, 0)),
        FormEvent::Ignored
    );
}

#[test]
fn height_counts_the_button_row_once() {
    assert_eq!(a_form().height(), 3);
    assert_eq!(
        Form::new().field("a", "A", TextInput::empty()).height(),
        1,
        "no buttons means no button row"
    );
}

#[test]
fn a_bounded_form_follows_the_focused_row() {
    let mut form = a_form();
    assert_eq!(form.viewport_start(2), 0);
    form.focus_on("save");
    assert_eq!(form.viewport_start(2), 1);
    form.focus_on("cancel");
    assert_eq!(form.viewport_start(2), 1, "buttons share one row");
    form.focus_on("name");
    assert_eq!(form.viewport_start(2), 0);
}

#[test]
fn a_narrow_form_keeps_each_focused_button_clickable() {
    let mut form = a_form();
    for id in ["save", "cancel"] {
        form.focus_on(id);
        form.render_in_viewport(0, 0, 5, 1);
        form.handle_mouse(Mouse::LeftClick(0, 1));
        form.render_in_viewport(0, 0, 5, 1);
        assert_eq!(
            form.handle_mouse(Mouse::Release(0, 1)),
            FormEvent::ButtonPressed(id.into())
        );
    }
}

#[test]
fn a_partially_visible_button_stays_under_the_pointer_until_release() {
    for scrolled in [false, true] {
        for release_inside in [false, true] {
            let mut form = Form::new();
            if scrolled {
                form = form.button("earlier", Button::new("Earlier"));
            }
            form = form
                .button("restart", Button::new("Restart"))
                .button("duplicate", Button::new("Duplicate"))
                .button("fork", Button::new("Fork"));
            form.focus_on("duplicate");
            form.render_buffer(0, 0, 24);
            assert_eq!(form.first_button, usize::from(scrolled));
            let fork = form.buttons.last().unwrap().1.last_rendered().unwrap();
            assert!(fork.columns < form.buttons.last().unwrap().1.width());
            assert_eq!(
                form.handle_mouse(Mouse::LeftClick(0, fork.x)),
                FormEvent::FocusMoved
            );
            assert_eq!(form.focused_id(), Some("fork"));
            form.render_buffer(0, 0, 24);
            assert_eq!(form.buttons.last().unwrap().1.last_rendered(), Some(fork));
            let release_x = if release_inside { fork.x } else { 0 };
            assert_eq!(
                form.handle_mouse(Mouse::Release(0, release_x)),
                if release_inside {
                    FormEvent::ButtonPressed("fork".into())
                } else {
                    FormEvent::Ignored
                }
            );
            form.render_buffer(0, 0, 24);
            assert_eq!(
                form.buttons
                    .last()
                    .unwrap()
                    .1
                    .last_rendered()
                    .unwrap()
                    .columns,
                form.buttons.last().unwrap().1.width()
            );
            form.handle_key(key(BareKey::Left));
            form.render_buffer(0, 0, 24);
            assert_eq!(form.focused_id(), Some("duplicate"));
            assert_eq!(form.first_button, usize::from(scrolled));
        }
    }
}

#[test]
fn widgets_outside_the_viewport_lose_their_hit_targets() {
    let mut form = a_form();
    form.render(0, 0, 40);
    assert!(form.fields[0].last_rendered.is_some());
    assert!(form.buttons[0].1.last_rendered().is_some());

    form.focus_on("path");
    form.render_in_viewport(0, 0, 40, 1);
    assert_eq!(form.fields[0].last_rendered, None);
    assert!(form.fields[1].last_rendered.is_some());
    assert_eq!(form.buttons[0].1.last_rendered(), None);
}

#[test]
fn focus_on_an_unknown_id_reports_failure_and_changes_nothing() {
    let mut form = a_form();
    assert!(!form.focus_on("nope"));
    assert_eq!(form.focused_id(), Some("name"));
}

#[test]
fn text_of_an_unknown_field_is_empty() {
    assert_eq!(a_form().text("nope"), "");
}

fn a_form_with_a_select() -> Form {
    Form::new()
        .field("name", "Name", TextInput::empty())
        .select(
            "layout",
            "Layout",
            Select::new(["default", "compact", "dev-three-pane"]),
        )
        .button("save", Button::new("Save"))
}

#[test]
fn a_select_field_reads_back_as_its_chosen_option() {
    let form = a_form_with_a_select();
    assert_eq!(form.text("layout"), "default");
    assert_eq!(
        form.select_by_id("layout").and_then(|s| s.selected_index()),
        Some(0)
    );
}

#[test]
fn typing_into_a_focused_select_opens_and_filters_it() {
    let mut form = a_form_with_a_select();
    form.focus_on("layout");
    let event = form.handle_key(key(BareKey::Char('c')));
    assert_eq!(event, FormEvent::FocusMoved);
    let select = form.select_by_id("layout").unwrap();
    assert!(select.is_open());
    assert_eq!(select.filter_text(), "c");
}

#[test]
fn enter_commits_the_dropdown_rather_than_submitting_the_form() {
    let mut form = a_form_with_a_select();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char('c')));
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FieldChanged("layout".to_string())
    );
    assert_eq!(form.text("layout"), "compact");
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FocusMoved,
        "with the dropdown closed, Enter opens it again"
    );
}

/// Esc has two things to back out of here, and the inner one goes first.
#[test]
fn esc_closes_the_dropdown_before_it_cancels_the_form() {
    let mut form = a_form_with_a_select();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
    assert!(!form.select_by_id("layout").unwrap().is_open());
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
}

#[test]
fn traversing_away_closes_an_open_dropdown() {
    let mut form = a_form_with_a_select();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));
    assert!(form.select_by_id("layout").unwrap().is_open());
    form.handle_key(key(BareKey::Tab));
    assert_eq!(form.focused_id(), Some("save"));
    assert!(
        !form.select_by_id("layout").unwrap().is_open(),
        "the dropdown outlived the focus that opened it"
    );
}

#[test]
fn an_open_dropdown_does_not_change_the_forms_height() {
    let mut form = a_form_with_a_select();
    let closed_height = form.height();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));
    assert_eq!(form.height(), closed_height);
}

#[test]
fn there_is_no_overlay_until_a_dropdown_has_been_drawn() {
    let mut form = a_form_with_a_select();
    assert_eq!(form.overlay_rect(), None);
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));
    assert_eq!(
        form.overlay_rect(),
        None,
        "a list that has never rendered covers nothing"
    );
}

#[test]
fn a_dropdown_is_clipped_to_the_form_viewport() {
    // Name, then the select, then the button row.
    let mut form = a_form_with_a_select();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));

    // Only the button row is left underneath, so only one option fits.
    form.render_in_viewport(0, 0, 40, 3);
    assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 1)));

    // Nothing is left underneath: the list is not drawn rather than the
    // form being scrolled to make space for it.
    form.render_in_viewport(0, 0, 40, 2);
    assert_eq!(form.overlay_rect(), None);
}

#[test]
fn a_bottom_row_dropdown_uses_the_rows_the_caller_lends_it() {
    let mut form = Form::new()
        .field("name", "Name", TextInput::empty())
        .select("layout", "Layout", Select::new(["default", "compact"]));
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));

    // Two rows of form, and two more below it that the caller owns.
    form.render_in_viewport_with_overlay(0, 0, 40, 2, 4);

    assert_eq!(
        form.fields[0].last_rendered,
        Some(Rect::new(0, 0, 40, 1)),
        "opening the list is not a reason to scroll the field above it away"
    );
    assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 2)));
    assert_eq!(
        click(&mut form, 3, 0),
        FormEvent::FieldChanged("layout".to_string())
    );
    assert_eq!(form.text("layout"), "compact");
}

/// The bug this replaced: the form scrolled a row to make space for the
/// list, taking the field above the select off the top of the viewport.
#[test]
fn opening_a_dropdown_does_not_scroll_a_form_that_already_fits() {
    let mut form = Form::new()
        .field("name", "Name", TextInput::empty())
        .select("layout", "Layout", Select::new(["default", "compact"]))
        .field("path", "Path", TextInput::empty())
        .button("save", Button::new("Save"));
    form.focus_on("layout");
    let rows = form.height();

    assert_eq!(form.viewport_start(rows), 0);
    form.handle_key(key(BareKey::Char(' ')));
    assert!(form.select_by_id("layout").unwrap().is_open());
    assert_eq!(form.viewport_start(rows), 0);

    form.render_in_viewport(0, 0, 40, rows);
    assert_eq!(form.fields[0].last_rendered, Some(Rect::new(0, 0, 40, 1)));
    assert_eq!(form.fields[1].last_rendered, Some(Rect::new(0, 1, 40, 1)));
}

#[test]
fn the_original_render_keeps_dropdowns_unbounded() {
    let mut form = a_form_with_a_select();
    form.focus_on("layout");
    form.handle_key(key(BareKey::Char(' ')));
    form.render(0, 0, 40);
    assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 3)));
}

/// A whole click - press and release on the same spot - the way the host
/// sends one.
fn click(form: &mut Form, line: isize, column: usize) -> FormEvent {
    form.handle_mouse(Mouse::LeftClick(line, column));
    form.handle_mouse(Mouse::Release(line, column))
}

fn a_form_with_choices() -> Form {
    Form::new()
        .checkbox("frames", "Frames", Checkbox::new(false))
        .radio("mode", "Mode", RadioGroup::new(["One", "Two"]))
        .button("save", Button::new("Save"))
}

#[test]
fn space_toggles_a_checkbox_and_typed_accessors_find_it() {
    let mut form = a_form_with_choices();
    assert!(!form.checked("frames"));
    assert_eq!(
        form.handle_key(key(BareKey::Char(' '))),
        FormEvent::FieldChanged("frames".to_string())
    );
    assert!(form.checked("frames"));
    assert!(form.checkbox_by_id_mut("frames").is_some());
    assert!(!form.checked("missing"));
}

#[test]
fn active_arrows_change_a_radio_and_text_reads_its_option() {
    let mut form = a_form_with_choices();
    form.focus_on("mode");
    form.handle_key(key(BareKey::Enter));
    assert_eq!(
        form.handle_key(key(BareKey::Right)),
        FormEvent::FieldChanged("mode".to_string())
    );
    assert_eq!(form.text("mode"), "Two");
    assert_eq!(
        form.radio_by_id("mode")
            .and_then(RadioGroup::selected_index),
        Some(1)
    );
    assert!(form.radio_by_id_mut("mode").is_some());
}

#[test]
fn active_radio_clicks_survive_a_render_between_press_and_release() {
    for render_between in [false, true] {
        for release_column in [14, 6, 20] {
            let mut form = Form::new().radio(
                "mode",
                "Mode",
                RadioGroup::new(["One", "Two", "Three"]).with_selected(1),
            );
            form.handle_key(key(BareKey::Enter));
            form.render_buffer(0, 0, 20);
            assert_eq!(form.radio_by_id("mode").unwrap().option_at(14, 0), Some(2));
            assert_eq!(
                form.handle_mouse(Mouse::LeftClick(0, 14)),
                FormEvent::FocusMoved
            );
            assert_eq!(form.text("mode"), "Two");
            if render_between {
                form.render_buffer(0, 0, 20);
            }
            let event = form.handle_mouse(Mouse::Release(0, release_column));
            if release_column == 14 {
                assert_eq!(event, FormEvent::FieldChanged("mode".into()));
                assert_eq!(form.text("mode"), "Three");
            } else {
                assert_eq!(event, FormEvent::Ignored);
                assert_eq!(form.text("mode"), "Two");
            }
            assert!(form.is_radio_active());
            assert!(form.focus_on("mode"));
            assert!(!form.is_radio_active());
        }
    }
}

#[test]
fn clicking_an_inactive_radio_does_not_activate_it() {
    let mut form = Form::new()
        .radio("first", "First", RadioGroup::new(["One", "Two"]))
        .radio("mode", "Mode", RadioGroup::new(["One", "Two", "Three"]));
    form.handle_key(key(BareKey::Enter));
    form.render_buffer(0, 0, 20);
    form.handle_mouse(Mouse::LeftClick(1, 14));
    assert_eq!(form.focused_id(), Some("mode"));
    assert!(!form.is_radio_active());
    assert_eq!(form.text("mode"), "One");
    form.render_buffer(0, 0, 20);
    assert_eq!(
        form.handle_mouse(Mouse::Release(1, 14)),
        FormEvent::FieldChanged("mode".into())
    );
    assert_eq!(form.text("mode"), "Two");
    assert!(!form.is_radio_active());
}

#[test]
fn clicking_choices_changes_only_when_the_value_changes() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);

    assert_eq!(
        click(&mut form, 0, 0),
        FormEvent::FieldChanged("frames".to_string())
    );
    assert!(form.checked("frames"));

    assert_eq!(
        click(&mut form, 1, 14),
        FormEvent::FieldChanged("mode".to_string())
    );
    assert_eq!(form.text("mode"), "Two");
    assert_eq!(
        click(&mut form, 1, 14),
        FormEvent::Ignored,
        "releasing on the option that is already selected changes nothing"
    );
}

/// A press is not a click. Everything below goes through `click`, which
/// sends both halves the way the host does; these are the cases where the
/// two halves disagree.
#[test]
fn a_press_alone_moves_focus_and_nothing_else() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);

    assert_eq!(
        form.handle_mouse(Mouse::LeftClick(0, 0)),
        FormEvent::FocusMoved
    );
    assert!(!form.checked("frames"), "the press alone toggled nothing");
    assert_eq!(form.focused_id(), Some("frames"));

    assert_eq!(
        form.handle_mouse(Mouse::Release(0, 0)),
        FormEvent::FieldChanged("frames".to_string())
    );
    assert!(form.checked("frames"));
}

#[test]
fn releasing_off_the_widget_takes_the_press_back() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);

    // Pressed on the checkbox, released on the radio row below it.
    form.handle_mouse(Mouse::LeftClick(0, 0));
    assert_eq!(form.handle_mouse(Mouse::Release(1, 0)), FormEvent::Ignored);
    assert!(!form.checked("frames"));

    // And the release does not act on what it landed on either.
    assert_eq!(form.text("mode"), "One");
}

#[test]
fn a_press_dragged_off_a_button_and_back_still_fires() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);
    let button_y = form.buttons[0].1.last_rendered().unwrap().y as isize;

    form.handle_mouse(Mouse::LeftClick(button_y, 1));
    // Off the button, then back onto it: only where the release lands counts.
    form.handle_mouse(Mouse::Hold(button_y, 30));
    form.handle_mouse(Mouse::Hold(button_y, 1));
    assert_eq!(
        form.handle_mouse(Mouse::Release(button_y, 1)),
        FormEvent::ButtonPressed("save".to_string())
    );

    form.handle_mouse(Mouse::LeftClick(button_y, 1));
    assert_eq!(
        form.handle_mouse(Mouse::Release(button_y, 30)),
        FormEvent::Ignored,
        "released past the end of the button"
    );
}

#[test]
fn a_release_without_a_press_does_nothing() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);
    assert_eq!(form.handle_mouse(Mouse::Release(0, 0)), FormEvent::Ignored);
    assert!(!form.checked("frames"));
}

/// The pointer leaving the pane arrives as buttonless motion, which means
/// the button came up somewhere this form never heard about.
#[test]
fn a_hover_between_press_and_release_cancels_the_press() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);

    form.handle_mouse(Mouse::LeftClick(0, 0));
    form.handle_mouse(Mouse::Hover(0, u16::MAX as usize));
    assert_eq!(form.handle_mouse(Mouse::Release(0, 0)), FormEvent::Ignored);
    assert!(!form.checked("frames"));
}

#[test]
fn a_disabled_button_refuses_the_mouse() {
    let mut form = Form::new()
        .field("name", "Name", TextInput::empty())
        .button("save", Button::new("Save").disabled(true));
    form.render(0, 0, 40);
    let button_y = form.buttons[0].1.last_rendered().unwrap().y as isize;
    assert_eq!(click(&mut form, button_y, 1), FormEvent::Ignored);
}

#[test]
fn hovering_highlights_the_choice_row_under_the_pointer() {
    let mut form = a_form_with_choices();
    form.render(0, 0, 40);

    assert_eq!(
        form.handle_mouse(Mouse::Hover(0, 0)),
        FormEvent::FocusMoved,
        "the checkbox row took the highlight"
    );
    assert_eq!(
        form.handle_mouse(Mouse::Hover(0, 0)),
        FormEvent::Ignored,
        "nothing changed the second time"
    );
    assert_eq!(
        form.handle_mouse(Mouse::Hover(1, 14)),
        FormEvent::FocusMoved
    );
    // The leave sentinel the host sends when the pointer goes elsewhere.
    assert_eq!(
        form.handle_mouse(Mouse::Hover(0, u16::MAX as usize)),
        FormEvent::FocusMoved
    );
    assert_eq!(
        form.handle_mouse(Mouse::Hover(0, u16::MAX as usize)),
        FormEvent::Ignored
    );
}

#[test]
fn enter_activates_choice_fields() {
    let mut form = a_form_with_choices();
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FieldChanged("frames".into())
    );
    form.focus_on("mode");
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
}

fn a_form_with_a_combo() -> Form {
    Form::new()
        .field("name", "Name", TextInput::empty())
        .combo("model", "Model", Combo::new(["opus", "sonnet", "haiku"]))
        .button("save", Button::new("Save"))
}

#[test]
fn a_combo_field_reads_back_as_its_text() {
    let mut form = a_form_with_a_combo();
    assert_eq!(form.text("model"), "");
    form.combo_by_id_mut("model").unwrap().set_text("gpt-5");
    assert_eq!(
        form.text("model"),
        "gpt-5",
        "a combo holds text, not an option index"
    );
    assert!(form.combo_by_id("model").is_some());
    assert!(form.field_by_id("model").unwrap().is_combo());
    assert!(form.combo_by_id("name").is_none());
}

#[test]
fn typing_into_a_focused_combo_fills_it_in_and_offers_the_matches() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    assert_eq!(
        form.handle_key(key(BareKey::Char('o'))),
        FormEvent::FieldChanged("model".to_string())
    );
    assert_eq!(form.text("model"), "o");
    assert!(form.combo_by_id("model").unwrap().is_open());
}

/// The guarantee the widget exists for: a value nobody picked off the list
/// is still the value.
#[test]
fn enter_on_a_combo_accepts_what_was_typed() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    form.handle_key(key(BareKey::Char('o')));
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert_eq!(form.text("model"), "o");
}

#[test]
fn arrowing_into_the_list_makes_enter_take_a_suggestion() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    form.handle_key(key(BareKey::Char('o')));
    assert_eq!(form.handle_key(key(BareKey::Down)), FormEvent::FocusMoved);
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FieldChanged("model".to_string())
    );
    assert_eq!(form.text("model"), "opus");
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FocusMoved,
        "with the list closed, Enter starts editing again"
    );
}

#[test]
fn esc_closes_a_combo_list_before_it_cancels_the_form() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    form.handle_key(key(BareKey::Enter));
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
    assert!(!form.combo_by_id("model").unwrap().is_open());
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::FocusMoved);
    assert_eq!(form.handle_key(key(BareKey::Esc)), FormEvent::Cancel);
}

#[test]
fn traversing_away_closes_an_open_combo_list() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    form.handle_key(key(BareKey::Enter));
    assert!(form.combo_by_id("model").unwrap().is_open());
    form.handle_key(key(BareKey::Tab));
    assert_eq!(form.focused_id(), Some("save"));
    assert!(!form.combo_by_id("model").unwrap().is_open());
}

#[test]
fn a_paste_into_a_combo_lands_in_it_and_offers_the_matches() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    assert_eq!(form.insert_pasted_text("son"), Some("model".to_string()));
    assert_eq!(form.text("model"), "son");
    assert!(form.combo_by_id("model").unwrap().is_open());
}

#[test]
fn an_open_combo_list_is_an_overlay_like_a_dropdown() {
    let mut form = a_form_with_a_combo();
    let closed_height = form.height();
    form.focus_on("model");
    form.handle_key(key(BareKey::Enter));
    assert_eq!(form.height(), closed_height);
    assert_eq!(
        form.overlay_rect(),
        None,
        "a list that has never rendered covers nothing"
    );
    form.render(0, 0, 40);
    assert_eq!(form.overlay_rect(), Some(Rect::new(0, 2, 40, 3)));
}

#[test]
fn clicking_a_suggestion_takes_it() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    form.handle_key(key(BareKey::Enter));
    form.render(0, 0, 40);
    assert_eq!(
        click(&mut form, 3, 0),
        FormEvent::FieldChanged("model".to_string())
    );
    assert_eq!(form.text("model"), "sonnet");
    assert!(!form.combo_by_id("model").unwrap().is_open());
}

/// The marker is the only part of the row that is a control: everywhere else
/// is text, and a click there is a click into text.
#[test]
fn the_marker_toggles_the_list_and_the_rest_of_the_row_takes_the_caret() {
    let mut form = a_form_with_a_combo();
    form.combo_by_id_mut("model").unwrap().set_text("sonnet");
    form.render(0, 0, 40);

    assert_eq!(click(&mut form, 1, 39), FormEvent::FocusMoved);
    assert!(form.combo_by_id("model").unwrap().is_open());
    form.render(0, 0, 40);
    assert_eq!(click(&mut form, 1, 39), FormEvent::FocusMoved);
    assert!(!form.combo_by_id("model").unwrap().is_open());

    assert_eq!(
        click(&mut form, 1, 9),
        FormEvent::Ignored,
        "a click into the text is not a click on anything to activate"
    );
    assert!(!form.combo_by_id("model").unwrap().is_open());
    assert_eq!(
        form.field_by_id("model")
            .and_then(Field::input)
            .map(TextInput::get_cursor_position),
        Some(2),
        "the caret goes where the click landed"
    );
}

#[test]
fn a_combo_accepts_a_suggestion_and_ends_editing() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    assert_eq!(
        form.handle_key(key(BareKey::Char('o'))),
        FormEvent::FieldChanged("model".to_string())
    );
    assert_eq!(form.handle_key(key(BareKey::Down)), FormEvent::FocusMoved);
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FieldChanged("model".to_string())
    );
    assert_eq!(form.text("model"), "opus");
    assert_eq!(
        form.handle_key(key(BareKey::Enter)),
        FormEvent::FocusMoved,
        "with the list closed, Enter is the input mode's again"
    );
}

#[test]
fn enter_on_a_combo_list_with_nothing_highlighted_leaves_the_input_mode() {
    let mut form = a_form_with_a_combo();
    form.focus_on("model");
    form.handle_key(key(BareKey::Char('o')));
    assert!(form.combo_by_id("model").unwrap().is_open());
    assert_eq!(form.handle_key(key(BareKey::Enter)), FormEvent::FocusMoved);
    assert_eq!(form.text("model"), "o");
    assert!(!form.editing, "the second Enter left the input mode");
}
