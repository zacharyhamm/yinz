use super::*;

#[test]
fn empty_param_parses_as_empty_text() {
    assert_eq!(parse_text("").unwrap(), "");
}

#[test]
fn empty_param_survives_parse_text_params() {
    let mut params = [String::new()];
    let parsed = parse_text_params(params.iter_mut());
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].text, "");
}

#[test]
fn flags_only_param_parses_as_empty_text() {
    // "d" is the disabled flag with no text after it - eg. a plugin
    // rendering a blank line with styling flags attached
    let mut params = ["d".to_owned()];
    let parsed = parse_text_params(params.iter_mut());
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].text, "");
    assert!(parsed[0].disabled);
}

#[test]
fn missing_param_still_yields_nothing() {
    let mut params: Vec<String> = vec![];
    assert!(parse_text_params(params.iter_mut()).is_empty());
}

/// Level 8 crosses a character out and leaves the levels below it alone: a struck run
/// keeps whatever colour it was given, and the characters around it are not struck.
#[test]
fn the_strike_level_crosses_out_only_the_indices_it_names() {
    // Eight empty index groups, then level 8 naming characters 1 and 2, then the text
    // as the comma-joined bytes the plugin serializes it to.
    let mut params = ["$$$$$$$$1,2$97,98,99,100".to_owned()];
    let parsed = parse_text_params(params.iter_mut());
    assert_eq!(parsed.len(), 1);
    let text = &parsed[0];
    assert_eq!(text.text, "abcd");
    assert!(!text.is_struck_at(0));
    assert!(text.is_struck_at(1));
    assert!(text.is_struck_at(2));
    assert!(!text.is_struck_at(3));
    // and it is not read as one of the levels below it
    assert!(!text.is_dimmed_at(1));
    assert!(!text.is_unbold_at(1));
}
