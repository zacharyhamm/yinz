use super::*;

#[test]
fn background_thread_panics_fail_session_shutdown() {
    const PROBE_ENV: &str = "YINZ_TEST_BACKGROUND_PANIC_PROBE";
    if std::env::var_os(PROBE_ENV).is_some() {
        let mut session = TestRunner::new(crate::TERMINAL_SIZE)
            .with_config("load_plugins clear-defaults=true {}")
            .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
            .start();
        session.expect_pty_spawn().output(b"ready");
        session.wait_until("probe session loaded", |grid| grid.contains("ready"));
        // Server teardown discards worker join errors. This must still fail the test
        // even when its visible behavior and outer server shutdown both succeed.
        let _ = std::thread::Builder::new()
            .name("fault_injected_worker".into())
            .spawn(|| panic!("injected worker invariant failure"))
            .unwrap()
            .join();
        session.quit();
        return;
    }

    // Isolate the intentional panic from this process's panic recorder, including when
    // bare cargo test runs other tests alongside this one.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runner::tests::background_thread_panics_fail_session_shutdown",
            "--nocapture",
        ])
        .env(PROBE_ENV, "1")
        .output()
        .unwrap();
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "worker panic passed:\n{diagnostic}"
    );
    assert!(diagnostic.contains("fault_injected_worker"), "{diagnostic}");
    assert!(
        diagnostic.contains("injected worker invariant failure"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("background thread panicked during integration test"),
        "{diagnostic}"
    );
}

/// A compact bar is drawn on the terminal's last row, which has no newline after it. An
/// end-anchored strip has to reach it there too, or the indicator only ever survives into
/// the snapshots of the one layout that puts the bar last.
#[test]
fn the_swap_layout_indicator_is_stripped_on_the_last_row_too() {
    let bar = " Yinz (test) NORMAL  [0] Tab #1";
    let indicator = format!("{bar}{}BASE ", " ".repeat(20));
    assert_eq!(
        strip_trailing_whitespace(&strip_swap_layout_indication(&format!("$ \n{indicator}\n"))),
        format!("$\n{bar}\n"),
    );
    assert_eq!(
        strip_trailing_whitespace(&strip_swap_layout_indication(&indicator)),
        bar,
        "the last row carries no newline to anchor against",
    );
}

/// Rows arrive here padded to the terminal width, so a clock shape in the middle of a
/// pane's line is followed by the rest of that row. Only the bar's own clock is flush
/// against the right edge, and only that one is the wall clock a snapshot cannot pin.
#[test]
fn only_a_flush_right_clock_is_stripped() {
    let width = 40;
    let pane_line = format!("{:width$}", "$ make -j 12:34");
    let bar_line = format!("{:>width$}", "<q> QUIT      14:32 ");
    let grid = format!("{pane_line}\n{bar_line}\n");

    let stripped = strip_status_bar_clock(&grid);
    assert!(
        stripped.contains("$ make -j 12:34"),
        "a pane's own line was rewritten: {stripped:?}"
    );
    assert!(
        !stripped.contains("14:32"),
        "the bar's clock survived: {stripped:?}"
    );
}

/// A nested session draws a second bar inside a pane, so its clock ends against the pane
/// border and the host's own row runs on past it. The border and what follows are put back.
#[test]
fn a_nested_bars_clock_is_stripped_and_the_border_kept() {
    let stripped = strip_status_bar_clock(" <q> QUIT      14:32 │        \n");
    assert!(!stripped.contains("14:32"), "{stripped:?}");
    assert!(stripped.contains('│'), "{stripped:?}");
}

/// The date half is what a narrow terminal sheds first, so both shapes have to go.
#[test]
fn the_date_half_goes_with_the_time() {
    let stripped = strip_status_bar_clock(" <q> QUIT   14:32  Fri 5 Sep \n");
    assert_eq!(stripped, " <q> QUIT    \n");
}

/// A longer number that happens to end in a clock shape - a byte count, a duration - is
/// not a clock.
#[test]
fn a_longer_number_ending_in_a_clock_shape_is_left_alone() {
    let line = " downloaded 112:34 \n";
    assert_eq!(strip_status_bar_clock(line), line);
}
