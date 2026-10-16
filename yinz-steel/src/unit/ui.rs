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
fn widgets_and_pure_rendering_accept_ordinary_large_viewports() {
    let mut runtime = runtime();
    runtime
        .eval(
            r##"
        (define fields (list (yinz-ui-input "name" "Name" "value")))
        (define form (yinz-ui-form fields))
        (define dialog (yinz-ui-dialog "Title" fields))
        (define markdown (yinz-ui-markdown "# Heading\n\nBody"))
        (define (frames coordinates)
          (map (lambda (widget) (yinz-ui-render-buffer widget coordinates))
               (list form dialog markdown)))
    "##,
        )
        .unwrap();
    for (width, height) in [(240, 80), (320, 120), (512, 512)] {
        let coordinates = json!({"x":2,"y":3,"width":width,"height":height});
        let frames = runtime.invoke("frames", vec![coordinates.clone()]).unwrap();
        for frame in frames.as_array().unwrap() {
            assert!(!frame["text"].as_str().unwrap().is_empty());
        }
        assert_eq!(
            runtime
                .invoke("yinz-ui-text", vec![json!("x"), coordinates])
                .unwrap(),
            serialize_text_with_coordinates(&Text::new("x"), 2, 3, Some(width), Some(height))
        );
    }
    for (width, height) in [(0, 0), (0, 80), (240, 0)] {
        let frames = runtime
            .invoke(
                "frames",
                vec![json!({"x":0,"y":0,"width":width,"height":height})],
            )
            .unwrap();
        for frame in frames.as_array().unwrap() {
            assert_eq!(frame["text"], "");
        }
    }
}

#[test]
fn viewport_limits_keep_area_dimensions_and_offsets_bounded() {
    let mut runtime = runtime();
    runtime
        .eval(
            r#"
        (define form (yinz-ui-form (list (yinz-ui-input "name" "Name" "value"))))
        (define (frame coordinates) (yinz-ui-render-buffer form coordinates))
    "#,
        )
        .unwrap();
    for (x, y, width, height) in [
        (0, 0, 512, 513),
        (0, 0, 262_145, 1),
        (0, 0, 16_385, 0),
        (0, 0, 0, 16_385),
        (0, 0, usize::MAX, 2),
        (0, 0, 2, usize::MAX),
        (crate::MAX_OUTPUT_BYTES + 1, 0, 1, 1),
        (0, crate::MAX_OUTPUT_BYTES + 1, 1, 1),
    ] {
        let coordinates = json!({"x":x,"y":y,"width":width,"height":height});
        assert!(runtime.invoke("frame", vec![coordinates.clone()]).is_err());
        assert!(runtime
            .invoke("yinz-ui-text", vec![json!("x"), coordinates])
            .is_err());
    }
}

#[test]
fn forms_example_renders_at_240_by_80() {
    let mut runtime = Runtime::new(|command| {
        assert!(command.get("ShowCursor").is_some());
        Ok(serde_json::Value::Null)
    })
    .unwrap();
    runtime
        .eval(include_str!("../../../example-plugins/forms.scm"))
        .unwrap();
    let output = runtime.render(80, 240).unwrap();
    for label in ["Pane frames", "Shared widgets"] {
        let encoded = label
            .bytes()
            .map(|byte| byte.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(output.contains(&encoded), "missing {label}");
    }
    runtime.eval("(set! confirming #t)").unwrap();
    assert!(!runtime.render(80, 240).unwrap().is_empty());
}

#[test]
fn large_viewports_keep_the_four_mib_output_limit() {
    let mut runtime = runtime();
    // Four-byte cells fill the allowed area but exceed 4 MiB when serialized.
    let source = ("\u{10ffff}".repeat(16_384) + "\n\n").repeat(16);
    runtime
        .engine
        .register_value("source", to_scheme(&json!(source)).unwrap());
    runtime
        .eval("(define markdown (yinz-ui-markdown source (hash \"compact\" #t)))")
        .unwrap();
    let error = runtime
        .eval("(yinz-ui-render-buffer markdown (yinz-ui-coordinates 0 0 16384 16))")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("UI output exceeds transport budget"),
        "{error}"
    );
    let error = runtime
        .eval("(yinz-ui-text (make-string 1048576 #\\x))")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("UI output exceeds transport budget"),
        "{error}"
    );
}

#[test]
fn png_bytevectors_keep_the_shared_encoder_and_eight_mib_source_limit() {
    use yinz_tile::ui_components::{png_transmit_sequence, MAX_IMAGE_ID};

    let mut runtime = runtime();
    for size in [128 * 1024, 2_572_749, crate::MAX_PNG_BYTES] {
        runtime.eval(&format!("(define png (make-bytes {size} 255)) (define output (yinz-ui-image-png {MAX_IMAGE_ID} png))")).unwrap();
        assert_eq!(
            runtime.value("output").unwrap(),
            png_transmit_sequence(MAX_IMAGE_ID, &vec![255; size])
        );
    }
    let error = runtime
        .eval("(yinz-ui-image-png 1 (make-bytes 8388609 0))")
        .unwrap_err();
    assert!(
        error.to_string().contains("PNG source exceeds 8 MiB"),
        "{error}"
    );
    assert!(runtime
        .eval("(yinz-json-string (make-bytes 131072 0))")
        .is_err());
}

#[test]
fn png_lists_vectors_and_pure_operation_keep_validation() {
    use yinz_tile::ui_components::png_transmit_sequence;

    let mut runtime = runtime();
    let expected = png_transmit_sequence(1, &[0, 128, 255]);
    assert_eq!(
        runtime
            .invoke(
                "yinz-ui-pure",
                vec![json!({"op":"ImagePng","args":[1,[0,128,255]]})]
            )
            .unwrap(),
        expected
    );
    for bytes in [
        "'(0 128 255)",
        "(immutable-vector 0 128 255)",
        "(bytes 0 128 255)",
    ] {
        runtime
            .eval(&format!("(define output (yinz-ui-image-png 1 {bytes}))"))
            .unwrap();
        assert_eq!(runtime.value("output").unwrap(), expected);
        for id in ["0", "-1", "16777216", "4294967297", "1.5", "#t"] {
            assert!(
                runtime
                    .eval(&format!("(yinz-ui-image-png {id} {bytes})"))
                    .is_err(),
                "accepted id {id}"
            );
            assert!(
                runtime
                    .eval(&format!("(%ui-pure \"ImagePng\" (list {id} {bytes}))"))
                    .is_err(),
                "accepted id {id}"
            );
        }
    }
    for bytes in [
        "'(-1)",
        "'(256)",
        "'(1.5)",
        "'(#t)",
        "'(\"0\")",
        "(immutable-vector 256)",
    ] {
        assert!(
            runtime
                .eval(&format!("(yinz-ui-image-png 1 {bytes})"))
                .is_err(),
            "accepted {bytes}"
        );
        assert!(
            runtime
                .eval(&format!("(%ui-pure \"ImagePng\" (list 1 {bytes}))"))
                .is_err(),
            "accepted {bytes}"
        );
    }
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

#[test]
fn image_constants_match_the_shared_encoder() {
    let runtime = runtime();
    assert_eq!(
        runtime.value("yinz-ui-image-max-id").unwrap(),
        yinz_tile::ui_components::MAX_IMAGE_ID
    );
    assert_eq!(
        runtime.value("yinz-ui-image-max-dimension").unwrap(),
        yinz_types::kitty_placeholders::ROWCOLUMN_DIACRITICS.len()
    );
    assert_eq!(
        runtime.value("yinz-ui-image-max-bytes").unwrap(),
        crate::MAX_PNG_BYTES
    );
}

#[test]
fn standalone_text_input_reuses_unicode_editing_paste_and_cursor_dispatch() {
    let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let received = commands.clone();
    let mut runtime = Runtime::new(move |command| {
        received.lock().unwrap().push(command);
        Ok(serde_json::Value::Null)
    })
    .unwrap();
    runtime
        .eval(
            r#"
      (define input (yinz-ui-text-input "/" "日a"))
      (yinz-ui-text-input-op input "SetCursorPosition" 1)
      (yinz-ui-text-input-op input "InsertText" "é")
      (define text (yinz-ui-text-input-op input "Text"))
      (define cursor (yinz-ui-text-input-op input "CursorPosition"))
      (yinz-ui-paste! input "界")
      (define pasted (yinz-ui-text-input-op input "Text"))
      (define buffer (yinz-ui-render-buffer input (yinz-ui-coordinates 0 0 8 1)))
    "#,
        )
        .unwrap();
    assert_eq!(runtime.value("text").unwrap(), "日éa");
    assert_eq!(runtime.value("cursor").unwrap(), 2);
    assert_eq!(runtime.value("pasted").unwrap(), "日é界a");
    assert!(commands.lock().unwrap().is_empty());
    runtime
        .eval("(yinz-ui-render input (yinz-ui-coordinates 0 0 8 1))")
        .unwrap();
    assert!(commands
        .lock()
        .unwrap()
        .iter()
        .any(|command| command.get("ShowCursor").is_some()));
    runtime.eval("(yinz-ui-text-input-op input \"ReplaceText\" '(\"é\" 1)) (yinz-ui-handle-key input (hash \"bare_key\" \"Backspace\" \"key_modifiers\" '()))").unwrap();
    assert_eq!(
        runtime
            .eval("(yinz-ui-text-input-op input \"Text\")")
            .unwrap()
            .values,
        ["\"\""]
    );
}
