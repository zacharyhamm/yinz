use super::*;

#[test]
fn send_input_schema_requires_a_pane_and_advertises_the_closed_key_set() {
    let specs = specs();
    let send_input = specs
        .as_array()
        .unwrap()
        .iter()
        .find(|spec| spec["function"]["name"] == "send_input")
        .unwrap();
    assert_eq!(
        send_input["function"]["parameters"]["required"],
        json!(["pane_id", "is_plugin"])
    );
    assert_eq!(
        send_input["function"]["parameters"]["properties"]["keys"]["items"]["enum"],
        json!([
            "enter",
            "tab",
            "escape",
            "backspace",
            "delete",
            "insert",
            "up",
            "down",
            "left",
            "right",
            "home",
            "end",
            "page_up",
            "page_down",
            "ctrl_a",
            "ctrl_b",
            "ctrl_c",
            "ctrl_d",
            "ctrl_e",
            "ctrl_f",
            "ctrl_k",
            "ctrl_l",
            "ctrl_u",
            "ctrl_w",
            "ctrl_y",
            "ctrl_z"
        ])
    );
}

#[test]
fn send_input_encodes_text_before_keys() {
    let args = Args::parse(r#"{"text":"hé","keys":["enter"]}"#).unwrap();
    assert_eq!(
        args.input_chunks(false).unwrap(),
        vec![b"h\xc3\xa9".to_vec(), b"\r".to_vec()]
    );
}

#[test]
fn send_input_brackets_only_the_text_and_only_when_asked() {
    let args = Args::parse(r#"{"text":"hé","keys":["enter"]}"#).unwrap();
    assert_eq!(
        args.input_chunks(true).unwrap(),
        vec![
            b"\x1b[200~".to_vec(),
            b"h\xc3\xa9".to_vec(),
            b"\x1b[201~".to_vec(),
            b"\r".to_vec(),
        ]
    );
    // No text, no markers: a lone key must stay a lone key so Yinz's own
    // key adjustment still matches the whole buffer.
    let keys_only = Args::parse(r#"{"keys":["up"]}"#).unwrap();
    assert_eq!(
        keys_only.input_chunks(true).unwrap(),
        vec![b"\x1b[A".to_vec()]
    );
}

#[test]
fn send_input_encodes_every_named_key() {
    let args = Args::parse(
        r#"{"keys":["enter","tab","escape","backspace","delete","insert","up","down","left","right","home","end","page_up","page_down","ctrl_a","ctrl_b","ctrl_c","ctrl_d","ctrl_e","ctrl_f","ctrl_k","ctrl_l","ctrl_u","ctrl_w","ctrl_y","ctrl_z"]}"#,
    )
    .unwrap();
    assert_eq!(
        args.input_chunks(false).unwrap(),
        vec![
            vec![13],
            vec![9],
            vec![27],
            vec![127],
            vec![27, 91, 51, 126],
            vec![27, 91, 50, 126],
            vec![27, 91, 65],
            vec![27, 91, 66],
            vec![27, 91, 68],
            vec![27, 91, 67],
            vec![27, 91, 72],
            vec![27, 91, 70],
            vec![27, 91, 53, 126],
            vec![27, 91, 54, 126],
            vec![1],
            vec![2],
            vec![3],
            vec![4],
            vec![5],
            vec![6],
            vec![11],
            vec![12],
            vec![21],
            vec![23],
            vec![25],
            vec![26],
        ]
    );
}

#[test]
fn send_input_rejects_empty_and_unknown_input() {
    assert_eq!(
        Args::parse("{}").unwrap().input_chunks(true).unwrap_err(),
        "send_input needs non-empty text or at least one key"
    );
    let unknown = Args::parse(r#"{"keys":["f13"]}"#).unwrap();
    assert_eq!(
        unknown.input_chunks(true).unwrap_err(),
        "unsupported key: f13"
    );
}
