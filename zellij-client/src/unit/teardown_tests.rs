use crate::terminal_escapes::{
    ClientTeardown, DISABLE_FOCUS_REPORTING, ENABLE_FOCUS_REPORTING, EXIT_ALTERNATE_SCREEN,
};

#[test]
fn the_teardown_message_cancels_focus_reporting() {
    let message = ClientTeardown::new(20, false).message("bye");
    assert!(
        message.contains(DISABLE_FOCUS_REPORTING),
        "the host must stop emitting focus reports on teardown, got: {:?}",
        message
    );
}

#[test]
fn the_startup_teardown_message_cancels_focus_reporting_too() {
    let message = ClientTeardown::at_startup(true).message("bye");
    assert!(
        message.contains(DISABLE_FOCUS_REPORTING),
        "a failure during startup must not leave the host reporting focus, got: {:?}",
        message
    );
}

#[test]
fn the_teardown_message_cancels_focus_reporting_before_leaving_the_alternate_screen() {
    let message = ClientTeardown::new(20, true).message("bye");
    let disable_focus = message.find(DISABLE_FOCUS_REPORTING).unwrap();
    let exit_alternate_screen = message.find(EXIT_ALTERNATE_SCREEN).unwrap();
    assert!(
        disable_focus < exit_alternate_screen,
        "focus reporting is cancelled while we still own the screen, got: {:?}",
        message
    );
}

#[test]
fn the_focus_reporting_sequences_are_the_decset_1004_pair() {
    assert_eq!(ENABLE_FOCUS_REPORTING, "\u{1b}[?1004h");
    assert_eq!(DISABLE_FOCUS_REPORTING, "\u{1b}[?1004l");
}
