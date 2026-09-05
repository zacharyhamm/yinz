use crate::terminal_escapes::{
    ClientTeardown, DISABLE_FOCUS_REPORTING, ENABLE_FOCUS_REPORTING, ENTER_KITTY_KEYBOARD_MODE,
    ENTER_KITTY_KEYBOARD_MODE_WITH_MODIFIERS, EXIT_ALTERNATE_SCREEN, EXIT_KITTY_KEYBOARD_MODE,
};

#[test]
fn the_restore_sequences_unwind_everything_the_client_pushed() {
    let sequences = ClientTeardown::at_startup(true).restore_sequences();
    for expected in [
        EXIT_KITTY_KEYBOARD_MODE,
        DISABLE_FOCUS_REPORTING,
        EXIT_ALTERNATE_SCREEN,
    ] {
        assert!(
            sequences.contains(expected),
            "a panic during startup leaves the user with whatever this does not undo, missing \
             {:?}, got: {:?}",
            expected,
            sequences
        );
    }
}

#[test]
fn the_teardown_message_is_the_restore_sequences_plus_the_message() {
    let teardown = ClientTeardown::new(20, true);
    let message = teardown.message("bye");
    assert!(
        message.starts_with(&teardown.restore_sequences()),
        "the ribbon is defined once and the message follows it, got: {:?}",
        message
    );
    assert!(message.ends_with("bye\n"), "got: {:?}", message);
}

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

/// The flag set `report_modifier_keys` pushes, spelled out. Flag 8 is what the option is for -
/// nothing else makes a terminal report a modifier key on its own - and flag 4 is what pays for
/// it: without an alternate key the shifted codepoint is unrecoverable, and `!` reaches a pane
/// as `\x1b[49;2u`. Dropping either one breaks the other's half of the feature.
#[test]
fn the_modifier_reporting_flags_include_the_alternate_keys_that_make_flag_8_survivable() {
    const DISAMBIGUATE_ESCAPE_CODES: u8 = 1;
    const REPORT_EVENT_TYPES: u8 = 2;
    const REPORT_ALTERNATE_KEYS: u8 = 4;
    const REPORT_ALL_KEYS_AS_ESCAPE_CODES: u8 = 8;

    let flags = ENTER_KITTY_KEYBOARD_MODE_WITH_MODIFIERS
        .strip_prefix("\u{1b}[>")
        .and_then(|rest| rest.strip_suffix('u'))
        .and_then(|digits| digits.parse::<u8>().ok())
        .expect("not a kitty flag push");
    assert_eq!(
        flags,
        DISAMBIGUATE_ESCAPE_CODES
            | REPORT_EVENT_TYPES
            | REPORT_ALTERNATE_KEYS
            | REPORT_ALL_KEYS_AS_ESCAPE_CODES
    );
    // The default push is flag 1 alone, and the two share one pop.
    assert_eq!(ENTER_KITTY_KEYBOARD_MODE, "\u{1b}[>1u");
    assert_eq!(EXIT_KITTY_KEYBOARD_MODE, "\u{1b}[<1u");
}

#[test]
fn the_focus_reporting_sequences_are_the_decset_1004_pair() {
    assert_eq!(ENABLE_FOCUS_REPORTING, "\u{1b}[?1004h");
    assert_eq!(DISABLE_FOCUS_REPORTING, "\u{1b}[?1004l");
}

mod claiming_the_terminal {
    use crate::os_input_output::{StdioAccess, TtyControl};
    use crate::terminal_escapes::{claim_terminal, QUERY_HOST_THEME};
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use zellij_utils::data::Palette;
    use zellij_utils::pane_size::Size;

    /// What the client did to the terminal, in the order it did it.
    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        RawModeOff,
        RawModeOn,
        RawModeOnKeepingSignals,
        Wrote(String),
    }

    #[derive(Clone, Default)]
    struct RecordingTerminal(Arc<Mutex<Vec<Event>>>);

    impl RecordingTerminal {
        fn events(&self) -> std::sync::MutexGuard<'_, Vec<Event>> {
            self.0.lock().unwrap()
        }
    }

    struct RecordingStdout(Arc<Mutex<Vec<Event>>>);

    impl Write for RecordingStdout {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap()
                .push(Event::Wrote(String::from_utf8_lossy(buf).into_owned()));
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl TtyControl for RecordingTerminal {
        fn get_terminal_size(&self) -> Size {
            Size { rows: 24, cols: 80 }
        }
        fn set_raw_mode(&mut self) {
            self.0.lock().unwrap().push(Event::RawModeOn);
        }
        fn set_raw_mode_keeping_signals(&mut self) {
            self.0.lock().unwrap().push(Event::RawModeOnKeepingSignals);
        }
        fn unset_raw_mode(&self) -> Result<(), io::Error> {
            self.0.lock().unwrap().push(Event::RawModeOff);
            Ok(())
        }
        fn load_palette(&self) -> Palette {
            Palette::default()
        }
        fn enable_mouse(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn disable_mouse(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl StdioAccess for RecordingTerminal {
        fn get_stdout_writer(&self) -> Box<dyn Write> {
            Box::new(RecordingStdout(self.0.clone()))
        }
        fn get_stdin_reader(&self) -> Box<dyn io::BufRead> {
            Box::new(io::Cursor::new(Vec::new()))
        }
        fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn raw_mode_is_on_before_anything_is_written() {
        let mut terminal = RecordingTerminal::default();
        claim_terminal(&mut terminal, true, false, false);
        let events = terminal.events();
        let first_write = events
            .iter()
            .position(|event| matches!(event, Event::Wrote(_)))
            .expect("the escape ribbon should have been written");
        let raw_mode_on = events
            .iter()
            .position(|event| *event == Event::RawModeOnKeepingSignals)
            .expect("raw mode should have been enabled");
        assert!(
            raw_mode_on < first_write,
            "the host's reply to a query we write while it can still echo lands on screen, so raw \
             mode has to come first, got: {:?}",
            *events
        );
    }

    #[test]
    fn the_host_is_never_asked_anything_while_it_can_echo() {
        let mut terminal = RecordingTerminal::default();
        claim_terminal(&mut terminal, true, false, false);
        let events = terminal.events();
        let raw_mode_on = events
            .iter()
            .position(|event| *event == Event::RawModeOnKeepingSignals)
            .unwrap();
        let theme_query = events
            .iter()
            .position(
                |event| matches!(event, Event::Wrote(written) if written.contains(QUERY_HOST_THEME)),
            )
            .expect("the theme query should have been written");
        assert!(raw_mode_on < theme_query, "got: {:?}", *events);
    }

    #[test]
    fn the_signal_keys_keep_working_while_the_client_connects() {
        let mut terminal = RecordingTerminal::default();
        claim_terminal(&mut terminal, true, false, false);
        let events = terminal.events();
        assert!(
            events.contains(&Event::RawModeOnKeepingSignals) && !events.contains(&Event::RawModeOn),
            "the client spends this stretch forking a server and waiting on a socket with nothing \
             reading stdin, so a Ctrl-C that is not a signal is a Ctrl-C that does nothing, got: \
             {:?}",
            *events
        );
    }

    #[test]
    fn a_reconnect_still_takes_raw_mode_even_though_it_writes_nothing() {
        let mut terminal = RecordingTerminal::default();
        claim_terminal(&mut terminal, true, false, true);
        let events = terminal.events();
        assert!(
            events.contains(&Event::RawModeOnKeepingSignals),
            "the previous pass left focus reporting on, and those reports echo just like a query \
             reply, got: {:?}",
            *events
        );
        assert!(
            !events.iter().any(|event| matches!(event, Event::Wrote(_))),
            "a reconnect keeps the attributes it already has, got: {:?}",
            *events
        );
    }
}
