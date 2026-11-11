use super::*;
use yinz_ipc::ipc::ExitReason;

#[test]
fn a_render_frame_is_applied_after_a_recoverable_error() {
    use yinz_types::structured_render::{FrameBuilder, WireCell};

    let (sender, messages) = mpsc::sync_channel(2);
    sender
        .send(ServerToClientMsg::LogError {
            lines: vec!["recoverable server error".into()],
        })
        .unwrap();
    let mut frame = FrameBuilder::new(8, 2, 1);
    frame.push_row(
        0,
        0,
        &[WireCell {
            ch: 'x' as u32,
            ..WireCell::BLANK
        }],
    );
    sender
        .send(ServerToClientMsg::RenderFrame {
            frame: frame.finish(),
        })
        .unwrap();

    assert!(receive_message(Some(&messages)).unwrap().is_none());
    let Some(ServerToClientMsg::RenderFrame { frame }) = receive_message(Some(&messages)).unwrap()
    else {
        panic!("expected the next render frame");
    };
    let mut state = TerminalState::new(2, 8);
    assert_eq!(state.apply_frame(&frame).unwrap().seq, 1);
    assert_eq!(state.screen().cell(0, 0).character(), 'x');
}

#[test]
fn recoverable_errors_allow_subsequent_messages() {
    let (sender, messages) = mpsc::channel();
    sender
        .send(ServerToClientMsg::LogError {
            lines: vec!["plugin failed to load".into(), "layout rejected".into()],
        })
        .unwrap();
    sender
        .send(ServerToClientMsg::SetMouseMode { enabled: true })
        .unwrap();
    assert!(receive_message(Some(&messages)).unwrap().is_none());
    assert!(matches!(
        receive_message(Some(&messages)).unwrap(),
        Some(ServerToClientMsg::SetMouseMode { enabled: true })
    ));
    assert!(receive_message(Some(&messages)).unwrap().is_none());
}

#[test]
fn explicit_error_exit_remains_fatal() {
    let (sender, messages) = mpsc::channel();
    sender
        .send(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Error("connection failed".into()),
        })
        .unwrap();
    let error = receive_message(Some(&messages)).unwrap_err();
    assert_eq!(error.to_string(), "connection failed");
}

#[test]
fn disconnected_connection_remains_fatal() {
    let (sender, messages) = mpsc::channel();
    drop(sender);
    let error = receive_message(Some(&messages)).unwrap_err();
    assert!(error.to_string().contains("closed unexpectedly"));
}

#[test]
fn normal_exit_is_forwarded_to_the_window() {
    let (sender, messages) = mpsc::channel();
    sender
        .send(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        })
        .unwrap();
    assert!(matches!(
        receive_message(Some(&messages)).unwrap(),
        Some(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal
        })
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn option_as_alt_preserves_each_side_and_defaults_to_none() {
    use winit::platform::macos::OptionAsAlt as Native;
    use yinz_types::input::native_terminal::OptionAsAlt;
    for (setting, expected) in [
        (OptionAsAlt::None, Native::None),
        (OptionAsAlt::Left, Native::OnlyLeft),
        (OptionAsAlt::Right, Native::OnlyRight),
        (OptionAsAlt::Both, Native::Both),
    ] {
        assert_eq!(native_option_as_alt(setting), expected);
    }
    assert_eq!(
        native_option_as_alt(
            TerminalConfig::default()
                .macos_option_as_alt
                .unwrap_or_default()
        ),
        Native::None
    );
}

#[test]
fn fractional_scroll_accumulates_in_cell_units() {
    use winit::dpi::PhysicalPosition;
    let mut scroll = ScrollAccumulator::default();
    for _ in 0..19 {
        assert_eq!(
            scroll.steps(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 1.0)),
                10.0,
                20.0
            ),
            (0, 0)
        );
    }
    assert_eq!(
        scroll.steps(
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 1.0)),
            10.0,
            20.0
        ),
        (0, 1)
    );
}

#[test]
fn fractional_scroll_retains_remainders_and_cancels_direction_changes() {
    let mut scroll = ScrollAccumulator::default();
    let mut steps = |x, y| scroll.steps(MouseScrollDelta::LineDelta(x, y), 10.0, 20.0);
    assert_eq!(steps(0.75, -0.5), (0, 0));
    assert_eq!(steps(-0.5, -0.75), (0, -1));
    assert_eq!(steps(1.0, 0.5), (1, 0));
    assert_eq!(steps(0.75, 0.75), (1, 1));
}

#[test]
fn large_scrolls_are_capped_without_deferring_whole_steps() {
    let mut scroll = ScrollAccumulator::default();
    assert_eq!(
        scroll.steps(MouseScrollDelta::LineDelta(100.5, -200.75), 10.0, 20.0),
        (0, -20)
    );
    assert_eq!(
        scroll.steps(MouseScrollDelta::LineDelta(0.0, 0.0), 10.0, 20.0),
        (0, 0)
    );
    assert_eq!(
        scroll.steps(MouseScrollDelta::LineDelta(0.5, -0.25), 10.0, 20.0),
        (1, -1)
    );
}

#[test]
fn failed_link_opening_allows_subsequent_opening() {
    links::launch::forget_opened();
    open_link("vscode://example");
    links::launch::fail_next();
    open_link("https://example.com/fails");
    open_link("https://example.com/works");
    let opened = links::launch::opened();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].1, "https://example.com/works");
}
