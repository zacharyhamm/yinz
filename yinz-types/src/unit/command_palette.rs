use super::*;

#[test]
fn palette_display_validation_rejects_ambiguous_controls_and_oversize_payloads() {
    let action = Action {
        id: "id".into(),
        label: "Action".into(),
        description: String::new(),
        handle: "1".into(),
    };
    assert!(Reply::Actions {
        actions: vec![action.clone()]
    }
    .validate()
    .is_ok());
    assert!(Reply::Actions {
        actions: vec![action.clone(); 2]
    }
    .validate()
    .is_err());
    assert!(Reply::Actions {
        actions: vec![action.clone(); 101]
    }
    .validate()
    .is_err());
    for invalid in ["", " ", "\x1b[31m", "line\rreset"] {
        let mut bad = action.clone();
        bad.label = invalid.into();
        assert!(Reply::Actions { actions: vec![bad] }.validate().is_err());
    }
    let form = |fields| Reply::View {
        view: View::Form {
            title: "Form".into(),
            fields,
            submit_label: "Submit".into(),
            handle: "2".into(),
        },
    };
    let field = Field::Choice {
        id: "field".into(),
        label: "Choice".into(),
        options: vec!["a".into()],
        initial_value: Some("b".into()),
    };
    assert!(form(vec![field]).validate().is_err());
    let field = Field::Text {
        id: "field".into(),
        label: "Text".into(),
        initial_value: String::new(),
    };
    assert!(form(vec![field.clone()]).validate().is_ok());
    assert!(form(vec![field.clone(); 2]).validate().is_err());
    assert!(form(vec![field; 17]).validate().is_err());
    assert!(Reply::View {
        view: View::Text {
            title: "Text".into(),
            text: "line\n\tindent".into()
        }
    }
    .validate()
    .is_ok());
    assert!(Reply::View {
        view: View::Text {
            title: "Text".into(),
            text: "x".repeat(MAX_DISPLAY_BYTES)
        }
    }
    .validate()
    .is_err());
}
