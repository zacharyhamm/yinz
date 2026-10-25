use super::*;
#[test]
fn geometry_only() {
    assert!(parse("layout { tab { pane; pane; }; }").is_ok());
    assert!(parse("layout { pane split_direction=\"vertical\" { pane size=\"40%\"; pane stacked=true { pane; pane expanded=true; }; }; }").is_ok());
    for forbidden in [
        "pane command=\"sh\"",
        "pane cwd=\"/tmp\"",
        "pane { plugin location=\"x\"; }",
        "floating_panes { pane; }",
        "tab { pane; }; tab { pane; }",
        "pane { args \"x\"; }",
        "pane { contents \"x\"; }",
    ] {
        assert!(
            parse(&format!("layout {{ {forbidden}; }}")).is_err(),
            "{forbidden}"
        );
    }
    assert!(parse("layout {}; layout {};").is_err());
}
