use crate::session_picker::*;
use crate::sessions::{merge_listings, SessionListing};
use std::time::Duration;
use yinz_ipc::probe::SessionProbe;
use yinz_types::internal_ids::SessionName;

fn row(name: &str, listing: SessionListing) -> PickerRow {
    PickerRow {
        label: name.to_owned(),
        destination: Some(Destination::Local(SessionName::new(name))),
        status: None,
        age: Duration::from_secs(60),
        listing,
        current: false,
    }
}

fn running(names: &[&str]) -> Vec<PickerRow> {
    names
        .iter()
        .map(|n| row(n, SessionListing::Running))
        .collect()
}

fn type_str(state: &mut PickerState, text: &str) {
    for c in text.chars() {
        assert_eq!(state.handle(PickerKey::Char(c)), Outcome::Continue);
    }
}

fn selected(state: &PickerState) -> &str {
    state.selected().unwrap().label.as_str()
}

fn strip(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for e in chars.by_ref() {
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn typing_filters_by_substring_ignoring_case() {
    let mut state = PickerState::new(running(&["main", "Magic-fox", "quiet-owl"]), "attach");
    type_str(&mut state, "MA");
    assert_eq!(state.visible_names(), ["main", "Magic-fox"]);
    type_str(&mut state, "g");
    assert_eq!(state.visible_names(), ["Magic-fox"]);
    state.handle(PickerKey::Backspace);
    state.handle(PickerKey::Backspace);
    state.handle(PickerKey::Backspace);
    assert_eq!(state.visible_names().len(), 3);
}

#[test]
fn tab_extends_to_the_shared_prefix_then_cycles() {
    let mut state = PickerState::new(running(&["project-api", "project-web", "other"]), "attach");
    type_str(&mut state, "p");
    state.handle(PickerKey::Tab);
    assert_eq!(state.input(), "project-");
    assert_eq!(selected(&state), "project-api");
    state.handle(PickerKey::Tab);
    assert_eq!(state.input(), "project-");
    assert_eq!(selected(&state), "project-web");
    state.handle(PickerKey::Tab);
    assert_eq!(selected(&state), "project-api", "cycling wraps around");
    state.handle(PickerKey::BackTab);
    assert_eq!(selected(&state), "project-web");
}

#[test]
fn tab_with_a_single_candidate_fills_its_whole_name() {
    let mut state = PickerState::new(running(&["main", "bar-baz"]), "attach");
    type_str(&mut state, "b");
    state.handle(PickerKey::Tab);
    assert_eq!(state.input(), "bar-baz");
    assert_eq!(selected(&state), "bar-baz");
    assert_eq!(
        state.handle(PickerKey::Enter),
        Outcome::Picked(Destination::Local(SessionName::new("bar-baz")))
    );
}

#[test]
fn tab_on_a_mid_name_match_cycles_the_matches() {
    let mut state = PickerState::new(running(&["alpha-x", "beta-x", "gamma"]), "attach");
    type_str(&mut state, "-x");
    assert_eq!(selected(&state), "alpha-x");
    state.handle(PickerKey::Tab);
    assert_eq!(state.input(), "-x", "no shared prefix extends the input");
    assert_eq!(selected(&state), "beta-x");
}

#[test]
fn the_cursor_survives_a_filter_that_empties_the_list() {
    let mut state = PickerState::new(running(&["one", "two"]), "attach");
    type_str(&mut state, "zzz");
    assert!(state.selected().is_none());
    assert_eq!(state.handle(PickerKey::Enter), Outcome::Continue);
    state.handle(PickerKey::Down);
    state.handle(PickerKey::End);
    assert!(state.selected().is_none());
    for _ in 0..3 {
        state.handle(PickerKey::Backspace);
    }
    assert_eq!(selected(&state), "one");
}

#[test]
fn arrows_move_within_bounds() {
    let mut state = PickerState::new(running(&["a", "b", "c"]), "attach");
    state.handle(PickerKey::Up);
    assert_eq!(selected(&state), "a");
    state.handle(PickerKey::Down);
    state.handle(PickerKey::Down);
    state.handle(PickerKey::Down);
    assert_eq!(selected(&state), "c");
    state.handle(PickerKey::Home);
    assert_eq!(selected(&state), "a");
    state.handle(PickerKey::PageDown);
    assert_eq!(selected(&state), "c");
}

#[test]
fn the_initial_cursor_skips_the_current_and_unresponsive_sessions() {
    let mut rows = vec![
        row("here", SessionListing::Running),
        row("wedged", SessionListing::Unresponsive),
        row("there", SessionListing::Running),
    ];
    rows[0].current = true;
    let state = PickerState::new(rows, "switch");
    assert_eq!(selected(&state), "there");
}

#[test]
fn enter_on_an_unresponsive_session_explains_and_stays_open() {
    let mut state = PickerState::new(vec![row("wedged", SessionListing::Unresponsive)], "attach");
    assert_eq!(state.handle(PickerKey::Enter), Outcome::Continue);
    let footer = strip(state.render(200, 10).last().unwrap());
    assert!(footer.contains("yinz kill-session wedged"), "{footer}");
    state.handle(PickerKey::Down);
    let footer = strip(state.render(200, 10).last().unwrap());
    assert!(
        footer.contains("Tab complete"),
        "any key clears the message"
    );
}

#[test]
fn a_session_this_process_may_not_reach_is_listed_but_not_offered() {
    let listing = crate::sessions::format_session_line(
        "sandboxed",
        Duration::from_secs(60),
        SessionListing::Denied,
        false,
        false,
    );
    assert!(listing.contains("UNREACHABLE"), "{listing}");
    assert!(!listing.contains("kill-session"), "{listing}");
    let rows = vec![
        row("sandboxed", SessionListing::Denied),
        row("there", SessionListing::Running),
    ];
    let mut state = PickerState::new(rows, "attach");
    assert_eq!(selected(&state), "there");
    state.handle(PickerKey::Up);
    assert_eq!(selected(&state), "sandboxed");
    assert_eq!(state.handle(PickerKey::Enter), Outcome::Continue);
    let footer = strip(state.render(200, 10).last().unwrap());
    assert!(footer.contains("cannot be reached"), "{footer}");
}

#[test]
fn enter_on_the_current_session_leaves_saying_so() {
    let mut rows = running(&["here"]);
    rows[0].current = true;
    let mut state = PickerState::new(rows, "switch");
    match state.handle(PickerKey::Enter) {
        Outcome::Cancelled(Some(message)) => assert!(message.contains("here")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn exited_sessions_can_be_picked() {
    let mut state = PickerState::new(vec![row("old", SessionListing::Exited)], "attach");
    assert_eq!(
        state.handle(PickerKey::Enter),
        Outcome::Picked(Destination::Local(SessionName::new("old")))
    );
}

#[test]
fn escape_and_interrupt_leave() {
    let mut state = PickerState::new(running(&["a"]), "attach");
    assert_eq!(state.handle(PickerKey::Escape), Outcome::Cancelled(None));
    assert_eq!(state.handle(PickerKey::Interrupt), Outcome::Interrupted);
}

#[test]
fn render_fits_the_width_and_scrolls_to_the_cursor() {
    let names: Vec<String> = (0..20).map(|i| format!("session-{i:02}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut state = PickerState::new(running(&names), "attach");
    for _ in 0..15 {
        state.handle(PickerKey::Down);
    }
    let lines = state.render(30, 8);
    assert_eq!(lines.len(), 8, "prompt, six rows, footer");
    let plain: Vec<String> = lines.iter().map(|l| strip(l)).collect();
    assert!(plain.iter().all(|l| l.chars().count() <= 30), "{plain:?}");
    assert!(plain[0].starts_with("Session: "));
    assert!(
        plain.iter().any(|l| l.starts_with("> session-15")),
        "{plain:?}"
    );
    assert!(plain[7].starts_with("(20 of 20"), "{plain:?}");
}

#[test]
fn render_says_when_nothing_matches() {
    let mut state = PickerState::new(running(&["a"]), "attach");
    type_str(&mut state, "zzz");
    let plain: Vec<String> = state.render(80, 10).iter().map(|l| strip(l)).collect();
    assert_eq!(plain[1], "  (no matching sessions)");
    assert!(plain[2].starts_with("(0 of 1"));
}

#[test]
fn truncation_skips_escapes_and_resets_after_a_cut() {
    assert_eq!(
        truncate_to_width("\u{1b}[1mabc\u{1b}[m", 10),
        "\u{1b}[1mabc\u{1b}[m"
    );
    assert_eq!(
        truncate_to_width("\u{1b}[1mabcdef", 3),
        "\u{1b}[1mabc\u{1b}[m"
    );
    assert_eq!(truncate_to_width("日本語", 4), "日本\u{1b}[m");
}

#[test]
fn render_fits_a_terminal_too_short_for_the_footer() {
    let mut state = PickerState::new(running(&["a", "b", "c"]), "attach");
    state.handle(PickerKey::Down);
    let two: Vec<String> = state.render(80, 2).iter().map(|l| strip(l)).collect();
    assert_eq!(two.len(), 2, "{two:?}");
    assert!(two[0].starts_with("Session: "), "{two:?}");
    assert!(two[1].starts_with("> b"), "the footer goes first: {two:?}");
    let one = state.render(80, 1);
    assert_eq!(one.len(), 1);
    assert!(strip(&one[0]).starts_with("Session: "));
    assert_eq!(state.render(80, 0).len(), 1, "the input line always shows");
}

/// Parses `bytes` as one read followed by silence.
fn parse(bytes: &[u8]) -> Vec<PickerKey> {
    let mut parser = KeyParser::default();
    let mut keys = parser.feed(bytes);
    keys.extend(parser.flush());
    keys
}

#[test]
fn keys_parse_from_terminal_bytes() {
    use PickerKey::*;
    assert_eq!(parse(b"ab\t\r"), [Char('a'), Char('b'), Tab, Enter]);
    assert_eq!(parse(b"\x1b[A\x1b[B\x1bOH\x1b[F"), [Up, Down, Home, End]);
    assert_eq!(parse(b"\x1b[5~\x1b[6~\x1b[Z"), [PageUp, PageDown, BackTab]);
    assert_eq!(parse(b"\x1b"), [Escape], "a lone ESC is the key");
    assert_eq!(parse(b"\x1b\x1b[A"), [Escape, Up]);
    assert_eq!(parse(b"\x03"), [Interrupt]);
    assert_eq!(parse(b"\x7f\x10\x0e"), [Backspace, Up, Down]);
    assert_eq!(parse("é日".as_bytes()), [Char('é'), Char('日')]);
    assert_eq!(
        parse(b"\x1b[3~x"),
        [Char('x')],
        "unknown sequences are dropped"
    );
    assert_eq!(parse(b"\x01"), [], "other control bytes are ignored");
    assert_eq!(
        parse(b"\xc3a"),
        [Char('a')],
        "a stray lead byte is dropped alone"
    );
}

#[test]
fn a_key_split_across_reads_is_still_one_key() {
    use PickerKey::*;
    let mut parser = KeyParser::default();
    assert_eq!(parser.feed(b"\x1b"), []);
    assert!(parser.has_pending());
    assert_eq!(
        parser.feed(b"[A"),
        [Up],
        "an arrow's ESC is not the Escape key"
    );
    assert!(!parser.has_pending());
    assert_eq!(parser.feed(b"x\x1b[5"), [Char('x')]);
    assert_eq!(parser.feed(b"~"), [PageUp]);
    let e = "é".as_bytes();
    assert_eq!(parser.feed(&e[..1]), []);
    assert_eq!(parser.feed(&e[1..]), [Char('é')]);
}

#[test]
fn silence_after_a_part_read_key_settles_it() {
    let mut parser = KeyParser::default();
    assert_eq!(parser.feed(b"\x1b"), []);
    assert_eq!(parser.flush(), [PickerKey::Escape]);
    assert!(!parser.has_pending());
    assert_eq!(parser.flush(), [], "flushing twice finds nothing");
    assert_eq!(parser.feed(b"\x1b[1"), []);
    assert_eq!(parser.flush(), [], "an unfinished sequence is dropped");
    assert_eq!(parser.feed(b"a"), [PickerKey::Char('a')]);
}

#[test]
fn a_live_socket_overrides_a_resurrectable_session_of_the_same_name() {
    let minute = Duration::from_secs(60);
    let mut merged = merge_listings(
        vec![
            (SessionName::new("both"), minute, SessionProbe::Alive),
            (
                SessionName::new("wedged"),
                minute,
                SessionProbe::Unresponsive,
            ),
            (SessionName::new("gone"), minute, SessionProbe::Absent),
            (SessionName::new("sandboxed"), minute, SessionProbe::Denied),
        ],
        vec![
            (SessionName::new("both"), minute),
            (SessionName::new("old"), minute),
        ],
    );
    merged.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let listed: Vec<(&str, SessionListing)> =
        merged.iter().map(|(n, _, l)| (n.as_str(), *l)).collect();
    assert_eq!(
        listed,
        [
            ("both", SessionListing::Running),
            ("old", SessionListing::Exited),
            ("sandboxed", SessionListing::Denied),
            ("wedged", SessionListing::Unresponsive),
        ]
    );
}

fn remote_row(host: &str, name: &str) -> PickerRow {
    let mut row = row(&format!("{host} / {name}"), SessionListing::Running);
    row.destination = Some(Destination::Remote {
        host: host.into(),
        name: name.into(),
    });
    row
}

#[test]
fn progressive_remote_results_preserve_destination_and_filter() {
    let first = remote_row("one", "work");
    let second = remote_row("two", "work");
    let mut state = PickerState::new(vec![first.clone(), second.clone()], "attach");
    state.handle(PickerKey::Down);
    let mut rows = running(&["work"]);
    rows.extend([remote_row("new", "work"), first, second.clone()]);
    state.replace_rows(rows);
    assert_eq!(
        state.handle(PickerKey::Enter),
        Outcome::Picked(second.destination.clone().unwrap())
    );
    type_str(&mut state, "two / wo");
    assert_eq!(state.visible_names(), ["two / work"]);
    state.replace_rows(vec![remote_row("one", "work"), second.clone()]);
    assert_eq!(
        state.handle(PickerKey::Enter),
        Outcome::Picked(second.destination.unwrap())
    );
}

#[test]
fn host_status_and_unavailable_remote_sessions_cannot_be_picked() {
    let mut loading = row("host", SessionListing::Denied);
    loading.destination = None;
    loading.status = Some("loading…".into());
    let mut state = PickerState::new(vec![loading], "attach");
    assert_eq!(state.handle(PickerKey::Enter), Outcome::Continue);
    assert!(state
        .render(80, 24)
        .iter()
        .any(|line| line.contains("loading…")));
    let mut unavailable = remote_row("host", "work");
    unavailable.listing = SessionListing::Unresponsive;
    state.replace_rows(vec![unavailable]);
    assert_eq!(state.handle(PickerKey::Enter), Outcome::Continue);
    assert!(!state
        .render(120, 24)
        .iter()
        .any(|line| line.contains("kill-session")));
    state.replace_rows(vec![remote_row("host", "work")]);
    assert!(matches!(
        state.handle(PickerKey::Enter),
        Outcome::Picked(Destination::Remote { .. })
    ));
}

#[test]
fn first_remote_result_replaces_an_initially_disabled_selection() {
    let mut local = row("local-work", SessionListing::Running);
    local.current = true;
    let mut state = PickerState::new(vec![local.clone()], "switch");
    let remote = remote_row("host", "work");
    state.replace_rows(vec![local, remote.clone()]);
    type_str(&mut state, "work");
    assert_eq!(
        state.handle(PickerKey::Enter),
        Outcome::Picked(remote.destination.unwrap())
    );
}
