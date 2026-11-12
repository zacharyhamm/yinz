use super::*;
use crate::emulation::selection::Selection;
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

fn embedded_settings() -> TerminalConfig {
    TerminalConfig {
        system_fonts: Some(false),
        ..Default::default()
    }
}

#[test]
fn four_faces_weight_defaults_and_blank_resets_reach_production_options() {
    let settings = TerminalConfig {
        font_family: Some("main".into()),
        font_bold: Some("bold".into()),
        font_italic: Some("italic".into()),
        font_bold_italic: Some("both".into()),
        font_weight: Some(600),
        ..Default::default()
    };
    let options = font_options(&settings, 1.0);
    assert_eq!(options.family.as_deref(), Some("main"));
    assert_eq!(options.bold_family.as_deref(), Some("bold"));
    assert_eq!(options.italic_family.as_deref(), Some("italic"));
    assert_eq!(options.bold_italic_family.as_deref(), Some("both"));
    assert_eq!(options.weight, 600);
    let blank = TerminalConfig {
        font_family: Some(" ".into()),
        font_bold: Some("".into()),
        font_italic: Some("\t".into()),
        font_bold_italic: Some("  ".into()),
        ..Default::default()
    };
    assert_eq!(
        font_options(&blank, 1.0),
        font_options(&TerminalConfig::default(), 1.0)
    );
}

#[test]
fn effective_font_options_bound_only_final_physical_size() {
    for (points, zoom, scale, expected) in [(3.0, 0.25, 1.0, 4.0), (384.0, 4.0, 4.0, 512.0)] {
        let settings = TerminalConfig {
            font_size: Some(points),
            ..embedded_settings()
        };
        let options = effective_font_options(&settings, zoom, scale);
        assert_eq!(options.size, expected);
        assert!(options.stack(1.0).is_ok());
        assert_eq!(settings.font_size, Some(points));
    }
    let settings = TerminalConfig {
        font_size: Some(384.0),
        ..embedded_settings()
    };
    assert_eq!(
        effective_font_options(&settings, 4.0, 2.0),
        effective_font_options(&settings, 4.0, 3.0)
    );
}

#[test]
fn font_application_preserves_warmed_cache_for_equivalent_and_paint_only_settings() {
    let settings = embedded_settings();
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = crate::testing::painter::Painter::state(2, 16, |painter| {
        painter.text(0, 0, "Ag -> fi");
        painter.cluster(1, 0, "e\u{301}", 1);
    });
    let selection = Selection::at(0, 0).extended_to(0, 1);
    retained.set_selection(Some(selection));
    retained.refresh(
        SceneInputs {
            state: &state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        &mut cache,
    );
    assert!(cache.entries() > 0);
    assert!(cache.shaped_runs() > 0);
    assert!(cache.cluster_entries() > 0);
    let atlas = cache.atlases().mask.stamp();
    let generation = cache.generation();
    for settings in [
        settings.clone(),
        TerminalConfig {
            font_size: Some(12.0),
            font_weight: Some(400),
            padding: Some(10.0),
            opacity: Some(0.5),
            font_family: Some(" ".into()),
            ..settings
        },
    ] {
        let requested = effective_font_options(&settings, 1.0, 1.0);
        let replacement = prepare_fonts(&active, &requested).unwrap();
        assert!(replacement.is_none());
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested,
            replacement,
        );
        assert_eq!(cache.generation(), generation);
        assert_eq!(cache.atlases().mask.stamp(), atlas);
        assert_eq!(selection.text(state.screen()), "Ag");
    }
}

#[test]
fn font_application_changes_each_font_input_and_failure_keeps_working_cache() {
    let settings = embedded_settings();
    let base = effective_font_options(&settings, 1.0, 1.0);
    let mut variants = Vec::new();
    for field in [
        "regular",
        "bold",
        "italic",
        "both",
        "weight",
        "ligatures",
        "system",
        "line",
        "size",
    ] {
        let mut changed = settings.clone();
        match field {
            "regular" => changed.font_family = Some("Iosevka Term".into()),
            "bold" => changed.font_bold = Some("Iosevka Term".into()),
            "italic" => changed.font_italic = Some("Iosevka Term".into()),
            "both" => changed.font_bold_italic = Some("Iosevka Term".into()),
            "weight" => changed.font_weight = Some(700),
            "ligatures" => changed.font_ligatures = Some(false),
            "system" => changed.system_fonts = Some(true),
            "line" => changed.line_height = Some(1.5),
            _ => changed.font_size = Some(18.0),
        }
        variants.push(effective_font_options(&changed, 1.0, 1.0));
    }
    variants.push(effective_font_options(&settings, 1.2, 1.0));
    variants.push(effective_font_options(&settings, 1.0, 2.0));
    for requested in variants {
        let mut active = base.clone();
        let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
        let generation = cache.generation();
        let mut retained = RetainedScene::new();
        let replacement = prepare_fonts(&active, &requested).unwrap();
        assert!(replacement.is_some());
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            replacement,
        );
        assert_ne!(cache.generation(), generation);
        assert_eq!(active, requested);
        let invalid = FontOptions {
            size: f32::NAN,
            ..requested.clone()
        };
        let generation = cache.generation();
        assert!(prepare_fonts(&active, &invalid).is_err());
        assert_eq!(cache.generation(), generation);
        assert_eq!(active, requested);
    }
}

#[test]
fn geometry_reports_cell_changes_even_with_same_grid_and_forces_new_connections() {
    let metrics = effective_font_options(&embedded_settings(), 1.0, 1.0)
        .stack(1.0)
        .unwrap()
        .metrics();
    let first = Geometry::calculate(80, 40, 0, metrics).unwrap();
    assert_eq!(
        first.messages(None).len(),
        2,
        "initial, reconnect and explicit queries have no snapshot"
    );
    assert!(first.messages(Some(first)).is_empty());
    let changed_cell = Geometry {
        cell: SizeInPixels {
            width: first.cell.width + 1,
            ..first.cell
        },
        ..first
    };
    assert!(matches!(
        &changed_cell.messages(Some(first))[..],
        [ClientToServerMsg::TerminalPixelDimensions { .. }]
    ));
    let padded = Geometry::calculate(80, 40, 10, metrics).unwrap();
    assert_eq!(padded.messages(Some(first)).len(), 2);
    assert!(Geometry::calculate(0, 40, 0, metrics).is_none());
    assert!(Geometry::calculate(80, 0, 0, metrics).is_none());
    assert_eq!(
        Geometry::calculate(1, 1, 256, metrics).unwrap().grid,
        Size { rows: 1, cols: 1 }
    );
}

#[test]
fn paint_only_reload_refreshes_retained_ink_without_replacing_fonts() {
    let mut settings = embedded_settings();
    let mut paints = Paints::default();
    let active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = crate::testing::painter::Painter::state(1, 8, |painter| painter.text(0, 0, "Ag"));
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache, paints: &Paints| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints,
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache, &paints);
    let generation = cache.generation();
    let before = retained.flatten();
    let current = settings.clone();
    let next_paints = Paints {
        foreground: [12, 34, 56],
        ..paints
    };
    apply_settings(
        &mut settings,
        &mut paints,
        &mut retained,
        current.clone(),
        next_paints,
    );
    refresh(&mut retained, &mut cache, &paints);
    assert!(retained.rebuilt_everything());
    assert_ne!(retained.flatten(), before);
    assert_eq!(cache.generation(), generation);
    assert!(
        prepare_fonts(&active, &effective_font_options(&settings, 1.0, 1.0))
            .unwrap()
            .is_none()
    );
    apply_settings(
        &mut settings,
        &mut paints,
        &mut retained,
        current,
        next_paints,
    );
    refresh(&mut retained, &mut cache, &paints);
    assert!(
        !retained.rebuilt_everything(),
        "identical reload invalidated retained ink"
    );
}

#[test]
fn host_pixel_reply_uses_current_window_grid_while_rendered_content_catches_up() {
    let metrics = crate::text::font::CellMetrics {
        width: 21,
        height: 62,
        baseline: 45,
        underline_top: 48,
        strikeout_top: 32,
        stroke: 1,
    };
    let current = Geometry {
        cell: SizeInPixels {
            width: 21,
            height: 62,
        },
        grid: Size {
            rows: 43,
            cols: 119,
        },
    };
    let old_frame_grid = Size {
        rows: 57,
        cols: 132,
    };
    let geometry = host_query_geometry(Some(current), old_frame_grid, metrics);
    assert_eq!(
        crate::host_reply::answer(b"\x1b[14t", geometry, &Paints::default(), None),
        b"\x1b[4;2666;2499t"
    );
    assert_eq!(
        crate::host_reply::answer(b"\x1b[16t", geometry, &Paints::default(), None),
        b"\x1b[6;62;21t"
    );
    let initial = host_query_geometry(None, old_frame_grid, metrics);
    assert_eq!(initial.rows, 57);
    assert_eq!(initial.cols, 132);
}

#[test]
fn font_reload_zoom_and_scale_geometry_share_selection_frame_and_copy_transition() {
    use crate::testing::painter::Painter;
    let original_settings = embedded_settings();
    let initial = effective_font_options(&original_settings, 1.0, 1.0);
    let initial_metrics = initial.stack(1.0).unwrap().metrics();
    let width = initial_metrics.width * 80;
    let height = initial_metrics.height * 8;
    let first = Geometry::calculate(width, height, 0, initial_metrics).unwrap();
    let changed = TerminalConfig {
        font_family: Some("Noto Sans Mono CJK SC".into()),
        system_fonts: Some(true),
        ..original_settings.clone()
    };
    for requested in [
        effective_font_options(&changed, 1.0, 1.0),
        effective_font_options(&original_settings, 1.5, 1.0),
        effective_font_options(&original_settings, 1.0, 1.5),
    ] {
        let replacement = if requested.family.is_some() {
            // A regular-family reload uses deterministic bundled discovery.
            let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
            let discovery = crate::text::discovery::Discovery::synthetic(&directory).unwrap();
            GlyphCache::new(
                crate::text::font::FontStack::build_with(&requested, Some(discovery)).unwrap(),
            )
        } else {
            prepare_fonts(&initial, &requested).unwrap().unwrap()
        };
        let geometry = Geometry::calculate(width, height, 0, replacement.metrics()).unwrap();
        assert!(geometry.grid.cols < 70);
        let text = format!("{}ABCD end", "x".repeat(70));
        let mut state = Painter::state(first.grid.rows, first.grid.cols, |p| p.text(0, 0, &text));
        let mut selection = LocalSelection::default();
        selection.start(0, 70);
        selection.extend(0, 73);
        // This is the same geometry message/capture hook used by App after
        // successful preparation for reload, keyboard zoom and monitor scale.
        for message in geometry.messages(Some(first)) {
            if let ClientToServerMsg::TerminalResize { new_size } = message {
                selection.resizing(&state, new_size.rows, new_size.cols);
            }
        }
        let frame = Painter::frame(geometry.grid.rows, geometry.grid.cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(geometry.grid.cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
        });
        selection.apply_frame(&mut state, &frame).unwrap();
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
        assert_eq!(
            selection.range(),
            Some(
                Selection::at(70 / geometry.grid.cols, 70 % geometry.grid.cols)
                    .extended_to(73 / geometry.grid.cols, 73 % geometry.grid.cols)
            )
        );
        let update = Painter::frame(geometry.grid.rows, geometry.grid.cols, |p| {
            for (offset, ch) in "WXYZ".char_indices() {
                let index = 70 + offset;
                p.text(
                    index / geometry.grid.cols,
                    index % geometry.grid.cols,
                    &ch.to_string(),
                );
            }
        });
        selection.apply_frame(&mut state, &update).unwrap();
        assert_eq!(selection.range(), None);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
    }
    // Failed font preparation never reaches the geometry/capture hook.
    let invalid = FontOptions {
        size: f32::NAN,
        ..initial.clone()
    };
    assert!(prepare_fonts(&initial, &invalid).is_err());
    let mut state = Painter::state(3, 80, |p| p.text(0, 0, "old!"));
    let mut selection = LocalSelection::default();
    selection.start(0, 0);
    selection.extend(0, 3);
    selection
        .apply_frame(&mut state, &Painter::frame(3, 80, |p| p.text(0, 0, "new!")))
        .unwrap();
    assert_eq!(selection.text(state.screen()).as_deref(), Some("new!"));
}
