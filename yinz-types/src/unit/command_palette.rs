use super::*;

#[test]
fn palette_display_validation_rejects_ambiguous_controls_and_oversize_payloads() {
    let action = Action {
        aliases: vec![],
        id: "id".into(),
        label: "Action".into(),
        description: String::new(),
        handle: "1".into(),
        cells: None,
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
            description: String::new(),
            actions: vec![],
            confirmation: false,
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

#[test]
fn palette_alias_defaults_validation_and_utf8_display_bound() {
    let action: Action = serde_json::from_value(serde_json::json!({
        "id": "pdf", "label": "Open PDF…", "handle": "1"
    }))
    .unwrap();
    assert!(action.aliases.is_empty());
    let command: Command = serde_json::from_value(serde_json::json!({
        "type": "register", "id": "pdf", "title": "PDF", "registration": "1"
    }))
    .unwrap();
    assert!(matches!(
        command,
        Command::Register {
            static_actions: false,
            ..
        }
    ));
    for count in [0, 100, 101] {
        let actions = (0..count)
            .map(|i| Action {
                id: i.to_string(),
                ..action.clone()
            })
            .collect();
        assert_eq!(Reply::Actions { actions }.validate().is_ok(), count <= 100);
    }
    for aliases in [
        serde_json::json!("reader"),
        serde_json::json!([1]),
        serde_json::json!(null),
    ] {
        let mut value = serde_json::to_value(&action).unwrap();
        value["aliases"] = aliases;
        assert!(serde_json::from_value::<Action>(value).is_err());
    }
    for alias in ["", " ", "\n", "\t", "\u{1b}", "\u{85}"] {
        let bad = Action {
            aliases: vec![alias.into()],
            ..action.clone()
        };
        assert!(Reply::Actions { actions: vec![bad] }.validate().is_err());
    }
    let mut reply = Reply::Actions {
        actions: vec![Action {
            aliases: vec!["読".into()],
            ..action
        }],
    };
    let remaining = MAX_DISPLAY_BYTES - serde_json::to_vec(&reply).unwrap().len();
    let Reply::Actions { actions } = &mut reply else {
        unreachable!()
    };
    actions[0].aliases[0].push_str(&"x".repeat(remaining));
    assert_eq!(serde_json::to_vec(&reply).unwrap().len(), MAX_DISPLAY_BYTES);
    assert!(reply.validate().is_ok());
    let Reply::Actions { actions } = &mut reply else {
        unreachable!()
    };
    actions[0].aliases[0].push('読');
    assert!(reply.validate().is_err());
}

#[test]
fn palette_completion_validates_text_views_and_releases_handles() {
    for notification in ["", "  ", "bad\x1b[31m", "bad\ntext"] {
        assert!(Reply::Complete {
            notification: notification.into(),
            view: None
        }
        .validate()
        .is_err());
    }
    assert!(Reply::Complete {
        notification: "x".repeat(MAX_DISPLAY_BYTES),
        view: None
    }
    .validate()
    .is_err());
    let reply = Reply::Complete {
        notification: "Report saved".into(),
        view: Some(View::Form {
            description: String::new(),
            actions: vec![],
            confirmation: false,
            title: "Report".into(),
            fields: vec![],
            submit_label: "Save".into(),
            handle: "submit".into(),
        }),
    };
    assert!(reply.validate().is_ok());
    assert_eq!(reply.handles(), vec!["submit"]);
    assert_eq!(
        serde_json::from_str::<Reply>(&serde_json::to_string(&reply).unwrap()).unwrap(),
        reply
    );
    assert!(Reply::Complete {
        notification: "Done".into(),
        view: Some(View::Text {
            title: "Result".into(),
            text: "\x1b".into()
        })
    }
    .validate()
    .is_err());
}

#[test]
fn form_secondary_callbacks_are_validated_and_released() {
    let mut reply: Reply = serde_json::from_value(serde_json::json!({
        "type": "view", "view": {"type": "form", "title": "Create", "submitLabel": "Create",
            "handle": "submit", "confirmation": true,
            "fields": [{"type": "directory", "id": "cwd", "label": "Directory", "initialValue": "/tmp"}],
            "actions": [{"id": "configure", "label": "Configure", "handle": "secondary"}]}
    })).unwrap();
    assert!(reply.validate().is_ok());
    assert_eq!(reply.handles(), ["submit", "secondary"]);
    let Reply::View {
        view: View::Form { actions, .. },
    } = &mut reply
    else {
        unreachable!()
    };
    actions.push(actions[0].clone());
    assert!(reply.validate().is_err());
    assert!(serde_json::from_value::<Reply>(
        serde_json::json!({"type":"fields","values":{"cwd":1}})
    )
    .is_err());
}

#[test]
fn view_descriptions_default_to_empty_and_validate_display_text() {
    for mut value in [
        serde_json::json!({"type": "actions", "title": "Settings", "actions": [], "source": null}),
        serde_json::json!({"type": "form", "title": "Setting", "fields": [], "submitLabel": "Apply", "handle": "1"}),
    ] {
        let view: View = serde_json::from_value(value.clone()).unwrap();
        assert!(serde_json::to_value(&view)
            .unwrap()
            .get("description")
            .is_none());
        for (description, valid) in [
            ("Explain the option.\nUnicode: 雪".to_owned(), true),
            ("bad\x1b[31m".to_owned(), false),
            ("x".repeat(MAX_DISPLAY_BYTES), false),
        ] {
            value["description"] = serde_json::json!(description);
            let view: View = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(Reply::View { view }.validate().is_ok(), valid);
        }
    }
}

#[test]
fn table_round_trip_defaults_shapes_and_limits() {
    let value = serde_json::json!({"type":"view", "view":{
        "type":"actions", "title":"Records", "source":null,
        "columns":[{"header":"Number","width":8}, {"header":"Subject","maxLines":3}],
        "actions":[{"id":"1","label":"検索","handle":"original", "cells":["1","A long Unicode subject 雪 ❄"]},
                   {"id":"jump","label":"Jump to number","handle":"jump"}]
    }});
    let reply: Reply = serde_json::from_value(value.clone()).unwrap();
    assert!(reply.validate().is_ok());
    assert_eq!(reply.handles(), ["original", "jump"]);
    assert_eq!(
        serde_json::from_value::<Reply>(serde_json::to_value(&reply).unwrap()).unwrap(),
        reply
    );
    let Reply::View {
        view: View::Actions {
            columns, actions, ..
        },
    } = &reply
    else {
        panic!()
    };
    assert_eq!(columns[0].max_lines, 1);
    assert!(actions[1].cells.is_none());
    assert!(serde_json::to_value(&actions[1])
        .unwrap()
        .get("cells")
        .is_none());
    for bad in [
        serde_json::json!([]),
        serde_json::json!(["1"]),
        serde_json::json!(["1", "bad\u{1b}"]),
    ] {
        let mut invalid = value.clone();
        invalid["view"]["actions"][0]["cells"] = bad;
        assert!(serde_json::from_value::<Reply>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
    for bad in [serde_json::json!([1]), serde_json::json!("cells")] {
        let mut invalid = value.clone();
        invalid["view"]["actions"][0]["cells"] = bad;
        assert!(serde_json::from_value::<Reply>(invalid).is_err());
    }
    for bad in [
        serde_json::json!({"header":"Subject","width":0}),
        serde_json::json!({"header":"Subject","width":513}),
        serde_json::json!({"header":"Subject","maxLines":0}),
        serde_json::json!({"header":"Subject","maxLines":4}),
        serde_json::json!({"header":"bad\nheader"}),
    ] {
        let mut invalid = value.clone();
        invalid["view"]["columns"][1] = bad;
        assert!(serde_json::from_value::<Reply>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
    assert!(validate_table(&[], actions).is_err());
    let flexible = Column {
        header: "Subject".into(),
        width: None,
        max_lines: 3,
    };
    assert!(validate_table(&[flexible.clone(), flexible.clone()], &[]).is_err());
    assert!(validate_table(
        &vec![
            Column {
                width: Some(8),
                ..flexible
            };
            17
        ],
        &[]
    )
    .is_err());
    let mut oversized = value;
    oversized["view"]["actions"][0]["cells"][1] =
        serde_json::json!("雪".repeat(MAX_DISPLAY_BYTES / 3));
    assert!(serde_json::from_value::<Reply>(oversized)
        .unwrap()
        .validate()
        .is_err());
}
