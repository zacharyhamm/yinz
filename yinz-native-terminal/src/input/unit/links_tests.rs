use super::launch::{forget_opened, opened};
use super::*;
use crate::testing::painter;

fn buffer(rows: usize, cols: usize, frames: &[Vec<u8>]) -> ScreenBuffer {
    let mut buffer = ScreenBuffer::new(rows, cols);
    for frame in frames {
        let view = yinz_types::structured_render::decode(frame).expect("a decodable frame");
        buffer.apply(&view).expect("an applicable frame");
    }
    buffer
}

#[test]
fn the_run_under_the_pointer_is_the_contiguous_span_of_one_id() {
    let frame = painter::linked_frame(
        1,
        8,
        0,
        true,
        &[(0, 0, "ab  cdef", &[1, 1, 0, 0, 2, 2, 2, 2])],
        &[
            (1, "https://example.com/one"),
            (2, "https://example.com/two"),
        ],
    );
    let buffer = buffer(1, 8, &[frame]);
    let run = run_at(&buffer, 0, 5).expect("a link under the pointer");
    assert_eq!(run.uri, "https://example.com/two");
    assert_eq!(run.spans, vec![(0, 4, 7)]);
    assert!(run.covers(0, 4));
    assert!(!run.covers(0, 3));
    assert_eq!(
        run_at(&buffer, 0, 1).expect("the first run").spans,
        vec![(0, 0, 1)]
    );
}

#[test]
fn a_cell_outside_any_link_resolves_to_nothing() {
    let frame = painter::linked_frame(
        1,
        8,
        0,
        true,
        &[(0, 0, "ab  cdef", &[1, 1, 0, 0, 0, 0, 0, 0])],
        &[(1, "https://example.com/one")],
    );
    let buffer = buffer(1, 8, &[frame]);
    assert!(run_at(&buffer, 0, 5).is_none());
    assert!(run_at(&buffer, 9, 9).is_none());
}

#[test]
fn a_run_that_wrapped_at_the_row_end_is_followed_onto_the_next_row() {
    let frame = painter::linked_frame(
        2,
        4,
        0,
        true,
        &[(0, 0, "  ab", &[0, 0, 1, 1]), (1, 0, "cd  ", &[1, 1, 0, 0])],
        &[(1, "https://example.com/wrapped")],
    );
    let buffer = buffer(2, 4, &[frame]);
    let run = run_at(&buffer, 0, 3).expect("a link under the pointer");
    assert_eq!(run.spans, vec![(0, 2, 3), (1, 0, 1)]);
    let from_below = run_at(&buffer, 1, 0).expect("the same run from the other row");
    assert_eq!(from_below.spans, run.spans);
}

#[test]
fn a_link_id_that_the_table_does_not_name_resolves_to_nothing() {
    let frame = painter::linked_frame(1, 4, 0, true, &[(0, 0, "abcd", &[7, 7, 0, 0])], &[]);
    let buffer = buffer(1, 4, &[frame]);
    assert!(run_at(&buffer, 0, 0).is_none());
}

#[test]
fn the_allow_list_accepts_the_schemes_a_terminal_link_really_uses() {
    for uri in [
        "http://example.com",
        "https://example.com/a?b=c#d",
        "HTTPS://EXAMPLE.COM",
        "ftp://example.com/x",
        "ftps://example.com/x",
        "mailto:someone@example.com",
        "file:///tmp/notes.txt",
    ] {
        assert!(is_openable(uri), "{} should be openable", uri);
    }
}

#[test]
fn the_allow_list_refuses_everything_else() {
    for uri in [
        "javascript:alert(1)",
        "data:text/html,<script>",
        "vbscript:msgbox",
        "about:blank",
        "chrome://settings",
        "ssh://host",
        "",
        "example.com",
        ":no-scheme",
        "https://example.com/a b",
        "https://example.com/\u{1b}]8;;\u{1b}\\",
        "https://example.com/\n",
    ] {
        assert!(!is_openable(uri), "{:?} should be refused", uri);
    }
}

#[test]
fn a_url_longer_than_the_wire_carries_is_refused() {
    let long = format!("https://example.com/{}", "a".repeat(4096));
    assert!(!is_openable(&long));
}

#[test]
fn opening_a_refused_scheme_never_reaches_the_opener() {
    forget_opened();
    let refused = open("javascript:alert(1)").expect_err("a refusal");
    assert!(refused.contains("refusing to open"));
    assert!(opened().is_empty());
}

#[test]
fn an_allowed_url_reaches_the_platform_opener_as_one_argument() {
    forget_opened();
    open("https://example.com/a?b=c").expect("an allowed url opens");
    assert_eq!(
        opened(),
        vec![(
            opener().expect("this platform has an opener").to_owned(),
            "https://example.com/a?b=c".to_owned()
        )],
        "the url travels as a single argument to the platform's opener, never through a shell"
    );
}

fn run(uri: &str) -> LinkRun {
    LinkRun {
        id: 1,
        uri: uri.to_owned(),
        spans: vec![(0, 0, 3)],
    }
}

const NONE: ModifiersState = ModifiersState::empty();

fn armed(wants: bool, modifiers: ModifiersState, uri: &str) -> Option<Armed> {
    armed_by_press(
        Opening::WITHOUT_SHIFT,
        wants,
        modifiers,
        Some((1, 0)),
        Some(&run(uri)),
    )
}

#[test]
fn a_plain_click_opens_a_link_in_a_pane_that_is_not_watching_the_mouse() {
    let armed = armed(false, NONE, "https://example.com").expect("the press arms the open");
    assert_eq!(armed.cell, (1, 0));
    assert_eq!(
        released_on(Some(armed), Some((1, 0))),
        Some("https://example.com".to_owned())
    );
}

#[test]
fn a_plain_click_is_not_enough_once_the_pane_is_watching_the_mouse() {
    assert_eq!(armed(true, NONE, "https://example.com"), None);
}

#[test]
fn shift_opens_a_link_in_a_pane_that_is_watching_the_mouse() {
    assert!(armed(true, ModifiersState::SHIFT, "https://example.com").is_some());
}

#[test]
fn without_shift_opening_shift_over_a_pane_not_watching_the_mouse_opens_nothing() {
    assert_eq!(
        armed(false, ModifiersState::SHIFT, "https://example.com"),
        None
    );
}

fn armed_with_shift(wants: bool, modifiers: ModifiersState) -> Option<Armed> {
    armed_by_press(
        Opening::WITH_SHIFT,
        wants,
        modifiers,
        Some((1, 0)),
        Some(&run("https://example.com")),
    )
}

#[test]
fn with_shift_opening_shift_opens_a_link_in_every_pane() {
    assert!(armed_with_shift(false, ModifiersState::SHIFT).is_some());
    assert!(armed_with_shift(true, ModifiersState::SHIFT).is_some());
}

#[test]
fn with_shift_opening_a_plain_click_keeps_its_old_meaning() {
    assert!(armed_with_shift(false, NONE).is_some());
    assert_eq!(armed_with_shift(true, NONE), None);
}

#[test]
fn with_shift_opening_no_other_modifier_opens_a_link() {
    for modifiers in [
        ModifiersState::CONTROL,
        ModifiersState::ALT,
        ModifiersState::SUPER,
        ModifiersState::CONTROL | ModifiersState::SHIFT,
        ModifiersState::ALT | ModifiersState::SHIFT,
    ] {
        assert_eq!(armed_with_shift(false, modifiers), None);
        assert_eq!(armed_with_shift(true, modifiers), None);
    }
}

#[test]
fn no_other_modifier_opens_a_link() {
    for modifiers in [
        ModifiersState::CONTROL,
        ModifiersState::ALT,
        ModifiersState::SUPER,
        ModifiersState::CONTROL | ModifiersState::SHIFT,
    ] {
        assert_eq!(armed(false, modifiers, "https://example.com"), None);
        assert_eq!(armed(true, modifiers, "https://example.com"), None);
    }
}

const DELIBERATE: ModifiersState = if cfg!(target_os = "macos") {
    ModifiersState::SUPER
} else {
    ModifiersState::CONTROL
};

#[test]
fn a_plain_click_never_opens_a_file_or_ftp_link() {
    for uri in [
        "file:///tmp/x.exe",
        "FILE://host/share/x.exe",
        "ftp://example.com/x",
        "ftps://example.com/x",
    ] {
        assert_eq!(armed(false, NONE, uri), None, "{}", uri);
        assert_eq!(armed(true, NONE, uri), None, "{}", uri);
        assert_eq!(armed(false, ModifiersState::SHIFT, uri), None, "{}", uri);
        assert_eq!(
            armed_by_press(
                Opening::WITH_SHIFT,
                false,
                ModifiersState::SHIFT,
                Some((1, 0)),
                Some(&run(uri)),
            ),
            None,
            "{}",
            uri
        );
        assert!(!is_reachable(
            Opening::WITH_SHIFT,
            false,
            NONE,
            Some(&run(uri))
        ));
    }
}

#[test]
fn the_platform_modifier_opens_a_file_link_and_marks_the_click_as_its_own() {
    for wants in [false, true] {
        let armed = armed(wants, DELIBERATE, "file:///tmp/notes.txt")
            .expect("the deliberate press arms the open");
        assert!(armed.deliberate);
        assert_eq!(
            released_on(Some(armed), Some((1, 0))),
            Some("file:///tmp/notes.txt".to_owned())
        );
    }
    assert!(armed(false, DELIBERATE | ModifiersState::SHIFT, "file:///tmp/x").is_some());
    assert_eq!(
        armed(false, DELIBERATE | ModifiersState::ALT, "file:///tmp/x"),
        None
    );
    assert!(is_reachable(
        Opening::WITHOUT_SHIFT,
        true,
        DELIBERATE,
        Some(&run("file:///tmp/x"))
    ));
    assert_eq!(
        armed_by_press(
            Opening::OFF,
            false,
            DELIBERATE,
            Some((1, 0)),
            Some(&run("file:///tmp/x"))
        ),
        None
    );
}

#[test]
fn web_links_keep_opening_on_a_plain_click() {
    let armed = armed(false, NONE, "https://example.com").expect("a plain click opens");
    assert!(!armed.deliberate);
    assert!(armed_by_press(
        Opening::WITHOUT_SHIFT,
        false,
        NONE,
        Some((1, 0)),
        Some(&run("mailto:someone@example.com"))
    )
    .is_some());
}

#[test]
fn a_drag_off_the_link_is_a_selection_and_opens_nothing() {
    let armed = armed(false, NONE, "https://example.com").expect("the press arms the open");
    assert_eq!(released_on(Some(armed.clone()), Some((9, 0))), None);
    assert_eq!(released_on(Some(armed), None), None);
    assert_eq!(released_on(None, Some((1, 0))), None);
}

#[test]
fn a_press_beside_the_run_or_on_a_refused_scheme_arms_nothing() {
    assert_eq!(
        armed_by_press(
            Opening::WITHOUT_SHIFT,
            false,
            NONE,
            Some((9, 0)),
            Some(&run("https://example.com"))
        ),
        None,
        "the pointer must be on the run the hover found"
    );
    assert_eq!(armed(false, NONE, "javascript:alert(1)"), None);
    assert_eq!(
        armed_by_press(Opening::WITHOUT_SHIFT, false, NONE, Some((1, 0)), None),
        None
    );
}

#[test]
fn opening_switched_off_arms_nothing() {
    assert_eq!(
        armed_by_press(
            Opening::OFF,
            false,
            NONE,
            Some((1, 0)),
            Some(&run("https://example.com"))
        ),
        None
    );
}

#[test]
fn a_link_is_reachable_only_when_this_click_would_really_open_it() {
    let allowed = run("https://example.com");
    let refused = run("javascript:alert(1)");
    assert!(is_reachable(
        Opening::WITHOUT_SHIFT,
        false,
        NONE,
        Some(&allowed)
    ));
    assert!(
        !is_reachable(Opening::WITHOUT_SHIFT, true, NONE, Some(&allowed)),
        "a pane watching the mouse needs shift, so a bare pointer promises nothing"
    );
    assert!(is_reachable(
        Opening::WITHOUT_SHIFT,
        true,
        ModifiersState::SHIFT,
        Some(&allowed)
    ));
    assert!(is_reachable(
        Opening::WITH_SHIFT,
        false,
        ModifiersState::SHIFT,
        Some(&allowed)
    ));
    assert!(
        !is_reachable(Opening::OFF, true, ModifiersState::SHIFT, Some(&allowed)),
        "with opening switched off shift reaches nothing either"
    );
    assert!(
        !is_reachable(Opening::WITHOUT_SHIFT, false, NONE, Some(&refused)),
        "a scheme the allow-list refuses must not be advertised as openable"
    );
    assert!(!is_reachable(Opening::WITHOUT_SHIFT, false, NONE, None));
    assert!(
        !is_reachable(Opening::OFF, false, NONE, Some(&allowed)),
        "with opening switched off nothing is reachable"
    );
}

#[test]
fn the_opener_is_named_for_this_platform() {
    if cfg!(target_os = "linux") {
        assert_eq!(opener(), Some("xdg-open"));
    } else if cfg!(target_os = "macos") {
        assert_eq!(opener(), Some("open"));
    } else if cfg!(windows) {
        assert_eq!(opener(), Some("ShellExecuteW"));
    } else {
        assert_eq!(opener(), None);
    }
}

fn narrow(wide: &[u16]) -> String {
    assert_eq!(wide.last(), Some(&0), "a wide string must end in NUL");
    String::from_utf16(&wide[..wide.len() - 1]).unwrap()
}

#[test]
fn the_windows_opener_passes_the_link_whole_to_the_open_verb() {
    let uri = "https://example.com/a b?q=\"x\"&y=%20|dir";
    let args = shell_execute_args(uri).unwrap();
    assert_eq!(narrow(&args.verb), "open");
    assert_eq!(narrow(&args.file), uri);
    assert_eq!(args.file.iter().filter(|unit| **unit == 0).count(), 1);
}

#[test]
fn the_windows_opener_keeps_characters_outside_the_basic_plane() {
    let uri = "https://example.com/\u{1f600}/\u{4f60}";
    let args = shell_execute_args(uri).unwrap();
    assert_eq!(narrow(&args.file), uri);
    assert!(args.file.len() > uri.chars().count() + 1);
}

#[test]
fn the_windows_opener_refuses_a_link_with_an_embedded_nul() {
    assert!(shell_execute_args("https://example.com/\0calc.exe").is_err());
}
