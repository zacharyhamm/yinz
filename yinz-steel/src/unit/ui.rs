use crate::{to_scheme, Runtime};
use serde_json::json;
use yinz_tile::ui_components::{serialize_text_with_coordinates, Text};
use yinz_types::data::{BareKey, KeyWithModifier};

fn runtime() -> Runtime {
    Runtime::new(|_| anyhow::bail!("pure widgets must not call the host")).unwrap()
}

#[test]
fn styled_widgets_use_the_shared_render_protocol_and_validate_indices() {
    let mut runtime = runtime();
    runtime.eval("(define output (yinz-ui-text (yinz-ui-styled \"日ab\" (hash \"selected\" #t \"opaque\" #t \"disabled\" #t \"levels\" '((0) (1)))) (yinz-ui-coordinates 2 3 12 1)))").unwrap();
    let text = Text::new("日ab")
        .selected()
        .opaque()
        .disabled()
        .color_indices(0, vec![0])
        .color_indices(1, vec![1]);
    assert_eq!(
        runtime.value("output").unwrap(),
        json!(serialize_text_with_coordinates(
            &text,
            2,
            3,
            Some(12),
            Some(1)
        ))
    );
    assert!(runtime
        .eval("(yinz-ui-text (hash \"text\" \"x\" \"levels\" '((1))))")
        .is_err());
    assert!(runtime
        .eval("(yinz-ui-table '((\"a\") (\"b\" \"c\")))")
        .is_err());
    assert!(runtime
        .eval("(yinz-ui-ribbon \"x\" (yinz-ui-coordinates 0 0 1000000 1))")
        .is_err());
    assert_eq!(runtime.take_output().unwrap(), "");
}

#[test]
fn pure_render_dimensions_can_remain_unspecified() {
    let mut runtime = runtime();
    runtime
        .eval("(define output (yinz-ui-text \"x\" (yinz-ui-coordinates 2 3 json-null json-null)))")
        .unwrap();
    assert_eq!(
        runtime.value("output").unwrap(),
        json!(serialize_text_with_coordinates(
            &Text::new("x"),
            2,
            3,
            None,
            None
        ))
    );
    assert!(runtime
        .eval("(yinz-ui-text \"x\" (yinz-ui-coordinates 0 0 json-null 1000000))")
        .is_err());
}

#[test]
fn forms_keep_native_edit_focus_selection_and_cursor_state() {
    let mut runtime = runtime();
    runtime.eval("(define form (yinz-ui-form (list (yinz-ui-input \"name\" \"Name\" \"\") (yinz-ui-checkbox \"enabled\" \"Enabled\" #f) (yinz-ui-select \"choice\" \"Choice\" '(\"a\" \"b\") 0) (yinz-ui-button \"save\" \"Save\"))))").unwrap();
    let key = serde_json::to_value(KeyWithModifier::new(BareKey::Char('日'))).unwrap();
    runtime
        .engine
        .register_value("key", to_scheme(&key).unwrap());
    runtime.eval("(define action (yinz-ui-handle-key form (hash \"Key\" key))) (yinz-ui-set-value! form \"enabled\" #t) (yinz-ui-set-value! form \"choice\" 1) (define values (yinz-ui-values form)) (define frame (yinz-ui-render-buffer form (yinz-ui-coordinates 2 3 20 6)))").unwrap();
    assert_eq!(runtime.value("action").unwrap()["kind"], "fieldChanged");
    assert_eq!(
        runtime.value("values").unwrap(),
        json!({"name":"日", "enabled":true,"choice":1})
    );
    let frame = runtime.value("frame").unwrap();
    assert_eq!(frame["updateCursor"], true);
    assert!(frame["cursor"].is_array());
    assert!(!frame["text"].as_str().unwrap().is_empty());
    runtime
        .eval("(yinz-ui-focus! form \"save\") (define focused (yinz-ui-focused-id form))")
        .unwrap();
    assert_eq!(runtime.value("focused").unwrap(), "save");
    assert!(runtime
        .eval("(yinz-ui-set-value! form \"choice\" 20)")
        .is_err());
    assert!(runtime.eval("(yinz-command form)").is_err());
}

#[test]
fn text_editor_history_undo_and_redo_share_existing_widget_behavior() {
    let mut runtime = runtime();
    runtime.eval("(define form (yinz-ui-form (list (yinz-ui-input \"edit\" \"\" \"a\")))) (yinz-ui-input-op form \"edit\" \"InsertText\" \"b\") (define edited (yinz-ui-value form \"edit\")) (yinz-ui-input-op form \"edit\" \"Undo\") (define undone (yinz-ui-value form \"edit\")) (yinz-ui-input-op form \"edit\" \"Redo\") (define redone (yinz-ui-value form \"edit\")) (yinz-ui-input-op form \"edit\" \"SetHistory\" '(\"first\" \"second\")) (define history (yinz-ui-input-op form \"edit\" \"History\"))").unwrap();
    assert_eq!(runtime.value("edited").unwrap(), "ab");
    assert_eq!(runtime.value("undone").unwrap(), "a");
    assert_eq!(runtime.value("redone").unwrap(), "ab");
    assert_eq!(
        runtime.value("history").unwrap(),
        json!(["first", "second"])
    );
}

#[test]
fn markdown_dialog_and_image_render_without_host_authority() {
    let mut runtime = runtime();
    runtime.eval("(define markdown (yinz-ui-markdown \"# Heading\\n\\n**bold** text\")) (define lines (yinz-ui-lines markdown 20)) (define frame (yinz-ui-render-buffer markdown (yinz-ui-coordinates 0 0 20 5))) (define dialog (yinz-ui-dialog \"Title\" (list (yinz-ui-input \"x\" \"X\" \"value\")))) (define dialog-frame (yinz-ui-render-buffer dialog (yinz-ui-coordinates 0 0 30 8))) (define image (yinz-ui-image-place 1 2 3))").unwrap();
    assert!(!runtime
        .value("lines")
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        runtime.value("frame").unwrap()["totalRows"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert!(!runtime.value("dialog-frame").unwrap()["text"]
        .as_str()
        .unwrap()
        .is_empty());
    assert!(runtime
        .value("image")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("\x1b_G"));
    assert!(runtime.eval("(yinz-ui-image-place 0 2 3)").is_err());
    assert!(runtime.eval("(yinz-ui-image-row 1 10000 2)").is_err());
}

#[test]
fn text_and_list_helpers_share_display_width_and_fuzzy_rules() {
    let mut runtime = runtime();
    runtime.eval("(define wrapped (yinz-ui-wrap \"日本語\" 4)) (define centered (yinz-ui-center \"日\" 6)) (define window (yinz-ui-list-window 100 5 50)) (define matches (yinz-ui-fuzzy-filter '(\"apple\" \"banana\") \"bn\"))").unwrap();
    assert_eq!(runtime.value("wrapped").unwrap(), json!(["日本", "語"]));
    assert_eq!(runtime.value("centered").unwrap(), "  日  ");
    assert_eq!(runtime.value("window").unwrap(), json!([48, 50, 53]));
    assert_eq!(runtime.value("matches").unwrap()[0]["index"], 1);
}

#[test]
fn dialogs_keep_styled_title_message_help_and_status() {
    let mut runtime = runtime();
    runtime.eval(r#"(define dialog (yinz-ui-dialog (hash "text" "Title" "levels" '(() () (0 1 2 3 4))) (list (yinz-ui-button "ok" "Okay")) (hash "message" (hash "text" "Message" "levels" '((0))) "help" "Help"))) (define height (yinz-ui-height dialog)) (yinz-ui-set-status! dialog (hash "text" "Notice" "levels" '(() () () (0)))) (define taller (yinz-ui-height dialog)) (define frame (yinz-ui-render-buffer dialog (yinz-ui-coordinates 0 0 30 10)))"#).unwrap();
    assert_eq!(
        runtime.value("taller").unwrap().as_u64().unwrap(),
        runtime.value("height").unwrap().as_u64().unwrap() + 1
    );
    assert!(runtime.value("frame").unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("84,105,116,108,101"));
    assert!(runtime
        .eval(r#"(yinz-ui-set-status! dialog (hash "text" "x" "levels" '((1))))"#)
        .is_err());
}

#[test]
fn layout_diagnostics_preserve_source_and_reject_invalid_descriptors() {
    let mut runtime = runtime();
    runtime.eval(r#"(define valid (yinz-format-layout-error "layout { pane; }")) (define invalid (yinz-format-layout-error "layout { pane size=oops; }")) (define syntax (yinz-format-layout-error "SyntaxError"))"#).unwrap();
    assert_eq!(runtime.value("valid").unwrap(), "");
    let invalid = runtime.value("invalid").unwrap();
    assert!(invalid.as_str().unwrap().contains("parse_layout_api"));
    assert!(runtime
        .value("syntax")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("Missing `;`"));
    assert!(runtime
        .eval("(yinz-format-layout-error (hash \"invalid\" #t))")
        .is_err());
}

#[test]
fn ansi_truncation_retains_colors_and_terminal_columns() {
    let mut runtime = runtime();
    let source = "\x1b[31m日ab\x1b[0m";
    assert_eq!(
        runtime
            .invoke("yinz-ui-truncate-ansi", vec![json!(source), json!(3)])
            .unwrap(),
        "\x1b[31m日a\x1b[0m"
    );
    assert_eq!(
        runtime
            .invoke("yinz-ui-truncate-ansi", vec![json!(source), json!(0)])
            .unwrap(),
        "\x1b[0m"
    );
    assert_eq!(
        runtime
            .invoke("yinz-ui-truncate-ansi", vec![json!(source), json!(20)])
            .unwrap(),
        format!("{source}\x1b[0m")
    );
}
