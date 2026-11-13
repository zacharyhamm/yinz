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
        assert!(matches!(replacement, FontUpdate::Unchanged));
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
        "regular", "bold", "italic", "both", "weight", "system", "size",
    ] {
        let mut changed = settings.clone();
        match field {
            "regular" => changed.font_family = Some("Iosevka Term".into()),
            "bold" => changed.font_bold = Some("Iosevka Term".into()),
            "italic" => changed.font_italic = Some("Iosevka Term".into()),
            "both" => changed.font_bold_italic = Some("Iosevka Term".into()),
            "weight" => changed.font_weight = Some(700),
            "system" => changed.system_fonts = Some(true),
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
        assert!(matches!(replacement, FontUpdate::Replacement(_)));
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
    assert!(matches!(
        prepare_fonts(&active, &effective_font_options(&settings, 1.0, 1.0)).unwrap(),
        FontUpdate::Unchanged
    ));
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
            let FontUpdate::Replacement(cache) = prepare_fonts(&initial, &requested).unwrap()
            else {
                panic!("expected font replacement");
            };
            *cache
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

        // Start a different selection after the same production resize hook.
        // Its row/column must change for each actual font-derived grid.
        let text = format!(
            "{}ABCD{}WXYZ{}",
            "x".repeat(70),
            "y".repeat(16),
            "z".repeat(46)
        );
        let old_frame = Painter::frame(first.grid.rows, first.grid.cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(first.grid.cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
        });
        let mut state = TerminalState::new(first.grid.rows, first.grid.cols);
        state.apply_frame(&old_frame).unwrap();
        let mut selection = LocalSelection::default();
        selection.start(0, 70);
        selection.extend(0, 73);
        for message in geometry.messages(Some(first)) {
            if let ClientToServerMsg::TerminalResize { new_size } = message {
                selection.resizing(&state, new_size.rows, new_size.cols);
            }
        }
        let old_range = Selection::at(90 / first.grid.cols, 90 % first.grid.cols)
            .extended_to(93 / first.grid.cols, 93 % first.grid.cols);
        selection.start(old_range.anchor.0, old_range.anchor.1);
        selection.extend(old_range.head.0, old_range.head.1);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        selection.apply_frame(&mut state, &old_frame).unwrap();
        let frame = Painter::frame(geometry.grid.rows, geometry.grid.cols, |p| {
            p.full_repaint();
            for (row, chunk) in text.as_bytes().chunks(geometry.grid.cols).enumerate() {
                p.text(row, 0, std::str::from_utf8(chunk).unwrap());
            }
        });
        selection.apply_frame(&mut state, &frame).unwrap();
        let expected = Selection::at(90 / geometry.grid.cols, 90 % geometry.grid.cols)
            .extended_to(93 / geometry.grid.cols, 93 % geometry.grid.cols);
        assert_ne!(expected, old_range);
        assert_ne!(old_range.text(state.screen()), "WXYZ");
        assert_eq!(expected.text(state.screen()), "WXYZ");
        assert_eq!(selection.range(), Some(expected));
        assert_eq!(selection.text(state.screen()).as_deref(), Some("WXYZ"));
        // Mirror App's session-switch clear/state reset before the next frame.
        selection.clear();
        state = TerminalState::new(geometry.grid.rows, geometry.grid.cols);
        selection.apply_frame(&mut state, &old_frame).unwrap();
        assert_eq!(selection.range(), None);
        assert_eq!(selection.text(state.screen()), None);
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

#[test]
fn ligature_reload_from_files_preserves_warmed_caches_cells_selection_and_geometry() {
    use crate::testing::painter::Painter;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.kdl");
    let drop_ins = directory.path().join("config.d");
    std::fs::create_dir(&drop_ins).unwrap();
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let opts = CliArgs {
        config: Some(path.clone()),
        config_dir: Some(directory.path().to_owned()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut settings = load();
    let mut active = effective_font_options(&settings, 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let state = Painter::state(3, 16, |p| {
        p.text(0, 0, "a->b =>");
        p.cluster(1, 0, "e\u{301}", 1);
        p.wide(1, 3, '😀');
        p.wide(2, 0, '中');
        p.cursor(0, 10, crate::emulation::screen_buffer::CursorShape::Beam);
    });
    let mut selection = LocalSelection::default();
    selection.start(0, 1);
    selection.extend(0, 2);
    retained.set_selection(selection.range());
    let range = selection.range();
    let cursor = state.cursor_position();
    let cells: Vec<_> = (0..3)
        .flat_map(|row| (0..16).map(move |col| (row, col)))
        .map(|(row, col)| state.screen().cell(row, col))
        .collect();
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints: &Paints::default(),
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        );
    };
    refresh(&mut retained, &mut cache);
    let joined = retained.flatten();
    let original_geometry = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
    let generation = cache.generation();
    let font = cache.primary_of(crate::text::font::FaceStyle::Regular);
    let joined_ids = cache.shape(font, "->");
    let mut previous = true;
    for (name, enabled) in [
        ("ligatures", true),
        ("font_ligatures", true),
        ("ligatures", false),
        ("font_ligatures", false),
        ("font_ligatures", true),
        ("ligatures", false),
        ("ligatures", true),
    ] {
        std::fs::write(
            drop_ins.join("20-ligatures.kdl"),
            format!("terminal {{ {name} {enabled}; }}"),
        )
        .unwrap();
        let next = load();
        let requested = effective_font_options(&next, 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert_eq!(
            matches!(update, FontUpdate::LigaturesOnly),
            previous != enabled
        );
        let entries = cache.entries();
        let clusters = cache.cluster_entries();
        let shaped_clusters = cache.shaped_clusters();
        let atlas = (cache.atlases().mask.stamp(), cache.atlases().color.stamp());
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        if previous != enabled {
            assert_eq!(cache.shaped_runs(), 0);
        }
        assert_eq!(cache.shaped_clusters(), shaped_clusters);
        assert_eq!(cache.entries(), entries);
        assert_eq!(cache.cluster_entries(), clusters);
        assert_eq!(
            (cache.atlases().mask.stamp(), cache.atlases().color.stamp()),
            atlas
        );
        assert_eq!(cache.generation(), generation);
        assert_eq!(
            cache.primary_of(crate::text::font::FaceStyle::Regular),
            font
        );
        assert!(Geometry::calculate(400, 200, 0, cache.metrics())
            .unwrap()
            .messages(Some(original_geometry))
            .is_empty());
        refresh(&mut retained, &mut cache);
        if previous != enabled {
            assert!(retained.rebuilt_everything());
        }
        if enabled {
            assert_eq!(retained.flatten(), joined);
        } else {
            assert_ne!(retained.flatten(), joined);
        }
        let glyphs = cache.shape(font, "->");
        // Disabled production scenes use nominal glyphs; direct shape() remains
        // available for explicit feature callers and is bypassed by the scene.
        if enabled {
            assert_eq!(glyphs, joined_ids);
        }
        assert_eq!(selection.range(), range);
        assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
        assert_eq!(state.cursor_position(), cursor);
        for (index, cell) in cells.iter().enumerate() {
            assert_eq!(state.screen().cell(index / 16, index % 16), *cell);
        }
        assert_eq!(active, effective_font_options(&next, 1.0, 1.0));
        settings = next;
        previous = enabled;
    }
    // Invalid main configuration aborts loading before fonts/settings commit.
    let before = retained.flatten();
    let atlas = cache.atlases().mask.stamp();
    std::fs::write(&path, "terminal { ligatures \"false\"; }").unwrap();
    assert!(yinz_cli::setup::Setup::config_without_layout(&opts).is_err());
    assert_eq!(settings.font_ligatures, Some(true));
    let invalid = FontOptions {
        size: f32::NAN,
        ligatures: false,
        ..active.clone()
    };
    assert!(prepare_fonts(&active, &invalid).is_err());
    assert_eq!(cache.atlases().mask.stamp(), atlas);
    assert_eq!(cache.generation(), generation);
    assert_eq!(retained.flatten(), before);
    assert_eq!(selection.range(), range);
    assert_eq!(selection.text(state.screen()).as_deref(), Some("->"));
    let mixed = FontOptions {
        size: active.size + 2.0,
        ligatures: false,
        ..active.clone()
    };
    assert!(matches!(
        prepare_fonts(&active, &mixed).unwrap(),
        FontUpdate::Replacement(_)
    ));
    let features = FontOptions {
        features: vec![crate::text::font::FontFeature::parse("zero").unwrap()],
        ligatures: false,
        ..active.clone()
    };
    assert!(matches!(
        prepare_fonts(&active, &features).unwrap(),
        FontUpdate::Replacement(_)
    ));
}

#[test]
fn cell_metrics_fractional_dpi_zoom_rounding_grid_edges_and_hit_testing() {
    for (points, zoom) in [(12.0, 1.0), (11.3, 1.15), (13.25, 0.85)] {
        for scale in [1.0, 1.25, 1.5, 2.0, 1.0] {
            let settings = TerminalConfig {
                font_size: Some(points),
                ..embedded_settings()
            };
            let natural = effective_font_options(&settings, zoom, scale)
                .stack(1.0)
                .unwrap()
                .metrics();
            for width in [0.5, 1.0, 1.375, 3.0] {
                for height in [0.5, 1.0, 1.275, 3.0] {
                    let settings = TerminalConfig {
                        cell_width: Some(width),
                        line_height: Some(height),
                        ..settings.clone()
                    };
                    let options = effective_font_options(&settings, zoom, scale);
                    assert_eq!(options.cell.cell_width, width);
                    assert_eq!(options.cell.line_height, height);
                    let metrics = options.stack(1.0).unwrap().metrics();
                    let w = (natural.width as f32 * width).round().max(1.0) as u32;
                    let h = (natural.height as f32 * height).round().max(1.0) as u32;
                    assert_eq!((metrics.width, metrics.height), (w, h));
                    let padding = (3.0 * scale).round() as u32;
                    let exact = Geometry::calculate(
                        17 * w + 2 * padding,
                        7 * h + 2 * padding,
                        padding,
                        metrics,
                    )
                    .unwrap();
                    assert_eq!(exact.grid, Size { cols: 17, rows: 7 });
                    let below = Geometry::calculate(
                        17 * w + 2 * padding - 1,
                        7 * h + 2 * padding - 1,
                        padding,
                        metrics,
                    )
                    .unwrap();
                    assert_eq!(below.grid, Size { cols: 16, rows: 6 });
                    assert_eq!(
                        Geometry::calculate(1, 1, padding, metrics).unwrap().grid,
                        Size { cols: 1, rows: 1 }
                    );
                    assert!(Geometry::calculate(0, 20, padding, metrics).is_none());
                    assert!(Geometry::calculate(20, 0, padding, metrics).is_none());
                    let size = crate::emulation::screen_buffer::TermSize { cols: 17, rows: 7 };
                    let edge = (
                        padding as f64 + 3.0 * w as f64,
                        padding as f64 + 2.0 * h as f64,
                    );
                    assert_eq!(pointer_cell(edge, padding, metrics, size), (2, 3));
                    assert_eq!(
                        pointer_cell((edge.0 - 0.01, edge.1 - 0.01), padding, metrics, size),
                        (1, 2)
                    );
                    assert_eq!(pointer_cell((-10.0, -10.0), padding, metrics, size), (0, 0));
                    assert_eq!(pointer_cell((1e6, 1e6), padding, metrics, size), (6, 16));
                    let tiny = Geometry::calculate(1, 1, padding, metrics).unwrap();
                    let old_tiny = Geometry::calculate(1, 1, padding, natural).unwrap();
                    if tiny.cell != old_tiny.cell {
                        assert!(matches!(
                            &tiny.messages(Some(old_tiny))[..],
                            [ClientToServerMsg::TerminalPixelDimensions { .. }]
                        ));
                    }
                    let messages = exact.messages(None);
                    let ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions } =
                        messages[0]
                    else {
                        panic!("pixel report first")
                    };
                    assert_eq!(
                        pixel_dimensions.character_cell_size,
                        Some(SizeInPixels {
                            width: w as usize,
                            height: h as usize
                        })
                    );
                    assert_eq!(
                        pixel_dimensions.text_area_size,
                        Some(SizeInPixels {
                            width: 17 * w as usize,
                            height: 7 * h as usize
                        })
                    );
                    assert_eq!(pixel_dimensions.text_area_grid_size, Some(exact.grid));
                    assert!(
                        matches!(messages[1], ClientToServerMsg::TerminalResize { new_size } if new_size == exact.grid)
                    );
                    assert!(exact.messages(Some(exact)).is_empty());
                    // Render frames may still contain the old grid; replies use reported geometry.
                    let geometry =
                        host_query_geometry(Some(exact), Size { cols: 80, rows: 24 }, metrics);
                    assert_eq!(
                        crate::host_reply::answer(b"\x1b[16t", geometry, &Paints::default(), None),
                        format!("\x1b[6;{h};{w}t").as_bytes()
                    );
                    assert_eq!(
                        crate::host_reply::answer(b"\x1b[14t", geometry, &Paints::default(), None),
                        format!("\x1b[4;{};{}t", 7 * h, 17 * w).as_bytes()
                    );
                }
            }
        }
    }
}

#[test]
fn cell_metrics_file_reload_uses_metric_path_and_preserves_content_copy_and_rounding_noops() {
    use crate::emulation::selection::LocalSelection;
    use crate::testing::painter::Painter;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    std::fs::write(&path, "terminal { system_fonts false; }").unwrap();
    let opts = CliArgs {
        config: Some(path.clone()),
        config_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let load = || {
        yinz_cli::setup::Setup::config_without_layout(&opts)
            .unwrap()
            .0
            .terminal
    };
    let mut active = effective_font_options(&load(), 1.0, 1.0);
    let mut cache = GlyphCache::new(active.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    let mut state = Painter::state(2, 16, |p| {
        p.text(0, 0, "Ag -> fi");
        p.wide(1, 0, '中');
        p.wide(1, 2, '😀');
        p.cluster(1, 4, "👩‍💻", 2);
        p.cluster(1, 6, "e\u{301}", 1);
    });
    let mut selection = LocalSelection::default();
    selection.start(1, 0);
    selection.extend(1, 6);
    let expected = selection.text(state.screen());
    let range = selection.range();
    let cells: Vec<_> = (0..32)
        .map(|i| state.screen().cell(i / 16, i % 16))
        .collect();
    let refresh = |retained: &mut RetainedScene, cache: &mut GlyphCache| {
        retained.refresh(
            SceneInputs {
                state: &state,
                phase: BlinkPhase::On,
                paints: &Paints::default(),
                cursor: CursorOptions::default(),
                hovered_link: None,
                preedit: None,
            },
            cache,
        )
    };
    retained.set_selection(range);
    refresh(&mut retained, &mut cache);
    for (width, height, ligatures) in [
        (1.5, 1.0, true),
        (1.5, 1.25, true),
        (0.5, 0.5, false),
        (3.0, 3.0, true),
        (1.0, 1.0, true),
        (1.001, 1.001, true),
        (1.002, 1.002, false),
    ] {
        std::fs::write(&path, format!("terminal {{ system_fonts false; cell_width {width}; line_height {height}; ligatures {ligatures}; }}")).unwrap();
        let requested = effective_font_options(&load(), 1.0, 1.0);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(update, FontUpdate::CellOnly));
        let generation = cache.generation();
        let atlas = cache.atlases().mask.stamp();
        let clusters = cache.shaped_clusters();
        let old = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
        apply_fonts(
            &mut active,
            &mut cache,
            &mut retained,
            requested.clone(),
            update,
        );
        assert_eq!(cache.shaped_clusters(), clusters);
        let new = Geometry::calculate(400, 200, 0, cache.metrics()).unwrap();
        if width > 1.0 && width < 1.01 {
            assert_eq!(cache.generation(), generation);
            assert_eq!(cache.atlases().mask.stamp(), atlas);
            assert!(new.messages(Some(old)).is_empty());
        } else {
            assert_ne!(cache.generation(), generation);
            assert_ne!(cache.atlases().mask.stamp().identity, atlas.identity);
        }
        refresh(&mut retained, &mut cache);
        let mut fresh = GlyphCache::new(requested.stack(1.0).unwrap());
        let mut fresh_scene = RetainedScene::new();
        fresh_scene.set_selection(range);
        refresh(&mut fresh_scene, &mut fresh);
        // Preserved atlas entries can have different packing after a ligature
        // toggle. Compare concrete placed geometry; GPU tests compare actual ink.
        let normalize = |mut scene: crate::render::scene::test_helpers::Scene| {
            for glyph in scene.glyphs.iter_mut().chain(scene.color_glyphs.iter_mut()) {
                glyph.entry.x = 0;
                glyph.entry.y = 0;
            }
            scene
        };
        assert_eq!(
            normalize(retained.flatten()),
            normalize(fresh_scene.flatten())
        );
        assert_eq!(selection.range(), range);
        assert_eq!(selection.text(state.screen()), expected);
        for (i, cell) in cells.iter().enumerate() {
            assert_eq!(state.screen().cell(i / 16, i % 16), *cell);
        }
    }
    // A font/zoom/monitor-scale change still replaces affected font resources.
    for (zoom, scale) in [(1.2, 1.0), (1.2, 1.25), (1.2, 1.5), (1.2, 2.0), (1.0, 1.0)] {
        let requested = effective_font_options(&load(), zoom, scale);
        let update = prepare_fonts(&active, &requested).unwrap();
        assert!(matches!(update, FontUpdate::Replacement(_)));
        apply_fonts(&mut active, &mut cache, &mut retained, requested, update);
        assert_eq!(selection.text(state.screen()), expected);
    }
    state.set_cell_size(cache.metrics().width, cache.metrics().height);
    let generation = cache.generation();
    std::fs::write(&path, "terminal { cell_width 4; }").unwrap();
    assert!(yinz_cli::setup::Setup::config_without_layout(&opts).is_err());
    assert_eq!(cache.generation(), generation);
}

#[test]
fn cell_metrics_emulation_graphics_and_wide_hit_cells_use_the_measured_cache() {
    use crate::testing::painter::{residency, Painter, Place};
    let mut state = TerminalState::new(4, 12);
    for (width, height) in [(1.0, 1.0), (1.5, 1.25), (0.5, 0.5), (1.0, 1.0)] {
        let settings = TerminalConfig {
            cell_width: Some(width),
            line_height: Some(height),
            ..embedded_settings()
        };
        let cache = GlyphCache::new(
            effective_font_options(&settings, 1.0, 1.0)
                .stack(1.0)
                .unwrap(),
        );
        let metrics = cache.metrics();
        state.set_cell_size(metrics.width, metrics.height);
        Painter::apply(&mut state, |p| {
            p.wide(1, 2, '中');
            p.graphics(&[
                residency(7, 2, 2, [1, 2, 3, 255]),
                Place::of(7, 2, 2).at(2, 3).record(),
            ]);
        });
        let images = crate::render::scene::images_of(&state, metrics);
        assert_eq!(images.len(), 1);
        assert_eq!(
            (images[0].x, images[0].y),
            ((3 * metrics.width) as i32, (2 * metrics.height) as i32)
        );
        let pad = 5;
        let first = pointer_cell(
            (
                (pad + 2 * metrics.width) as f64,
                (pad + metrics.height) as f64,
            ),
            pad,
            metrics,
            state.size(),
        );
        let spacer = pointer_cell(
            (
                (pad + 4 * metrics.width) as f64 - 0.01,
                (pad + 2 * metrics.height) as f64 - 0.01,
            ),
            pad,
            metrics,
            state.size(),
        );
        assert_eq!(first, (1, 2));
        assert_eq!(spacer, (1, 3));
        let selection = Selection::at(first.0, first.1).extended_to(spacer.0, spacer.1);
        assert_eq!(selection.text(state.screen()), "中");
        // Sixel cell changes clear old chunks; identical metrics preserve them.
        let frame = Painter::frame(4, 12, |p| {
            p.graphics(&[crate::testing::painter::sixel_chunk(
                2,
                3,
                2,
                6,
                "\x1bPq#1;2;100;0;0!2~\x1b\\",
            )])
        });
        state.apply_frame(&frame).unwrap();
        assert!(!state.sixels().is_empty());
        state.set_cell_size(metrics.width, metrics.height);
        assert!(!state.sixels().is_empty());
        state.set_cell_size(metrics.width + 1, metrics.height);
        assert!(state.sixels().is_empty());
    }
}
