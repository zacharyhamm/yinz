#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, split_down_and_wait_for_prompt, start_yinz,
    FakePtyHandle, TestSession,
};

const OSC7_OPENER: &[u8] = b"\x1b]7;";

fn osc7(uri: &str) -> Vec<u8> {
    format!("\x1b]7;{uri}\x1b\\").into_bytes()
}

fn occurrences_of(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn claim_quiet_terminal(yinz: &TestSession) -> FakePtyHandle {
    let terminal = claim_first_terminal_and_wait_for_prompt(yinz);
    terminal.disable_echo();
    terminal
}

fn split_down_to_quiet_terminal(yinz: &TestSession) -> FakePtyHandle {
    let terminal = split_down_and_wait_for_prompt(yinz);
    terminal.disable_echo();
    terminal
}

fn report_cwd_and_wait(yinz: &TestSession, terminal: &FakePtyHandle, uri: &str) {
    let report = osc7(uri);
    terminal.output(&report);
    yinz.wait_until_raw_output(
        &format!("the host terminal was told about {uri}"),
        |raw_bytes| occurrences_of(raw_bytes, &report) > 0,
    );
}

fn echo_and_wait(yinz: &TestSession, terminal: &FakePtyHandle, sentinel: &str) {
    terminal.output(format!("{sentinel}\r\n").as_bytes());
    yinz.wait_until(
        "the pane processed output after the report",
        |grid_snapshot| grid_snapshot.contains(sentinel),
    );
}

fn focus_upper_pane(yinz: &TestSession, terminal: &FakePtyHandle) {
    assert_eq!(
        yinz.run_cli_action(CliAction::FocusPaneId {
            pane_id: format!("terminal_{}", terminal.terminal_id()),
        }),
        0,
        "focus-pane exited cleanly"
    );
    yinz.wait_until("focus moved to the upper pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(3))
    });
}

#[test]
fn a_working_directory_report_reaches_the_host_terminal() {
    let mut yinz = start_yinz();
    let terminal = claim_quiet_terminal(&yinz);

    report_cwd_and_wait(&yinz, &terminal, "file://host/tmp");

    yinz.quit();
}

#[test]
fn an_unchanged_working_directory_is_reported_only_once() {
    let mut yinz = start_yinz();
    let terminal = claim_quiet_terminal(&yinz);

    report_cwd_and_wait(&yinz, &terminal, "file://host/tmp");
    terminal.output(&osc7("file://host/tmp"));
    echo_and_wait(&yinz, &terminal, "second-report-processed");

    assert_eq!(
        occurrences_of(&yinz.raw_bytes(), &osc7("file://host/tmp")),
        1,
        "a pane repeating its working directory does not report it again"
    );

    yinz.quit();
}

#[test]
fn a_changed_working_directory_is_reported_again() {
    let mut yinz = start_yinz();
    let terminal = claim_quiet_terminal(&yinz);

    report_cwd_and_wait(&yinz, &terminal, "file://host/first");
    report_cwd_and_wait(&yinz, &terminal, "file://host/second");

    yinz.quit();
}

#[test]
fn moving_focus_reports_the_working_directory_of_the_newly_focused_pane() {
    let mut yinz = start_yinz();
    let upper_terminal = claim_quiet_terminal(&yinz);
    report_cwd_and_wait(&yinz, &upper_terminal, "file://host/upper");

    let lower_terminal = split_down_to_quiet_terminal(&yinz);
    report_cwd_and_wait(&yinz, &lower_terminal, "file://host/lower");

    focus_upper_pane(&yinz, &upper_terminal);

    let upper_report = osc7("file://host/upper");
    yinz.wait_until_raw_output(
        "the upper pane's working directory was restated on focus",
        |raw_bytes| occurrences_of(raw_bytes, &upper_report) == 2,
    );

    yinz.quit();
}

#[test]
fn a_pane_that_never_reported_a_working_directory_reports_nothing_on_focus() {
    let mut yinz = start_yinz();
    let upper_terminal = claim_quiet_terminal(&yinz);
    report_cwd_and_wait(&yinz, &upper_terminal, "file://host/upper");

    // A split may inherit a launch directory. Only an OSC 7 report is forwarded.
    let lower_terminal = split_down_to_quiet_terminal(&yinz);
    echo_and_wait(&yinz, &lower_terminal, "lower-pane-is-focused");

    assert_eq!(
        occurrences_of(&yinz.raw_bytes(), OSC7_OPENER),
        1,
        "focusing a pane with nothing to report leaves the host terminal as it was"
    );

    focus_upper_pane(&yinz, &upper_terminal);
    let upper_report = osc7("file://host/upper");
    yinz.wait_until_raw_output(
        "the upper pane's working directory was restated on focus",
        |raw_bytes| occurrences_of(raw_bytes, &upper_report) == 2,
    );

    yinz.quit();
}
