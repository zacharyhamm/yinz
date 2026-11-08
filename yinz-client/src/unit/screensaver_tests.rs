use super::*;
use crate::os_input_output::TtyControl;
use crate::test_support::FakeClientOsApi;
use yinz_types::pane_size::Size;

fn saver() -> Screensaver {
    Screensaver::new(
        &Config::default(),
        &Options {
            disable_splash_screen: Some(true),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn identities_use_local_client_details_and_refresh_server_details_in_both_phases() {
    let mut saver = saver();
    let os = FakeClientOsApi::new().capturing_stdout();
    let start = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
    saver.tick(&os, None, start, true).unwrap();
    let output = String::from_utf8(os.stdout_bytes()).unwrap();
    assert!(output.contains(&identity_line(
        "Client",
        yinz_runtime::consts::VERSION,
        &yinz_runtime::host_identity::host_display_name(),
    )));
    assert!(output.contains("Server: unknown on unknown"));

    saver.set_server_identity("server-version", "server-box");
    for seconds in [1, 12] {
        let before = os.stdout_bytes().len();
        saver
            .tick(&os, None, start + Duration::from_secs(seconds), true)
            .unwrap();
        let output = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
        assert!(output.contains("Server: server-version on server-box"));
        assert!(!output.contains("Server: unknown"));
    }
    saver.set_server_identity("new-version", "new-host");
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, start + Duration::from_secs(13), true)
        .unwrap();
    let output = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
    assert!(output.contains("Server: new-version on new-host"));
    assert!(!output.contains("server-box"));
}

#[test]
fn identity_components_remove_controls_and_default_empty_values() {
    assert_eq!(
        identity_line("Server", "\n\x1b\t", "\r"),
        "Server: unknown on unknown"
    );
    assert_eq!(
        identity_line("Server", "1.2\n3", "box\x07\u{009b}"),
        "Server: 1.23 on box"
    );
}

#[test]
fn defaults_idle_deadline_activity_and_wake_consumption() {
    assert!(Screensaver::new(
        &Config::default(),
        &Options {
            screensaver: Some(false),
            ..Default::default()
        }
    )
    .is_none());
    let mut saver = saver();
    let os = FakeClientOsApi::new().capturing_stdout();
    let start = saver.activity.lock().unwrap().last_input;
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(60))
    );
    assert!(!saver
        .tick(&os, None, start + Duration::from_secs(59), true)
        .unwrap());
    assert!(!saver.active());
    let paste = InputInstruction::KeyEvent(InputEvent::Paste("secret".into()), vec![]);
    assert!(!record_input(&saver.activity, &paste, start + Duration::from_secs(59)).unwrap());
    saver
        .tick(&os, None, start + Duration::from_secs(60), true)
        .unwrap();
    assert!(!saver.active());
    saver
        .tick(&os, None, start + Duration::from_secs(119), true)
        .unwrap();
    assert!(saver.active());
    assert!(String::from_utf8(os.stdout_bytes())
        .unwrap()
        .contains("\x1b[?25l"));
    assert!(record_input(&saver.activity, &paste, start + Duration::from_secs(120)).unwrap());
    assert!(!record_input(&saver.activity, &paste, start + Duration::from_secs(120)).unwrap());
    assert!(saver
        .tick(&os, None, start + Duration::from_secs(120), true)
        .unwrap());
    assert!(!saver.active());
    assert!(saver.wake_at(start).unwrap().is_none());
    saver
        .tick(&os, None, start + Duration::from_secs(180), true)
        .unwrap();
    assert!(!saver.active(), "wait for the server's repaint completion");
    saver.repaint_complete();
    saver
        .tick(&os, None, start + Duration::from_secs(240), true)
        .unwrap();
    assert!(saver.active());
}

#[test]
fn protocol_focus_and_resize_do_not_reset_idle_but_recovery_does() {
    let mut saver = saver();
    let os = FakeClientOsApi::new();
    let start = saver.activity.lock().unwrap().last_input;
    for instruction in [
        InputInstruction::HostTerminalFocusChanged(true),
        InputInstruction::KeyEvent(InputEvent::FocusLost, vec![]),
        InputInstruction::AnsiStdinInstructions(vec![]),
    ] {
        assert!(!record_input(
            &saver.activity,
            &instruction,
            start + Duration::from_secs(59)
        )
        .unwrap());
    }
    saver
        .tick(&os, None, start + Duration::from_secs(60), true)
        .unwrap();
    assert!(saver.active());
    saver
        .tick(&os, None, start + Duration::from_secs(61), false)
        .unwrap();
    assert!(!saver.active());
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(121))
    );
}

#[test]
fn loops_at_splash_speed_and_repaints_on_resize_without_waiting_for_frame() {
    for millis in [0, 50, 250, 500, 950] {
        assert_eq!(
            animation_phase(Duration::from_millis(millis)).1,
            Phase::Original(Duration::from_millis(millis))
        );
    }
    for millis in [1000, 2000, 3999] {
        assert_eq!(
            animation_phase(Duration::from_millis(millis)).1,
            Phase::Original(Duration::from_millis(millis))
        );
    }
    assert_eq!(
        animation_phase(Duration::from_secs(4)).1,
        Phase::Original(Duration::ZERO)
    );
    assert_eq!(
        animation_phase(Duration::from_millis(4250)).1,
        Phase::Original(Duration::from_millis(250))
    );
    let mut saver = saver();
    let os = FakeClientOsApi::new().capturing_stdout();
    let now = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
    saver.tick(&os, None, now, true).unwrap();
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, now + Duration::from_millis(25), true)
        .unwrap();
    assert_eq!(before, os.stdout_bytes().len());
    let resized = os.clone().with_terminal_size(Size {
        rows: 30,
        cols: 100,
    });
    saver
        .tick(&resized, None, now + Duration::from_millis(26), true)
        .unwrap();
    assert!(os.stdout_bytes().len() > before);
    let tiny = os.clone().with_terminal_size(Size { rows: 2, cols: 2 });
    assert!(saver
        .tick(&tiny, None, now + Duration::from_millis(27), true)
        .unwrap());
    assert!(!saver.active());
}

#[test]
fn mouse_and_both_keyboard_protocols_reset_idle_and_wake_once() {
    use yinz_key_codec::vendored::termwiz::input::{Modifiers, MouseButtons, MouseEvent};
    use yinz_types::keys::{BareKey, KeyWithModifier};
    let key = KeyWithModifier::new(BareKey::Char('x'));
    for instruction in [
        InputInstruction::KeyWithModifierEvent(key.clone(), b"x".to_vec(), false),
        InputInstruction::KeyWithModifierEvent(key, b"x".to_vec(), true),
        InputInstruction::KeyEvent(
            InputEvent::Mouse(MouseEvent {
                x: 5,
                y: 5,
                mouse_buttons: MouseButtons::LEFT,
                modifiers: Modifiers::NONE,
            }),
            vec![],
        ),
    ] {
        let mut saver = saver();
        let os = FakeClientOsApi::new();
        let start = saver.activity.lock().unwrap().last_input;
        assert!(!record_input(
            &saver.activity,
            &instruction,
            start + Duration::from_secs(59)
        )
        .unwrap());
        assert_eq!(
            saver.wake_at(start).unwrap(),
            Some(start + Duration::from_secs(119))
        );
        saver
            .tick(&os, None, start + Duration::from_secs(119), true)
            .unwrap();
        assert!(record_input(
            &saver.activity,
            &instruction,
            start + Duration::from_secs(120)
        )
        .unwrap());
        assert!(!record_input(
            &saver.activity,
            &instruction,
            start + Duration::from_secs(120)
        )
        .unwrap());
    }
}

#[test]
fn pause_skips_redundant_frames_but_resizes_and_restarts_on_time() {
    let mut saver = saver();
    let os = FakeClientOsApi::new().capturing_stdout();
    let start = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
    saver.tick(&os, None, start, true).unwrap();
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + splash::FRAME_INTERVAL)
    );
    // A late frame goes straight to the resting pose without replaying missed frames.
    saver
        .tick(&os, None, start + Duration::from_millis(1200), true)
        .unwrap();
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(4))
    );
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, start + Duration::from_secs(2), true)
        .unwrap();
    assert_eq!(os.stdout_bytes().len(), before);
    // These widths yield the same centered stage, but the hint column moves.
    let resized = os.clone().with_terminal_size(Size { rows: 24, cols: 81 });
    assert_eq!(
        Stage::new(os.get_terminal_size()),
        Stage::new(resized.get_terminal_size())
    );
    saver
        .tick(&resized, None, start + Duration::from_secs(2), true)
        .unwrap();
    let output = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
    assert!(output.contains("press any key"));
    assert!(output.contains("\x1b[24;35H"));
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(4))
    );
    saver
        .tick(&resized, None, start + Duration::from_secs(4), true)
        .unwrap();
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(4) + splash::FRAME_INTERVAL)
    );
}

#[test]
fn three_original_loops_alternate_with_one_minute_of_bouncing() {
    for cycle in [0, 1, 100] {
        let start = Duration::from_secs(cycle * 72);
        for seconds in [0, 4, 8] {
            assert_eq!(
                animation_phase(start + Duration::from_secs(seconds)).1,
                Phase::Original(Duration::ZERO)
            );
        }
        assert_eq!(
            animation_phase(start + Duration::from_millis(11999)).1,
            Phase::Original(Duration::from_millis(3999))
        );
        assert_eq!(
            animation_phase(start + Duration::from_secs(12)).1,
            Phase::Bouncing(Duration::ZERO)
        );
        assert_eq!(
            animation_phase(start + Duration::from_millis(71999)).1,
            Phase::Bouncing(Duration::from_millis(59999))
        );
    }
}

#[test]
fn bouncing_repaints_on_resize_returns_to_the_quote_and_restarts_after_waking() {
    let mut saver = saver();
    let os = FakeClientOsApi::new().capturing_stdout();
    let start = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
    saver.tick(&os, None, start, true).unwrap();
    let original = os.stdout_bytes();
    saver
        .tick(&os, None, start + Duration::from_secs(9), true)
        .unwrap();
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(12))
    );
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, start + Duration::from_secs(12), true)
        .unwrap();
    let bouncing = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
    assert!(!bouncing.contains("Aristotle"));
    assert!(bouncing.contains("press any key"));
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_millis(12050))
    );
    let before = os.stdout_bytes().len();
    let resized = os.clone().with_terminal_size(Size { rows: 10, cols: 28 });
    saver
        .tick(&resized, None, start + Duration::from_millis(12001), true)
        .unwrap();
    assert!(os.stdout_bytes().len() > before);
    assert!(saver.active());
    saver
        .tick(&os, None, start + Duration::from_millis(71999), true)
        .unwrap();
    assert_eq!(
        saver.wake_at(start).unwrap(),
        Some(start + Duration::from_secs(72))
    );
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, start + Duration::from_secs(72), true)
        .unwrap();
    let restored = &os.stdout_bytes()[before..];
    // Activation additionally hides existing Kitty placements; the restored frame is identical.
    assert!(original.ends_with(restored));
    assert!(String::from_utf8(restored.to_vec())
        .unwrap()
        .contains("Aristotle"));
    saver
        .tick(&os, None, start + Duration::from_secs(84), true)
        .unwrap();
    let input = InputInstruction::KeyEvent(InputEvent::Paste("wake".into()), vec![]);
    assert!(record_input(&saver.activity, &input, start + Duration::from_secs(85)).unwrap());
    assert!(saver
        .tick(&os, None, start + Duration::from_secs(85), true)
        .unwrap());
    saver.repaint_complete();
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, start + Duration::from_secs(145), true)
        .unwrap();
    assert_eq!(&os.stdout_bytes()[before..], original);
}

fn assert_horizontal_reversal(previous: Direction, next: Direction) {
    let points_right = |direction| matches!(direction, Direction::UpRight | Direction::DownRight);
    assert_ne!(points_right(previous), points_right(next));
}

#[test]
fn direction_is_selected_once_per_observed_cycle_and_survives_frames_and_resizes() {
    let mut saver = saver();
    let os = FakeClientOsApi::new();
    let small = os.clone().with_terminal_size(Size { rows: 10, cols: 28 });
    let start = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
    assert_eq!(saver.direction, None);
    saver.tick(&os, None, start, true).unwrap();
    let first = saver.direction.unwrap();
    assert_eq!(saver.cycle, Some(0));
    // Include a redundant tick, original-loop boundaries, delayed bouncing entry,
    // letter changes, and a late frame near the end of the bounce.
    for millis in [0, 25, 50, 4000, 8000, 13500, 16000, 20000, 71999] {
        let now = start + Duration::from_millis(millis);
        saver.tick(&os, None, now, true).unwrap();
        saver.tick(&small, None, now, true).unwrap();
        saver.tick(&os, None, now, true).unwrap();
        assert_eq!(saver.direction, Some(first));
        assert_eq!(saver.cycle, Some(0));
    }
    saver
        .tick(&os, None, start + Duration::from_secs(72), true)
        .unwrap();
    let second = saver.direction.unwrap();
    assert_horizontal_reversal(first, second);
    assert_eq!(saver.cycle, Some(1));
    // Skip an even number of boundaries: replaying them would incorrectly preserve
    // the horizontal sign. Select once against the last direction actually shown.
    saver
        .tick(&small, None, start + Duration::from_secs(3 * 72 + 18), true)
        .unwrap();
    let third = saver.direction.unwrap();
    assert_horizontal_reversal(second, third);
    assert_eq!(saver.cycle, Some(3));
    saver
        .tick(&os, None, start + Duration::from_secs(3 * 72 + 22), true)
        .unwrap();
    assert_eq!(saver.direction, Some(third));
}

#[test]
fn dismissal_and_ineligibility_clear_the_cycle_but_retain_direction_history() {
    for cause in ["input", "size", "eligibility"] {
        let mut saver = saver();
        let os = FakeClientOsApi::new();
        let start = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
        saver.tick(&os, None, start, true).unwrap();
        let previous = saver.direction.unwrap();
        let wake = start + Duration::from_secs(13);
        let needs_repaint = match cause {
            "input" => {
                let input = InputInstruction::KeyEvent(InputEvent::Paste("wake".into()), vec![]);
                assert!(record_input(&saver.activity, &input, wake).unwrap());
                saver.tick(&os, None, wake, true).unwrap()
            },
            "size" => {
                let tiny = os.clone().with_terminal_size(Size { rows: 9, cols: 27 });
                saver.tick(&tiny, None, wake, true).unwrap()
            },
            _ => saver.tick(&os, None, wake, false).unwrap(),
        };
        assert_eq!(needs_repaint, cause != "eligibility");
        assert!(!saver.active());
        assert_eq!(saver.cycle, None);
        assert_eq!(saver.direction, Some(previous));
        saver.repaint_complete();
        saver
            .tick(&os, None, wake + Duration::from_secs(60), true)
            .unwrap();
        assert!(saver.active());
        assert_eq!(saver.cycle, Some(0));
        assert_horizontal_reversal(previous, saver.direction.unwrap());
    }
}

#[test]
fn phase_and_absolute_cycle_share_exact_boundaries() {
    for cycle in [0, 1, 100, u64::MAX / 72 - 1] {
        let start = Duration::from_secs(cycle * 72);
        assert_eq!(
            animation_phase(start),
            (u128::from(cycle), Phase::Original(Duration::ZERO))
        );
        assert_eq!(
            animation_phase(start + Duration::from_secs(12)),
            (u128::from(cycle), Phase::Bouncing(Duration::ZERO))
        );
        assert_eq!(
            animation_phase(start + Duration::from_millis(71999)),
            (
                u128::from(cycle),
                Phase::Bouncing(Duration::from_millis(59999))
            )
        );
        assert_eq!(
            animation_phase(start + Duration::from_secs(72)),
            (u128::from(cycle) + 1, Phase::Original(Duration::ZERO))
        );
    }
}
