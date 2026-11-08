use super::*;

#[test]
fn resume_detection_ignores_idle_connections_and_detects_suspend() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let mut detector = ResumeDetector::new(start);
    // No server traffic is necessary: bounded reads keep sampling while idle.
    for tick in 1..=600 {
        assert!(!detector.resumed(start + READ_POLL * tick));
    }
    let wake = start + Duration::from_secs(3600);
    assert!(detector.resumed(wake));
    assert!(!detector.resumed(wake + READ_POLL));
}

#[test]
fn resume_detection_recovers_from_backward_clock_adjustments() {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let mut detector = ResumeDetector::new(start);
    let earlier = start - Duration::from_secs(60);
    assert!(!detector.resumed(earlier));
    assert!(!detector.resumed(earlier + READ_POLL));
    assert!(detector.resumed(earlier + Duration::from_secs(6)));
}

#[test]
fn backoff_has_bounded_jitter_and_resets_only_after_a_minute() {
    for sample in [0, 200, 400] {
        let mut backoff = Backoff::default();
        for nominal in [0, 1, 2, 4, 8, 16, 30, 30, 30] {
            let delay = backoff.delay(sample);
            assert!(delay <= Duration::from_secs(30));
            assert!(delay.as_secs_f64() >= f64::from(nominal) * 0.8);
            assert!(delay.as_secs_f64() <= (f64::from(nominal) * 1.2).min(30.));
        }
        backoff.healthy(Duration::from_secs(59));
        assert!(backoff.delay(sample) >= Duration::from_secs(24));
        backoff.healthy(Duration::from_secs(60));
        assert_eq!(backoff.delay(sample), Duration::ZERO);
    }
}

#[test]
fn failed_transport_unblocks_input_and_generations_reject_stale_failures() {
    let mut command = CommandIsExecuting::new();
    let control = RecoveryControl::new(command.clone());
    command.blocking_input_thread();
    control.lost(0, "write failed".into());
    let (done, rx) = mpsc::channel();
    std::thread::spawn(move || {
        command.wait_until_input_thread_is_unblocked();
        done.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.phase(), Phase::Reconnecting);
    assert!(!control.can_write());
    control.replacing();
    assert!(!control.accepts(0));
    assert_eq!(control.stamp(), (1, false));
    control.lost(0, "late EOF".into());
    assert_eq!(control.phase(), Phase::Repainting);
    assert!(control.repainted());
    control.lost(1, "second outage".into());
    assert_eq!(control.phase(), Phase::Reconnecting);
    control.stop();
    control.lost(1, "late write failure".into());
    assert_eq!(control.phase(), Phase::Stopped);
}

#[test]
fn recovery_waits_for_the_current_routers_drain_marker_before_disconnect() {
    let os = crate::test_support::FakeClientOsApi::new();
    let mut recovery = Recovery::new(
        "host".into(),
        CommandIsExecuting::new(),
        &Default::default(),
        &Default::default(),
    );
    recovery.rendered = true;
    recovery.control.replacing();
    recovery.control.repainted();
    recovery
        .control
        .lost(1, "Host key verification failed".into());
    let now = Instant::now();
    assert!(recovery.poll(&os, "work", now).unwrap().is_none());
    assert!(recovery.next_retry.is_none());
    recovery.control.reader_drained(0);
    assert!(recovery.poll(&os, "work", now).unwrap().is_none());
    recovery.control.reader_drained(1);
    assert!(recovery
        .poll(&os, "work", now)
        .err()
        .unwrap()
        .to_string()
        .contains("Host key verification failed"));
    assert!(recovery.attempt.is_none());
}

#[test]
fn replay_contains_capabilities_and_current_size_but_never_keyboard_input() {
    let control = RecoveryControl::new(CommandIsExecuting::new());
    control.cache(&ClientToServerMsg::KittyGraphicsSupport { supported: false });
    control.cache(&ClientToServerMsg::KittyGraphicsSupport { supported: true });
    control.cache(&ClientToServerMsg::SixelSupport { supported: true });
    control.cache(&ClientToServerMsg::Paste("OFFLINE".into()));
    let os = crate::test_support::FakeClientOsApi::new();
    control.replay(&os);
    let messages = os.sent_messages();
    assert_eq!(messages.len(), 4);
    assert!(matches!(
        messages[0],
        ClientToServerMsg::TerminalResize { .. }
    ));
    assert!(matches!(
        messages[1],
        ClientToServerMsg::KittyGraphicsSupport { supported: true }
    ));
    assert!(messages
        .iter()
        .all(|message| !matches!(message, ClientToServerMsg::Paste(_))));
}

#[test]
fn recovery_reannounces_after_repaint_even_when_the_host_recently_pinged() {
    use crate::nested_reannounce::NestedReannounce;
    use yinz_ipc::nested_session::{self, NestedSessionCapability, NestedSessionMessage};

    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "original".into());
    nested.stop();
    nested.note_host_contact();
    let mut recovery = Recovery::new(
        "host".into(),
        CommandIsExecuting::new(),
        &Default::default(),
        &Default::default(),
    );
    recovery.first_render(&os, "original", Instant::now());
    assert!(os.stdout_bytes().is_empty());

    recovery.control.lost(0, "EOF".into());
    recovery.control.replacing();
    // A short outage never triggers the silence timer. Use the session's current
    // name in case it was renamed since the original announcement.
    nested.note_host_contact();
    recovery.first_render(&os, "renamed", Instant::now());
    assert_eq!(recovery.control.phase(), Phase::Connected);
    let expected = nested_session::encode_frame(&NestedSessionMessage::Announce {
        session_name: "renamed".into(),
        capabilities: vec![NestedSessionCapability::NestedControl],
    });
    assert_eq!(os.stdout_bytes(), expected);
    recovery.first_render(&os, "renamed", Instant::now());
    assert_eq!(os.stdout_bytes(), expected);
}

#[test]
fn replay_preserves_only_the_latest_observed_focus_without_inventing_a_default() {
    for changes in [vec![], vec![false], vec![false, true], vec![true, false]] {
        let control = RecoveryControl::new(CommandIsExecuting::new());
        for focused in &changes {
            control.cache(&ClientToServerMsg::HostTerminalFocusChanged { focused: *focused });
        }
        control.lost(0, "EOF".into());
        control.replacing();
        let os = crate::test_support::FakeClientOsApi::new();
        control.replay(&os);
        let replayed: Vec<_> = os
            .sent_messages()
            .into_iter()
            .filter_map(|message| match message {
                ClientToServerMsg::HostTerminalFocusChanged { focused } => Some(focused),
                _ => None,
            })
            .collect();
        assert_eq!(
            replayed,
            changes.last().copied().into_iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn replay_merges_partial_pixel_reports_in_either_order() {
    use yinz_ipc::ipc::PixelDimensions;
    use yinz_types::pane_size::SizeInPixels;

    for reverse in [false, true] {
        let control = RecoveryControl::new(CommandIsExecuting::new());
        control.cache(&ClientToServerMsg::TerminalPixelDimensions {
            pixel_dimensions: PixelDimensions {
                text_area_size: Some(SizeInPixels {
                    height: 600,
                    width: 800,
                }),
                character_cell_size: Some(SizeInPixels {
                    height: 10,
                    width: 5,
                }),
                text_area_grid_size: Some(Size {
                    rows: 60,
                    cols: 160,
                }),
            },
        });
        control.lost(0, "EOF".into());
        let area = PixelDimensions {
            text_area_size: Some(SizeInPixels {
                height: 720,
                width: 1280,
            }),
            text_area_grid_size: Some(Size {
                rows: 40,
                cols: 128,
            }),
            ..Default::default()
        };
        let cell = PixelDimensions {
            character_cell_size: Some(SizeInPixels {
                height: 18,
                width: 10,
            }),
            ..Default::default()
        };
        for report in if reverse { [cell, area] } else { [area, cell] } {
            control.cache(&ClientToServerMsg::TerminalPixelDimensions {
                pixel_dimensions: report,
            });
        }
        // An empty report must not erase any previously observed dimensions.
        control.cache(&ClientToServerMsg::TerminalPixelDimensions {
            pixel_dimensions: Default::default(),
        });
        let os = crate::test_support::FakeClientOsApi::new();
        control.replay(&os);
        let replayed: Vec<_> = os
            .sent_messages()
            .into_iter()
            .filter_map(|message| match message {
                ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions } => {
                    Some(pixel_dimensions)
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            replayed,
            vec![PixelDimensions {
                text_area_size: area.text_area_size,
                character_cell_size: cell.character_cell_size,
                text_area_grid_size: area.text_area_grid_size,
            }]
        );
    }
}

#[test]
fn recovery_deadline_is_shared_with_bootstrap_and_forwarding_and_cancellation_reaps_children() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let args = dir.path().join("args");
    let endpoint = serde_json::json!({ "session_name": "work", "socket": "/remote/work",
        "yinz_version": yinz_runtime::consts::VERSION,
        "contract_version": yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION,
        "bootstrap_version": yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION,
    });
    yinz_test_support::write_warm_script(
        &dir.path().join("ssh"),
        r#"
printf '%s\n' "$*" >> "$YINZ_TEST_SSH_ARGS"
case "$*" in
    *remote-session*) printf '%s\n' "$YINZ_TEST_ENDPOINT" ;;
    *) printf '%s\n' "$$" > "$YINZ_TEST_FORWARD_PID"; read -r blocked ;;
esac
"#,
    );
    unsafe {
        std::env::set_var("PATH", dir.path());
        std::env::set_var("YINZ_SOCKET_DIR", dir.path().join("sockets"));
        std::env::set_var("YINZ_TEST_SSH_ARGS", &args);
        std::env::set_var("YINZ_TEST_ENDPOINT", endpoint.to_string());
        std::env::set_var("YINZ_TEST_FORWARD_PID", dir.path().join("pid"));
    }
    for cancel in [false, true] {
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let started = Instant::now();
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(session_switch::prepare_recovery(
                "host".into(),
                "work".into(),
                Default::default(),
                &cancelled,
                Instant::now() + Duration::from_millis(500),
                &Default::default(),
            ))
            .unwrap();
        });
        if cancel {
            stop.store(true, Ordering::Release);
        }
        assert!(result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_err());
        worker.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the forward must use the attempt's deadline"
        );
        if let Ok(pid) = std::fs::read_to_string(dir.path().join("pid")) {
            assert_eq!(
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid.trim().parse().unwrap()),
                    None
                ),
                Err(nix::errno::Errno::ESRCH)
            );
        }
    }
    let calls = std::fs::read_to_string(args).unwrap();
    assert!(calls.contains("remote-session"));
    assert!(!calls.contains("--create"));
    assert_eq!(
        std::fs::read_dir(dir.path().join("sockets/client"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn queued_stale_frames_and_session_switches_do_not_affect_the_replacement() {
    use crate::{
        client_loop::ClientLoop, nested_reannounce::NestedReannounce, render_budget::RenderBudget,
        stdin_ansi_parser::StdinAnsiParser, terminal_escapes::ClientTeardown, ClientInstruction,
    };
    use yinz_runtime::channels::{self, SenderWithContext};
    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let mut command = CommandIsExecuting::new();
    let mut recovery = Recovery::new(
        "host".into(),
        command.clone(),
        &Default::default(),
        &Default::default(),
    );
    recovery.rendered = true;
    recovery.control.replacing();
    recovery.control.repainted();
    let (tx, rx) = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    let budget = Arc::new(RenderBudget::new(100));
    budget.reserve(12).unwrap();
    for instruction in [
        ClientInstruction::FromServer(
            1,
            Box::new(ClientInstruction::Connected {
                signal_quit: false,
                update_protocol: 0,
                server_version: "replacement-build".into(),
                server_host: "replacement-host".into(),
            }),
        ),
        ClientInstruction::FromServer(
            0,
            Box::new(ClientInstruction::Connected {
                signal_quit: true,
                update_protocol: 0,
                server_version: "stale-build".into(),
                server_host: "stale-host".into(),
            }),
        ),
        ClientInstruction::FromServer(0, Box::new(ClientInstruction::Render("STALE".into()))),
        ClientInstruction::FromServer(
            0,
            Box::new(ClientInstruction::SwitchSession(Default::default())),
        ),
        ClientInstruction::FromServer(1, Box::new(ClientInstruction::Render("CURRENT".into()))),
    ] {
        tx.send(instruction).unwrap();
    }
    let saver = crate::screensaver::Screensaver::new(
        &Default::default(),
        &yinz_types::input::options::Options {
            screensaver_idle_timeout: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    let producer = std::thread::spawn({
        let os = os.clone();
        let tx = tx.clone();
        move || {
            os.stdout_attempts.wait_until(
                "replacement identity appears on the screensaver",
                yinz_test_support::default_timeout(),
                |writes| {
                    writes
                        .iter()
                        .any(|write| {
                            String::from_utf8_lossy(write)
                                .contains("Server: replacement-build on replacement-host")
                        })
                        .then_some(())
                },
            );
            tx.send(ClientInstruction::FromServer(
                1,
                Box::new(ClientInstruction::Exit(yinz_ipc::ipc::ExitReason::Normal)),
            ))
            .unwrap();
        }
    });
    let (input_tx, _input_rx) = crate::input_handler::input_control_channel();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "generation-test".into());
    nested.stop();
    let args = Default::default();
    let mut config = Default::default();
    let mut sessions =
        session_switch::Sessions::new(Some("host".into()), "work".into(), &args, &mut config);
    let signal_quit_supported = Arc::new(AtomicBool::new(true));
    let outcome = ClientLoop {
        signal_quit_supported: signal_quit_supported.clone(),
        sessions: &mut sessions,
        recovery: &mut Some(recovery),
        send_client_instructions: Some(&tx),
        remote_host: Some("host"),
        os_input: &os,
        command_is_executing: &mut command,
        stdin_ansi_parser: &Arc::new(Mutex::new(StdinAnsiParser::new())),
        send_input_control: &input_tx,
        nested_reannounce: &nested,
        teardown: ClientTeardown::new(24, false),
        synchronised_output: None,
        render_budget: &budget,
        startup_started: Instant::now(),
        splash: None,
        screensaver: Some(saver),
    }
    .run(&rx);
    producer.join().unwrap();
    assert!(!String::from_utf8(os.stdout_bytes())
        .unwrap()
        .contains("stale-host"));
    assert!(!signal_quit_supported.load(Ordering::Acquire));
    assert_eq!(outcome.exit_status, 0);
    assert!(outcome.reconnect_to_session.is_none());
    assert!(!String::from_utf8(os.stdout_bytes())
        .unwrap()
        .contains("STALE"));
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

#[test]
fn eof_starts_remote_recovery_but_explicit_detach_does_not() {
    use crate::{
        os_input_output::ServerConnection, render_budget::RenderBudget, ClientInstruction,
    };
    use yinz_ipc::ipc::{ExitReason, ServerToClientMsg};
    use yinz_runtime::channels::{self, SenderWithContext};
    for explicit in [false, true] {
        let os = crate::test_support::FakeClientOsApi::new();
        let control = RecoveryControl::new(CommandIsExecuting::new());
        os.set_remote_recovery(Some(control.clone())).unwrap();
        if explicit {
            os.queue_from_server(ServerToClientMsg::Exit {
                exit_reason: ExitReason::NormalDetached,
            });
        }
        os.queue_server_hangup();
        let (tx, rx) = channels::unbounded();
        crate::router::route(
            Box::new(os),
            SenderWithContext::new(tx),
            &RenderBudget::new(100),
        );
        let instructions: Vec<_> = rx.iter().map(|(instruction, _)| instruction).collect();
        if explicit {
            assert_eq!(control.phase(), Phase::Connected);
            assert!(
                matches!(&instructions[..], [ClientInstruction::FromServer(0, instruction)] if matches!(**instruction, ClientInstruction::Exit(ExitReason::NormalDetached)))
            );
        } else {
            assert_eq!(control.phase(), Phase::Reconnecting);
            assert!(
                matches!(&instructions[..], [ClientInstruction::FromServer(0, instruction)] if matches!(**instruction, ClientInstruction::ServerReaderDrained(0)))
            );
        }
    }
}

#[test]
fn recovery_status_counts_down_the_connection_attempt_deadline_to_zero() {
    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let mut recovery = Recovery::new(
        "host".into(),
        CommandIsExecuting::new(),
        &Default::default(),
        &Default::default(),
    );
    recovery.control.lost(0, "outage".into());
    recovery.attempt_number = 1;
    let start = Instant::now();
    // Keep the attempt pending without starting an SSH worker or waiting in real time.
    let (_sender, result) = mpsc::channel();
    recovery.attempt = Some(Attempt {
        cancelled: Arc::new(AtomicBool::new(false)),
        deadline: start + ATTEMPT_TIMEOUT,
        result,
        worker: None,
    });
    for elapsed in 0..=30 {
        let now = start + Duration::from_secs(elapsed);
        assert!(recovery.poll(&os, "work", now).unwrap().is_none());
        let before = os.stdout_bytes().len();
        recovery.paint_status(&os, "work", now).unwrap();
        let output = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
        assert!(
            output.contains(&format!("Attempt 1 · connecting ({}s left)", 30 - elapsed)),
            "elapsed {elapsed}s: {output:?}"
        );
        // Whole-second ticks keep the blink unchanged, so the timer alone must repaint.
        assert_eq!(
            output.contains(crate::terminal_escapes::CLEAR_SCREEN),
            elapsed == 0
        );
    }
    // Allow for cancellation/cleanup taking longer than the attempt's deadline.
    recovery
        .paint_status(&os, "work", start + Duration::from_secs(35))
        .unwrap();
    assert!(recovery
        .status
        .as_ref()
        .unwrap()
        .2
        .contains("connecting (0s left)"));
}

#[test]
fn recovery_status_rounds_remaining_seconds_up_for_attempts_and_retry_waits() {
    for waiting in [false, true] {
        let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
        let mut recovery = Recovery::new(
            "host".into(),
            CommandIsExecuting::new(),
            &Default::default(),
            &Default::default(),
        );
        recovery.control.lost(0, "outage".into());
        let start = Instant::now();
        let deadline = start + Duration::from_secs(2);
        let (_sender, result) = mpsc::channel();
        if waiting {
            recovery.next_retry = Some(deadline);
        } else {
            recovery.attempt = Some(Attempt {
                cancelled: Arc::new(AtomicBool::new(false)),
                deadline,
                result,
                worker: None,
            });
        }
        for (elapsed_ms, seconds) in [(0, 2), (999, 2), (1000, 1), (1999, 1), (2000, 0), (2500, 0)]
        {
            recovery
                .paint_status(&os, "work", start + Duration::from_millis(elapsed_ms))
                .unwrap();
            let expected = if waiting {
                format!("next retry {seconds}s")
            } else {
                format!("connecting ({seconds}s left)")
            };
            assert!(
                recovery.status.as_ref().unwrap().2.contains(&expected),
                "waiting={waiting}, elapsed={elapsed_ms}ms"
            );
        }
    }
}

#[test]
fn recovery_status_uses_client_theme_for_text_and_screen_background() {
    use yinz_config::theme::Theme;
    use yinz_types::colors::PaletteColor;

    let mut config = Config {
        themes: yinz_config::assets::get_default_themes(),
        ..Default::default()
    };
    config.themes.insert(
        "indexed".into(),
        Theme {
            palette: yinz_types::style::Styling {
                text_unselected: StyleDeclaration {
                    base: PaletteColor::EightBit(42),
                    background: PaletteColor::EightBit(17),
                    ..Default::default()
                },
                ..default_palette().into()
            },
            terminal: None,
            sourced_from_external_file: false,
        },
    );
    for (theme, foreground, background) in [
        (Some("dracula"), "2;255;255;255", "2;0;0;0"),
        (Some("ayu-light"), "2;92;97;102", "2;231;234;237"),
        (Some("indexed"), "5;42", "5;17"),
        (None, "5;255", "5;16"),
        (Some("missing-theme"), "5;255", "5;16"),
    ] {
        let options = Options {
            theme: theme.map(str::to_owned),
            ..Default::default()
        };
        let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
        let mut recovery =
            Recovery::new("host".into(), CommandIsExecuting::new(), &config, &options);
        recovery.control.lost(0, "outage".into());
        let now = Instant::now();
        // The first paint and a resize must clear with the theme's background;
        // animation updates must keep the same foreground and background.
        for (tick, size, clear) in [
            (0, Size { rows: 24, cols: 80 }, true),
            (1, Size { rows: 24, cols: 80 }, false),
            (
                2,
                Size {
                    rows: 40,
                    cols: 120,
                },
                true,
            ),
        ] {
            let before = os.stdout_bytes().len();
            let resized = os.clone().with_terminal_size(size);
            recovery
                .paint_status(&resized, "work", now + Duration::from_millis(500 * tick))
                .unwrap();
            let output = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
            let colors = format!("\x1b[38;{foreground}m\x1b[48;{background}m");
            assert!(
                output.starts_with(&format!("\x1b7\x1b[0m{colors}")),
                "{theme:?}"
            );
            assert_eq!(
                output.contains(crate::terminal_escapes::CLEAR_SCREEN),
                clear
            );
            if clear {
                assert!(output.contains(&format!(
                    "{colors}{}",
                    crate::terminal_escapes::CLEAR_SCREEN
                )));
            }
            assert!(!output.contains("\x1b[1;7m"));
            assert!(output.contains("Reconnecting"));
            assert!(output.ends_with("\x1b[0m\x1b8"));
        }
    }
}

#[test]
fn recovery_status_follows_host_theme_changes_only_with_a_valid_theme_pair() {
    let config = Config {
        themes: yinz_config::assets::get_default_themes(),
        ..Default::default()
    };
    for (dark, light, follows_host) in [
        (Some("dracula"), Some("ayu-light"), true),
        (Some("dracula"), None, false),
        (None, Some("ayu-light"), false),
        (Some("missing-theme"), Some("ayu-light"), false),
        (Some("dracula"), Some("missing-theme"), false),
    ] {
        let options = Options {
            theme: Some("dracula".into()),
            theme_dark: dark.map(str::to_owned),
            theme_light: light.map(str::to_owned),
            ..Default::default()
        };
        let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
        let mut recovery =
            Recovery::new("host".into(), CommandIsExecuting::new(), &config, &options);
        recovery.control.lost(0, "outage".into());
        let now = Instant::now();
        recovery.paint_status(&os, "work", now).unwrap();
        for mode in [HostTerminalThemeMode::Light, HostTerminalThemeMode::Dark] {
            let before = os.stdout_bytes().len();
            recovery
                .control
                .cache(&ClientToServerMsg::HostTerminalThemeChanged { mode });
            // Keep time and geometry fixed: changing the theme alone must repaint.
            recovery.paint_status(&os, "work", now).unwrap();
            let output = String::from_utf8(os.stdout_bytes()[before..].to_vec()).unwrap();
            if follows_host {
                let colors = match mode {
                    HostTerminalThemeMode::Light => "\x1b[38;2;92;97;102m\x1b[48;2;231;234;237m",
                    HostTerminalThemeMode::Dark => "\x1b[38;2;255;255;255m\x1b[48;2;0;0;0m",
                };
                assert!(output.contains(&format!(
                    "{colors}{}",
                    crate::terminal_escapes::CLEAR_SCREEN
                )));
            } else {
                assert!(output.is_empty());
            }
            let before = os.stdout_bytes().len();
            recovery.paint_status(&os, "work", now).unwrap();
            assert_eq!(os.stdout_bytes().len(), before);
        }
    }
}

#[test]
fn recovery_status_uses_current_terminal_geometry_and_permanent_failures_do_not_spawn_attempts() {
    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let now = Instant::now();
    let mut recovery = Recovery::new(
        "host".into(),
        CommandIsExecuting::new(),
        &Default::default(),
        &Default::default(),
    );
    recovery.rendered = true;
    recovery.control.lost(0, "EOF".into());
    recovery.next_retry = Some(now + Duration::from_secs(10));
    recovery.paint_status(&os, "work", now).unwrap();
    let resized = os.clone().with_terminal_size(Size {
        rows: 40,
        cols: 120,
    });
    recovery.paint_status(&resized, "renamed", now).unwrap();
    let output = String::from_utf8(os.stdout_bytes()).unwrap();
    assert!(output.contains("\x1b[9;1H┌"));
    assert!(output.contains("\x1b[17;21H┌"));
    assert!(output.contains("📡 Reconnecting"));
    assert!(output.contains("host / renamed"));
    assert!(output.contains("Ctrl-C to cancel"));
    let narrow = crate::test_support::FakeClientOsApi::new()
        .capturing_stdout()
        .with_terminal_size(Size { rows: 12, cols: 40 });
    recovery.paint_status(&narrow, "work", now).unwrap();
    let output = String::from_utf8(narrow.stdout_bytes()).unwrap();
    assert!(output.contains("host / work"));
    assert!(output.contains("Ctrl-C to cancel"));
    recovery.control.state.lock().unwrap().failure = Some("Host key verification failed".into());
    recovery.control.reader_drained(0);
    let error = recovery.poll(&os, "renamed", now).err().unwrap();
    assert!(error.to_string().contains("Host key verification failed"));
    assert!(recovery.attempt.is_none());
}

#[test]
fn recovery_status_clears_old_dialog_on_resize_and_blinks_without_other_changes() {
    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let mut recovery = Recovery::new(
        "host".into(),
        CommandIsExecuting::new(),
        &Default::default(),
        &Default::default(),
    );
    recovery.control.lost(0, "outage".into());
    let now = Instant::now();
    for rows in [24, 40, 50, 12, 30] {
        let previous_len = os.stdout_bytes().len();
        let resized = os.clone().with_terminal_size(Size { rows, cols: 100 });
        recovery.paint_status(&resized, "work", now).unwrap();
        let output = String::from_utf8(os.stdout_bytes()[previous_len..].to_vec()).unwrap();
        assert!(output.starts_with("\x1b7"));
        assert!(output.ends_with("\x1b8"));
        assert!(output.contains(crate::terminal_escapes::CLEAR_SCREEN));
        assert!(output.contains(&format!("\x1b[{};11H┌", (rows - 8) / 2 + 1)));
    }
    let resized = os.clone().with_terminal_size(Size {
        rows: 30,
        cols: 100,
    });
    let previous_len = os.stdout_bytes().len();
    recovery
        .paint_status(&resized, "work", now + Duration::from_millis(100))
        .unwrap();
    assert_eq!(os.stdout_bytes().len(), previous_len);
    recovery
        .paint_status(&resized, "work", now + Duration::from_millis(500))
        .unwrap();
    let output = String::from_utf8(os.stdout_bytes()[previous_len..].to_vec()).unwrap();
    assert!(!output.contains("📡"));
    assert!(output.contains("Reconnecting"));
    assert!(!output.contains(crate::terminal_escapes::CLEAR_SCREEN));
    let previous_len = os.stdout_bytes().len();
    recovery
        .paint_status(&resized, "work", now + Duration::from_secs(1))
        .unwrap();
    let output = String::from_utf8(os.stdout_bytes()[previous_len..].to_vec()).unwrap();
    assert!(output.contains("📡"));
}

#[test]
fn recovery_status_fits_small_terminals_and_wide_host_names() {
    let positions = regex::Regex::new(r"\x1b\[(\d+);(\d+)H").unwrap();
    for size in [
        Size {
            rows: 24,
            cols: 120,
        },
        Size { rows: 12, cols: 40 },
        Size { rows: 6, cols: 20 },
        Size { rows: 3, cols: 8 },
        Size { rows: 1, cols: 1 },
    ] {
        let os = crate::test_support::FakeClientOsApi::new()
            .capturing_stdout()
            .with_terminal_size(size);
        let mut recovery = Recovery::new(
            "遠隔".repeat(80),
            CommandIsExecuting::new(),
            &Default::default(),
            &Default::default(),
        );
        recovery.control.lost(0, "outage".into());
        recovery.paint_status(&os, "work", Instant::now()).unwrap();
        let (_, _, message) = recovery.status.as_ref().unwrap();
        let rows: Vec<_> = positions.captures_iter(message).collect();
        for (index, position) in rows.iter().enumerate() {
            let row: usize = position[1].parse().unwrap();
            let col: usize = position[2].parse().unwrap();
            let start = position.get(0).unwrap().end();
            let end = rows
                .get(index + 1)
                .map(|next| next.get(0).unwrap().start())
                .unwrap_or(message.len());
            let width = unicode_width::UnicodeWidthStr::width(&message[start..end]);
            assert!(row <= size.rows);
            assert_eq!(width, size.cols.min(80));
            assert!(col + width - 1 <= size.cols);
        }
    }
}

#[test]
fn replay_merges_color_registers_by_index_with_the_latest_value_winning() {
    use yinz_ipc::ipc::ColorRegister;

    let control = RecoveryControl::new(CommandIsExecuting::new());
    // Normalize duplicates even in the first report.
    for report in [
        vec![(2, "old"), (2, "green"), (1, "red")],
        vec![(3, "blue")],
        vec![(1, "old"), (1, "new")],
        vec![],
    ] {
        control.cache(&ClientToServerMsg::ColorRegisters {
            color_registers: report
                .into_iter()
                .map(|(index, color)| ColorRegister {
                    index,
                    color: color.into(),
                })
                .collect(),
        });
        control.lost(0, "offline".into());
    }
    control.replacing();
    let os = crate::test_support::FakeClientOsApi::new();
    control.replay(&os);
    let registers: Vec<_> = os
        .sent_messages()
        .into_iter()
        .filter_map(|message| match message {
            ClientToServerMsg::ColorRegisters { color_registers } => Some(color_registers),
            _ => None,
        })
        .collect();
    assert_eq!(
        registers,
        vec![vec![
            ColorRegister {
                index: 1,
                color: "new".into()
            },
            ColorRegister {
                index: 2,
                color: "green".into()
            },
            ColorRegister {
                index: 3,
                color: "blue".into()
            },
        ]]
    );
}
