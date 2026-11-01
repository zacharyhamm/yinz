use super::*;
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
fn loops_at_quarter_speed_and_repaints_on_resize_without_waiting_for_frame() {
    assert_eq!(
        animation_time(Duration::from_secs(1)),
        Duration::from_millis(250)
    );
    assert_eq!(animation_time(Duration::from_secs(4)), Duration::ZERO);
    assert_eq!(
        animation_time(Duration::from_secs(9)),
        Duration::from_millis(250)
    );
    let mut saver = saver();
    let os = FakeClientOsApi::new().capturing_stdout();
    let now = saver.activity.lock().unwrap().last_input + Duration::from_secs(60);
    saver.tick(&os, None, now, true).unwrap();
    let before = os.stdout_bytes().len();
    saver
        .tick(&os, None, now + Duration::from_millis(100), true)
        .unwrap();
    assert_eq!(before, os.stdout_bytes().len());
    let resized = os.clone().with_terminal_size(Size {
        rows: 30,
        cols: 100,
    });
    saver
        .tick(&resized, None, now + Duration::from_millis(101), true)
        .unwrap();
    assert!(os.stdout_bytes().len() > before);
    let tiny = os.clone().with_terminal_size(Size { rows: 2, cols: 2 });
    assert!(saver
        .tick(&tiny, None, now + Duration::from_millis(102), true)
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
